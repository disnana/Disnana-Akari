use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt,
    panic::catch_unwind,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio::{
    sync::{mpsc, oneshot},
    time::{Instant, sleep_until},
};
use twilight_model::{
    gateway::{
        Intents, ShardId,
        payload::{incoming::MemberChunk, outgoing::RequestGuildMembers},
        presence::Presence,
    },
    guild::Member,
    id::{
        Id,
        marker::{GuildMarker, UserMarker},
    },
};

const MAX_MEMBERS: usize = 100;
const MAX_FULL_MEMBERS: usize = 100_000;
const MAX_RESERVED_ITEMS: usize = 200_000;
const MAX_CHUNK_MEMBERS: usize = 1_000;
const MAX_PREFIX_CHARS: usize = 32;
const MAX_CHUNKS: u32 = 16;
const MAX_FULL_CHUNKS: u32 = 1_024;
const MAX_PENDING: usize = 64;
const MAX_PENDING_PER_SHARD: usize = 16;
const MAX_RATE_KEYS: usize = 1_024;
const FULL_QUERY_INTERVAL: Duration = Duration::from_secs(30);
const NONCE_PREFIX: &str = "akari-";
static NEXT_NONCE: AtomicU64 = AtomicU64::new(1);

/// Gateway に対象を指定してサーバーのメンバーを問い合わせるハンドルです。
///
/// ユーザー名の先頭一致、ユーザー ID の指定、件数上限付きの全メンバー取得に対応します。
/// プレゼンスは明示して追加できます。取得結果を Akari のメンバーキャッシュへ自動で保存しません。
#[derive(Clone)]
pub struct MemberQuery {
    state: Arc<Mutex<State>>,
}

impl fmt::Debug for MemberQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemberQuery").finish_non_exhaustive()
    }
}

/// Gateway に問い合わせたメンバーと、見つからなかったユーザーです。
#[derive(Debug)]
pub struct MemberQueryResult {
    members: Vec<Member>,
    not_found: Vec<Id<UserMarker>>,
    presences: Vec<Presence>,
}

impl MemberQueryResult {
    /// Discord が返したメンバーをチャンク順に返します。
    pub fn members(&self) -> &[Member] {
        &self.members
    }

    /// ID を指定した問い合わせで、Discord が不在と報告したユーザーを返します。
    pub fn not_found(&self) -> &[Id<UserMarker>] {
        &self.not_found
    }

    /// プレゼンスを要求した問い合わせで、Discord が返したプレゼンスです。
    ///
    /// プレゼンスが返らなかったメンバーは一覧に含まれません。
    pub fn presences(&self) -> &[Presence] {
        &self.presences
    }

    /// 問い合わせ結果からメンバーの一覧を取り出します。
    pub fn into_members(self) -> Vec<Member> {
        self.members
    }

    /// メンバー、不在と報告されたユーザー、プレゼンスを順に取り出します。
    pub fn into_parts(self) -> (Vec<Member>, Vec<Id<UserMarker>>, Vec<Presence>) {
        (self.members, self.not_found, self.presences)
    }
}

/// Gateway へのメンバー問い合わせが失敗した理由です。
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum MemberQueryError {
    /// ユーザー名の先頭文字列が空か、32 文字を超えています。
    #[error("ユーザー名の先頭文字列が無効です")]
    InvalidPrefix,
    /// 件数は 1〜100 の間で指定してください。
    #[error("メンバーの取得件数が無効です")]
    InvalidLimit,
    /// 全メンバーの取得上限は 1〜100,000 件の間で指定してください。
    #[error("全メンバーの取得上限が無効です")]
    InvalidFullLimit,
    /// 全メンバーの取得で指定した上限を超えました。部分的な一覧は返りません。
    #[error("全メンバーが取得上限 {limit} 件を超えました")]
    LimitExceeded {
        /// 呼び出し側が指定した取得上限です。
        limit: usize,
    },
    /// 全メンバーまたはプレゼンスの取得に必要な Gateway Intent が足りません。
    #[error("メンバー問い合わせに必要な Gateway Intent がありません: {required:?}")]
    MissingIntents {
        /// 指定されていなかった Gateway Intent です。
        required: Intents,
    },
    /// 重複のないユーザー ID を 1〜100 件指定してください。
    #[error("問い合わせるユーザー ID が無効です")]
    InvalidUserIds,
    /// 制限時間が 0、または表現できる範囲を超えています。
    #[error("メンバー問い合わせの制限時間が無効です")]
    InvalidTimeout,
    /// Tokio のタイマーを利用できません。
    #[error("メンバー問い合わせに必要な Tokio タイマーを利用できません")]
    TimerUnavailable,
    /// 対象の Gateway シャードで問い合わせを受け付けられません。
    #[error("対象の Gateway シャードは準備できていません")]
    NotReady,
    /// 同時問い合わせ枠または送信待ちの枠が埋まっています。
    #[error("メンバー問い合わせの処理枠がいっぱいです")]
    Busy,
    /// 同じサーバーへの全メンバー取得に、送信間隔の制限がかかっています。
    #[error("全メンバー問い合わせは {retry_after:?} 後に再試行できます")]
    RateLimited {
        /// 次の送信まで待つ時間です。
        retry_after: Duration,
    },
    /// 応答を受け取る前に制限時間を過ぎました。
    #[error("メンバー問い合わせの制限時間を過ぎました")]
    TimedOut,
    /// 対応する Discord の応答がチャンクの上限や形式に合いません。
    #[error("メンバー問い合わせの応答が無効です")]
    MalformedChunk,
    /// 問い合わせ中に対象シャードの接続が切れました。
    #[error("メンバー問い合わせ中に Gateway が切断されました")]
    Disconnected,
    /// Gateway に問い合わせを送信できませんでした。
    #[error("メンバー問い合わせを Gateway に送信できませんでした")]
    SendFailed,
    /// Bot の停止によって問い合わせを終了しました。
    #[error("メンバー問い合わせは終了しました")]
    Closed,
}

type QueryResult = Result<MemberQueryResult, MemberQueryError>;

impl MemberQuery {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    /// ユーザー名が指定した文字列から始まるメンバーを検索します。
    ///
    /// 件数は 1〜100、文字列は空でない 32 文字以内のものを指定します。
    /// 制限時間には送信待ちの時間を含みます。Future を破棄すると結果を待たずに枠を解放します。
    ///
    /// # エラー
    ///
    /// 入力、Gateway の状態、同時問い合わせ枠、応答の形式、または制限時間に応じて失敗します。
    pub async fn query_prefix(
        &self,
        guild: Id<GuildMarker>,
        prefix: &str,
        limit: usize,
        timeout: Duration,
    ) -> Result<MemberQueryResult, MemberQueryError> {
        self.query_prefix_impl(guild, prefix, limit, timeout, false)
            .await
    }

    /// ユーザー名の先頭一致で、メンバーとプレゼンスを取得します。
    ///
    /// # エラー
    ///
    /// [Self::query_prefix] のエラーに加え、Gateway に `GUILD_PRESENCES` がなければ失敗します。
    pub async fn query_prefix_with_presences(
        &self,
        guild: Id<GuildMarker>,
        prefix: &str,
        limit: usize,
        timeout: Duration,
    ) -> Result<MemberQueryResult, MemberQueryError> {
        self.query_prefix_impl(guild, prefix, limit, timeout, true)
            .await
    }

    async fn query_prefix_impl(
        &self,
        guild: Id<GuildMarker>,
        prefix: &str,
        limit: usize,
        timeout: Duration,
        presences: bool,
    ) -> QueryResult {
        if prefix.chars().take(MAX_PREFIX_CHARS + 1).count() > MAX_PREFIX_CHARS
            || prefix.trim().is_empty()
        {
            return Err(MemberQueryError::InvalidPrefix);
        }
        if !(1..=MAX_MEMBERS).contains(&limit) {
            return Err(MemberQueryError::InvalidLimit);
        }
        self.query(
            guild,
            Selection::Prefix(prefix.to_owned(), limit),
            timeout,
            presences,
        )
        .await
    }

    /// ユーザー ID を指定してメンバーを問い合わせます。
    ///
    /// 重複のない ID を 1〜100 件指定してください。
    ///
    /// # エラー
    ///
    /// 無効な ID の組み合わせを拒否します。その他は [Self::query_prefix] と同じです。
    pub async fn query_ids(
        &self,
        guild: Id<GuildMarker>,
        users: &[Id<UserMarker>],
        timeout: Duration,
    ) -> Result<MemberQueryResult, MemberQueryError> {
        self.query_ids_impl(guild, users, timeout, false).await
    }

    /// ユーザー ID を指定してメンバーとプレゼンスを取得します。
    ///
    /// # エラー
    ///
    /// [Self::query_ids] のエラーに加え、Gateway に `GUILD_PRESENCES` がなければ失敗します。
    pub async fn query_ids_with_presences(
        &self,
        guild: Id<GuildMarker>,
        users: &[Id<UserMarker>],
        timeout: Duration,
    ) -> Result<MemberQueryResult, MemberQueryError> {
        self.query_ids_impl(guild, users, timeout, true).await
    }

    async fn query_ids_impl(
        &self,
        guild: Id<GuildMarker>,
        users: &[Id<UserMarker>],
        timeout: Duration,
        presences: bool,
    ) -> QueryResult {
        if !(1..=MAX_MEMBERS).contains(&users.len())
            || users.iter().copied().collect::<HashSet<_>>().len() != users.len()
        {
            return Err(MemberQueryError::InvalidUserIds);
        }
        self.query(guild, Selection::Users(users.to_vec()), timeout, presences)
            .await
    }

    /// サーバーの全メンバーを、指定した上限以内に限って取得します。
    ///
    /// Gateway に `GUILD_MEMBERS` が必要です。同じサーバーへの全件要求の間隔は 30 秒です。
    /// 上限は 1〜100,000 件です。全チャンクが揃った場合に一覧を返し、途中結果は返しません。
    /// 同時問い合わせの予約枠も共有します。
    ///
    /// # エラー
    ///
    /// 上限の指定が無効、上限を超えた、必要な Intent がない、送信間隔の制限中などの場合に失敗します。
    pub async fn query_all(
        &self,
        guild: Id<GuildMarker>,
        max_members: usize,
        timeout: Duration,
    ) -> Result<MemberQueryResult, MemberQueryError> {
        self.query_all_impl(guild, max_members, timeout, false)
            .await
    }

    /// サーバーの全メンバーと、Discord が返せるプレゼンスを取得します。
    ///
    /// # エラー
    ///
    /// [Self::query_all] のエラーに加え、Gateway に `GUILD_PRESENCES` がなければ失敗します。
    pub async fn query_all_with_presences(
        &self,
        guild: Id<GuildMarker>,
        max_members: usize,
        timeout: Duration,
    ) -> Result<MemberQueryResult, MemberQueryError> {
        self.query_all_impl(guild, max_members, timeout, true).await
    }

    async fn query_all_impl(
        &self,
        guild: Id<GuildMarker>,
        max_members: usize,
        timeout: Duration,
        presences: bool,
    ) -> QueryResult {
        if !(1..=MAX_FULL_MEMBERS).contains(&max_members) {
            return Err(MemberQueryError::InvalidFullLimit);
        }
        self.query(guild, Selection::All(max_members), timeout, presences)
            .await
    }

    pub(crate) fn configure_intents(&self, intents: Intents) {
        if let Ok(mut state) = self.state.lock() {
            state.intents = intents;
        }
    }

    pub(crate) fn initialize(
        &self,
        shards: impl IntoIterator<Item = ShardId>,
    ) -> Vec<mpsc::Receiver<MemberQueryCommand>> {
        let Ok(mut state) = self.state.lock() else {
            return Vec::new();
        };
        if state.closed || !state.routes.is_empty() {
            return Vec::new();
        }
        shards
            .into_iter()
            .map(|shard| {
                let (sender, receiver) = mpsc::channel(MAX_PENDING_PER_SHARD);
                state.routes.insert(
                    shard,
                    Route {
                        sender,
                        ready: false,
                    },
                );
                receiver
            })
            .collect()
    }

    pub(crate) fn ready(&self, shard: ShardId) {
        if let Ok(mut state) = self.state.lock()
            && let Some(route) = state.routes.get_mut(&shard)
        {
            route.ready = true;
        }
    }

    pub(crate) fn disconnect(&self, shard: ShardId) {
        let cancelled = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            if let Some(route) = state.routes.get_mut(&shard) {
                route.ready = false;
            }
            state
                .pending
                .extract_if(|_, pending| pending.shard == shard)
                .map(|(_, pending)| pending)
                .collect::<Vec<_>>()
        };
        for pending in cancelled {
            let _ = pending.reply.send(Err(MemberQueryError::Disconnected));
        }
    }

    pub(crate) fn close(&self) {
        let (pending, routes) = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            state.closed = true;
            state.cooldowns.clear();
            (
                std::mem::take(&mut state.pending),
                std::mem::take(&mut state.routes),
            )
        };
        for (_, pending) in pending {
            let _ = pending.reply.send(Err(MemberQueryError::Closed));
        }
        drop(routes);
    }

    pub(crate) fn request<'a>(
        &self,
        shard: ShardId,
        command: &'a MemberQueryCommand,
    ) -> Option<&'a RequestGuildMembers> {
        let mut state = self.state.lock().ok()?;
        let pending = state.pending.get_mut(&command.id)?;
        let now = Instant::now();
        if pending.shard != shard || pending.deadline <= now || pending.sent {
            return None;
        }
        pending.sent = true;
        if pending.full {
            let guild = pending.guild;
            state.cooldowns.insert(
                guild,
                Cooldown {
                    command: command.id,
                    until: now + FULL_QUERY_INTERVAL,
                },
            );
        }
        Some(&command.payload)
    }

    pub(crate) fn send_failed(&self, shard: ShardId, command: &MemberQueryCommand) {
        let pending = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            if state
                .pending
                .get(&command.id)
                .is_none_or(|request| request.shard != shard)
            {
                return;
            }
            let pending = state.pending.remove(&command.id);
            if let Some(guild) = pending
                .as_ref()
                .filter(|pending| pending.full)
                .map(|pending| pending.guild)
                && state
                    .cooldowns
                    .get(&guild)
                    .is_some_and(|cooldown| cooldown.command == command.id)
            {
                state.cooldowns.remove(&guild);
            }
            pending
        };
        if let Some(pending) = pending {
            let _ = pending.reply.send(Err(MemberQueryError::SendFailed));
        }
    }

    pub(crate) fn observe_chunk(&self, shard: ShardId, chunk: MemberChunk) {
        let Some(id) = chunk.nonce.as_deref().and_then(parse_nonce) else {
            return;
        };
        let settled = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            let Some(pending) = state.pending.get_mut(&id) else {
                return;
            };
            if !pending.sent
                || pending.shard != shard
                || pending.guild != chunk.guild_id
                || pending.deadline <= Instant::now()
            {
                return;
            }
            match pending.receive(chunk) {
                Ok(false) => None,
                status => state.pending.remove(&id).map(|pending| (pending, status)),
            }
        };
        if let Some((pending, status)) = settled {
            pending.complete(status);
        }
    }

    pub(crate) fn observe_rate_limit(
        &self,
        shard: ShardId,
        guild: Id<GuildMarker>,
        nonce: &str,
        retry_after: Duration,
    ) {
        let Some(id) = parse_nonce(nonce) else { return };
        let pending = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            if state.pending.get(&id).is_none_or(|pending| {
                !pending.full || !pending.sent || pending.shard != shard || pending.guild != guild
            }) {
                return;
            }
            let now = Instant::now();
            let until = now
                .checked_add(retry_after)
                .unwrap_or(now + FULL_QUERY_INTERVAL);
            if let Some(cooldown) = state.cooldowns.get_mut(&guild)
                && cooldown.command == id
            {
                cooldown.until = cooldown.until.max(until);
            }
            state.pending.remove(&id)
        };
        if let Some(pending) = pending {
            let _ = pending
                .reply
                .send(Err(MemberQueryError::RateLimited { retry_after }));
        }
    }

    async fn query(
        &self,
        guild: Id<GuildMarker>,
        selection: Selection,
        timeout: Duration,
        presences: bool,
    ) -> QueryResult {
        let deadline = Instant::now()
            .checked_add(timeout)
            .filter(|_| !timeout.is_zero())
            .ok_or(MemberQueryError::InvalidTimeout)?;
        if tokio::runtime::Handle::try_current().is_err() {
            return Err(MemberQueryError::TimerUnavailable);
        }
        let timer = catch_unwind(|| sleep_until(deadline))
            .map_err(|_| MemberQueryError::TimerUnavailable)?;
        let (_registration, reply) = self.enqueue(guild, selection, deadline, presences)?;
        tokio::select! {
            biased;
            _ = timer => Err(MemberQueryError::TimedOut),
            value = reply => value.unwrap_or(Err(MemberQueryError::Closed)),
        }
    }

    fn enqueue(
        &self,
        guild: Id<GuildMarker>,
        selection: Selection,
        deadline: Instant,
        presences: bool,
    ) -> Result<(Registration, oneshot::Receiver<QueryResult>), MemberQueryError> {
        let id = next_nonce().ok_or(MemberQueryError::Busy)?;
        let payload = selection.request(guild, id, presences)?;
        let full = selection.is_full();
        let limit = selection.limit();
        let reserved = limit * if presences { 2 } else { 1 };
        let (reply, receiver) = oneshot::channel();
        let expired;
        let permit = {
            let mut state = self.state.lock().map_err(|_| MemberQueryError::Closed)?;
            if state.closed {
                return Err(MemberQueryError::Closed);
            }
            let required = selection.intents(presences) - state.intents;
            if !required.is_empty() {
                return Err(MemberQueryError::MissingIntents { required });
            }
            let total = state
                .routes
                .keys()
                .next()
                .ok_or(MemberQueryError::NotReady)?
                .total();
            let number = ((guild.get() >> 22) % u64::from(total)) as u32;
            let shard = ShardId::new(number, total);
            expired = if full || state.is_full(shard, reserved) {
                let now = Instant::now();
                state
                    .pending
                    .extract_if(|_, pending| pending.deadline <= now)
                    .map(|(_, pending)| pending)
                    .collect()
            } else {
                Vec::new()
            };
            if full {
                state.check_full_query(guild, Instant::now())?;
            }
            let route = state
                .routes
                .get(&shard)
                .filter(|route| route.ready)
                .ok_or(MemberQueryError::NotReady)?;
            if state.is_full(shard, reserved) {
                return Err(MemberQueryError::Busy);
            }
            let permit = route
                .sender
                .clone()
                .try_reserve_owned()
                .map_err(|error| match error {
                    mpsc::error::TrySendError::Full(_) => MemberQueryError::Busy,
                    mpsc::error::TrySendError::Closed(_) => MemberQueryError::NotReady,
                })?;
            state.pending.insert(
                id,
                Pending {
                    shard,
                    guild,
                    limit,
                    reserved,
                    full,
                    remaining: limit,
                    ids: selection.into_ids(),
                    count: None,
                    chunks: BTreeMap::new(),
                    seen: HashMap::new(),
                    presence_ids: presences.then(HashSet::new),
                    reply,
                    deadline,
                    sent: false,
                },
            );
            permit
        };
        for pending in expired {
            let _ = pending.reply.send(Err(MemberQueryError::TimedOut));
        }
        permit.send(MemberQueryCommand { id, payload });
        let registration = Registration {
            state: Arc::clone(&self.state),
            id,
        };
        Ok((registration, receiver))
    }
}

#[derive(Debug)]
pub(crate) struct MemberQueryCommand {
    id: u64,
    payload: RequestGuildMembers,
}

fn next_nonce() -> Option<u64> {
    let mut id = NEXT_NONCE.load(Ordering::Relaxed);
    loop {
        let next = id.checked_add(1)?;
        match NEXT_NONCE.compare_exchange_weak(id, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Some(id),
            Err(current) => id = current,
        }
    }
}

fn parse_nonce(nonce: &str) -> Option<u64> {
    let id = nonce.strip_prefix(NONCE_PREFIX)?;
    if id.len() != 16
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    u64::from_str_radix(id, 16).ok()
}

enum Selection {
    Prefix(String, usize),
    Users(Vec<Id<UserMarker>>),
    All(usize),
}

impl Selection {
    fn limit(&self) -> usize {
        match self {
            Self::Prefix(_, limit) | Self::All(limit) => *limit,
            Self::Users(ids) => ids.len(),
        }
    }

    fn into_ids(self) -> Option<Vec<Id<UserMarker>>> {
        match self {
            Self::Users(ids) => Some(ids),
            Self::Prefix(_, _) | Self::All(_) => None,
        }
    }

    fn is_full(&self) -> bool {
        matches!(self, Self::All(_))
    }

    fn intents(&self, presences: bool) -> Intents {
        let mut intents = Intents::empty();
        intents.set(Intents::GUILD_MEMBERS, self.is_full());
        intents.set(Intents::GUILD_PRESENCES, presences);
        intents
    }

    fn request(
        &self,
        guild: Id<GuildMarker>,
        id: u64,
        presences: bool,
    ) -> Result<RequestGuildMembers, MemberQueryError> {
        let mut builder =
            RequestGuildMembers::builder(guild).nonce(format!("{NONCE_PREFIX}{id:016x}"));
        if presences {
            builder = builder.presences(true);
        }
        match self {
            Self::Prefix(prefix, limit) => Ok(builder.query(prefix.as_str(), Some(*limit as u64))),
            Self::Users(ids) => builder
                .user_ids(ids.clone())
                .map_err(|_| MemberQueryError::InvalidUserIds),
            Self::All(_) => Ok(builder.query("", None)),
        }
    }
}

#[derive(Debug)]
struct State {
    intents: Intents,
    routes: HashMap<ShardId, Route>,
    pending: HashMap<u64, Pending>,
    cooldowns: HashMap<Id<GuildMarker>, Cooldown>,
    closed: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            intents: Intents::empty(),
            routes: HashMap::new(),
            pending: HashMap::new(),
            cooldowns: HashMap::new(),
            closed: false,
        }
    }
}

impl State {
    fn is_full(&self, shard: ShardId, reserved: usize) -> bool {
        self.pending.len() >= MAX_PENDING
            || self
                .pending
                .values()
                .filter(|entry| entry.shard == shard)
                .count()
                >= MAX_PENDING_PER_SHARD
            || self
                .pending
                .values()
                .map(|pending| pending.reserved)
                .sum::<usize>()
                + reserved
                > MAX_RESERVED_ITEMS
    }

    fn check_full_query(
        &mut self,
        guild: Id<GuildMarker>,
        now: Instant,
    ) -> Result<(), MemberQueryError> {
        if let Some(cooldown) = self.cooldowns.get(&guild)
            && cooldown.until > now
        {
            return Err(MemberQueryError::RateLimited {
                retry_after: cooldown.until - now,
            });
        }
        if self
            .pending
            .values()
            .any(|pending| pending.full && pending.guild == guild)
        {
            return Err(MemberQueryError::Busy);
        }
        self.cooldowns.retain(|_, cooldown| {
            cooldown.until > now || self.pending.contains_key(&cooldown.command)
        });
        let unsent = self
            .pending
            .values()
            .filter(|pending| pending.full && !pending.sent)
            .count();
        if self.cooldowns.len() + unsent >= MAX_RATE_KEYS {
            return Err(MemberQueryError::Busy);
        }
        Ok(())
    }
}

#[derive(Debug)]
struct Cooldown {
    command: u64,
    until: Instant,
}

#[derive(Debug)]
struct Route {
    sender: mpsc::Sender<MemberQueryCommand>,
    ready: bool,
}

#[derive(Debug)]
struct Pending {
    shard: ShardId,
    guild: Id<GuildMarker>,
    limit: usize,
    reserved: usize,
    full: bool,
    remaining: usize,
    ids: Option<Vec<Id<UserMarker>>>,
    count: Option<u32>,
    chunks: BTreeMap<u32, MemberQueryResult>,
    seen: HashMap<Id<UserMarker>, bool>,
    presence_ids: Option<HashSet<Id<UserMarker>>>,
    reply: oneshot::Sender<QueryResult>,
    deadline: Instant,
    sent: bool,
}

impl Pending {
    fn receive(&mut self, chunk: MemberChunk) -> Result<bool, MemberQueryError> {
        let count = chunk.chunk_count;
        let max_chunks = if self.full {
            MAX_FULL_CHUNKS
        } else {
            MAX_CHUNKS
        };
        if count == 0
            || count > max_chunks
            || chunk.chunk_index >= count
            || self.count.is_some_and(|expected| expected != count)
        {
            return Err(MemberQueryError::MalformedChunk);
        }
        if self.chunks.contains_key(&chunk.chunk_index) {
            return Ok(false);
        }
        if chunk.members.len() > MAX_CHUNK_MEMBERS || !self.matches_requested(&chunk) {
            return Err(MemberQueryError::MalformedChunk);
        }
        let length = chunk.members.len().saturating_add(chunk.not_found.len());
        if length > self.remaining {
            return Err(if self.full {
                MemberQueryError::LimitExceeded { limit: self.limit }
            } else {
                MemberQueryError::MalformedChunk
            });
        }
        if !chunk
            .members
            .iter()
            .map(|member| (member.user.id, true))
            .chain(chunk.not_found.iter().map(|&id| (id, false)))
            .all(|(id, member)| self.seen.insert(id, member).is_none())
        {
            return Err(MemberQueryError::MalformedChunk);
        }
        self.validate_presences(&chunk)?;
        let presences = if self.presence_ids.is_some() {
            chunk.presences
        } else {
            Vec::new()
        };
        self.remaining -= length;
        self.count = Some(count);
        self.chunks.insert(
            chunk.chunk_index,
            MemberQueryResult {
                members: chunk.members,
                not_found: chunk.not_found,
                presences,
            },
        );
        let completed = self.chunks.len() == count as usize;
        if completed
            && self
                .presence_ids
                .as_ref()
                .is_some_and(|ids| ids.iter().any(|id| self.seen.get(id) != Some(&true)))
        {
            return Err(MemberQueryError::MalformedChunk);
        }
        Ok(completed)
    }

    fn matches_requested(&self, chunk: &MemberChunk) -> bool {
        let Some(ids) = self.ids.as_ref() else {
            return chunk.not_found.is_empty();
        };
        chunk
            .members
            .iter()
            .all(|member| ids.contains(&member.user.id))
            && chunk.not_found.iter().all(|id| ids.contains(id))
    }

    fn validate_presences(&mut self, chunk: &MemberChunk) -> Result<(), MemberQueryError> {
        let Some(seen) = self.presence_ids.as_mut() else {
            return Ok(());
        };
        if chunk.presences.len() > MAX_CHUNK_MEMBERS
            || seen.len().saturating_add(chunk.presences.len()) > self.limit
            || !chunk
                .presences
                .iter()
                .all(|presence| presence.guild_id == self.guild && seen.insert(presence.user.id()))
        {
            return Err(MemberQueryError::MalformedChunk);
        }
        Ok(())
    }

    fn complete(self, status: Result<bool, MemberQueryError>) {
        let result = status.map(|_| {
            let mut chunks = self.chunks.into_values();
            let mut result = chunks.next().unwrap_or(MemberQueryResult {
                members: Vec::new(),
                not_found: Vec::new(),
                presences: Vec::new(),
            });
            if self.count.unwrap_or_default() > 1 {
                let received = self.limit - self.remaining;
                result
                    .members
                    .reserve(received.saturating_sub(result.members.len()));
                let presences = self.presence_ids.as_ref().map_or(0, HashSet::len);
                result
                    .presences
                    .reserve(presences.saturating_sub(result.presences.len()));
            }
            for chunk in chunks {
                result.members.extend(chunk.members);
                result.not_found.extend(chunk.not_found);
                result.presences.extend(chunk.presences);
            }
            result
        });
        let _ = self.reply.send(result);
    }
}

struct Registration {
    state: Arc<Mutex<State>>,
    id: u64,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let pending = self
            .state
            .lock()
            .ok()
            .and_then(|mut state| state.pending.remove(&self.id));
        drop(pending);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const GUILD: Id<GuildMarker> = Id::new(20);
    const TIMEOUT: Duration = Duration::from_secs(5);

    fn member(id: u64) -> Member {
        serde_json::from_value(json!({
            "user":{"id":id.to_string(),"username":"test","discriminator":"0"},
            "joined_at":"2026-01-01T00:00:00.000000+00:00",
            "roles":[], "deaf":false, "mute":false, "flags":0
        }))
        .unwrap()
    }

    fn chunk(
        guild: Id<GuildMarker>,
        nonce: Option<String>,
        count: u32,
        index: u32,
        ids: &[u64],
    ) -> MemberChunk {
        MemberChunk {
            guild_id: guild,
            nonce,
            chunk_count: count,
            chunk_index: index,
            members: ids.iter().copied().map(member).collect(),
            not_found: Vec::new(),
            presences: Vec::new(),
        }
    }

    fn active() -> (MemberQuery, mpsc::Receiver<MemberQueryCommand>) {
        let query = MemberQuery::new();
        let receiver = query.initialize([ShardId::ONE]).into_iter().next().unwrap();
        query.ready(ShardId::ONE);
        (query, receiver)
    }

    fn presence(guild: Id<GuildMarker>, user: u64) -> Presence {
        serde_json::from_value(json!({
            "guild_id":guild.get().to_string(), "user":{"id":user.to_string()},
            "client_status":{}, "status":"idle"
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn all_members_and_presences_need_explicit_intents_before_queueing() {
        let (query, mut gateway) = active();
        assert_eq!(
            query.query_all(GUILD, 10, TIMEOUT).await.unwrap_err(),
            MemberQueryError::MissingIntents {
                required: Intents::GUILD_MEMBERS
            }
        );
        assert_eq!(
            query
                .query_all_with_presences(GUILD, 10, TIMEOUT)
                .await
                .unwrap_err(),
            MemberQueryError::MissingIntents {
                required: Intents::GUILD_MEMBERS | Intents::GUILD_PRESENCES
            }
        );
        query.configure_intents(Intents::GUILD_MEMBERS);
        assert_eq!(
            query
                .query_ids_with_presences(GUILD, &[Id::new(3)], TIMEOUT)
                .await
                .unwrap_err(),
            MemberQueryError::MissingIntents {
                required: Intents::GUILD_PRESENCES
            }
        );
        assert_eq!(
            query
                .query_prefix_with_presences(GUILD, "user", 1, TIMEOUT)
                .await
                .unwrap_err(),
            MemberQueryError::MissingIntents {
                required: Intents::GUILD_PRESENCES
            }
        );
        assert!(matches!(
            gateway.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn full_query_has_zero_wire_limit_and_waits_for_all_chunks_and_presences() {
        let (query, mut gateway) = active();
        query.configure_intents(Intents::GUILD_MEMBERS | Intents::GUILD_PRESENCES);
        let task = tokio::spawn({
            let query = query.clone();
            async move { query.query_all_with_presences(GUILD, 3, TIMEOUT).await }
        });
        let command = gateway.recv().await.unwrap();
        let request = query.request(ShardId::ONE, &command).unwrap();
        assert_eq!(request.d.query.as_deref(), Some(""));
        assert_eq!(request.d.limit, Some(0));
        assert_eq!(request.d.presences, Some(true));

        let mut last = chunk(GUILD, request.d.nonce.clone(), 2, 1, &[2]);
        last.presences.push(presence(GUILD, 1));
        query.observe_chunk(ShardId::ONE, last);
        assert!(!task.is_finished());
        query.observe_chunk(
            ShardId::ONE,
            chunk(GUILD, request.d.nonce.clone(), 2, 0, &[1]),
        );
        let (members, missing, presences) = task.await.unwrap().unwrap().into_parts();
        assert_eq!(
            members
                .iter()
                .map(|member| member.user.id)
                .collect::<Vec<_>>(),
            [Id::new(1), Id::new(2)]
        );
        assert!(missing.is_empty());
        assert_eq!(presences.first().unwrap().user.id(), Id::new(1));
    }

    #[tokio::test]
    async fn full_query_fails_instead_of_returning_members_at_the_cap() {
        let (query, mut gateway) = active();
        query.configure_intents(Intents::GUILD_MEMBERS);
        let task = tokio::spawn({
            let query = query.clone();
            async move { query.query_all(GUILD, 1, TIMEOUT).await }
        });
        let command = gateway.recv().await.unwrap();
        let request = query.request(ShardId::ONE, &command).unwrap();
        query.observe_chunk(
            ShardId::ONE,
            chunk(GUILD, request.d.nonce.clone(), 2, 0, &[1]),
        );
        assert!(!task.is_finished());
        query.observe_chunk(
            ShardId::ONE,
            chunk(GUILD, request.d.nonce.clone(), 2, 1, &[2]),
        );
        assert_eq!(
            task.await.unwrap().unwrap_err(),
            MemberQueryError::LimitExceeded { limit: 1 }
        );
        assert!(query.state.lock().unwrap().pending.is_empty());
    }

    #[tokio::test]
    async fn concurrent_full_query_reservations_respect_the_shared_entry_budget() {
        let (query, mut gateway) = active();
        query.configure_intents(Intents::GUILD_MEMBERS | Intents::GUILD_PRESENCES);
        let deadline = Instant::now() + TIMEOUT;
        let (guard, _result) = query
            .enqueue(GUILD, Selection::All(MAX_FULL_MEMBERS), deadline, true)
            .unwrap();
        let blocked = query.enqueue(
            Id::new(90),
            Selection::Prefix("new".into(), 1),
            deadline,
            false,
        );
        assert!(matches!(blocked, Err(MemberQueryError::Busy)));
        drop(guard);
        let cancelled = gateway.recv().await.unwrap();
        assert!(query.request(ShardId::ONE, &cancelled).is_none());
        assert!(
            query
                .enqueue(
                    Id::new(90),
                    Selection::Prefix("new".into(), 1),
                    deadline,
                    false
                )
                .is_ok()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn full_queries_use_per_guild_cooldowns_and_honor_matching_discord_rate_limits() {
        let (query, mut gateway) = active();
        query.configure_intents(Intents::GUILD_MEMBERS);
        let task = tokio::spawn({
            let query = query.clone();
            async move { query.query_all(GUILD, 10, TIMEOUT).await }
        });
        let command = gateway.recv().await.unwrap();
        let request = query.request(ShardId::ONE, &command).unwrap();
        let nonce = request.d.nonce.as_deref().unwrap();
        assert_eq!(
            query.query_all(GUILD, 10, TIMEOUT).await.unwrap_err(),
            MemberQueryError::RateLimited {
                retry_after: FULL_QUERY_INTERVAL
            }
        );
        let retry_after = Duration::from_secs(40);
        query.observe_rate_limit(ShardId::ONE, Id::new(90), nonce, retry_after);
        query.observe_rate_limit(ShardId::new(1, 2), GUILD, nonce, retry_after);
        assert!(!task.is_finished());
        let other = query
            .enqueue(
                Id::new(90),
                Selection::All(10),
                Instant::now() + TIMEOUT,
                false,
            )
            .unwrap();
        drop(other);

        query.observe_rate_limit(ShardId::ONE, GUILD, nonce, retry_after);
        assert_eq!(
            task.await.unwrap().unwrap_err(),
            MemberQueryError::RateLimited { retry_after }
        );
        assert_eq!(
            query.query_all(GUILD, 10, TIMEOUT).await.unwrap_err(),
            MemberQueryError::RateLimited { retry_after }
        );
        tokio::time::advance(retry_after).await;
        assert!(
            query
                .enqueue(GUILD, Selection::All(10), Instant::now() + TIMEOUT, false)
                .is_ok()
        );
    }

    #[tokio::test]
    async fn presences_with_unreturned_user_ids_reject_the_whole_result() {
        let (query, mut gateway) = active();
        query.configure_intents(Intents::GUILD_PRESENCES);
        let task = tokio::spawn({
            let query = query.clone();
            async move {
                query
                    .query_ids_with_presences(GUILD, &[Id::new(1), Id::new(2)], TIMEOUT)
                    .await
            }
        });
        let command = gateway.recv().await.unwrap();
        let request = query.request(ShardId::ONE, &command).unwrap();
        assert_eq!(request.d.presences, Some(true));
        let mut response = chunk(GUILD, request.d.nonce.clone(), 1, 0, &[1]);
        response.not_found = vec![Id::new(2)];
        response.presences.push(presence(GUILD, 2));
        query.observe_chunk(ShardId::ONE, response);
        assert_eq!(
            task.await.unwrap().unwrap_err(),
            MemberQueryError::MalformedChunk
        );
    }

    #[tokio::test]
    async fn prefix_query_merges_chunks_once_in_index_order() {
        let (query, mut gateway) = active();
        let task = tokio::spawn({
            let query = query.clone();
            async move { query.query_prefix(GUILD, "名前", 2, TIMEOUT).await }
        });
        let command = gateway.recv().await.unwrap();
        let nonce = command.payload.d.nonce.clone();
        let wrong_shard = ShardId::new(1, 2);
        assert!(nonce.as_ref().is_some_and(|nonce| nonce.len() <= 32));
        assert!(query.request(wrong_shard, &command).is_none());
        // 送信前、別のシャードやサーバー、別の nonce の応答は結果に含まれない。
        query.observe_chunk(ShardId::ONE, chunk(GUILD, nonce.clone(), 1, 0, &[99]));
        let request = query.request(ShardId::ONE, &command).unwrap();
        assert_eq!(request.d.query.as_deref(), Some("名前"));
        assert_eq!(request.d.limit, Some(2));
        assert!(request.d.presences.is_none());
        assert!(query.request(ShardId::ONE, &command).is_none());
        query.observe_chunk(wrong_shard, chunk(GUILD, nonce.clone(), 1, 0, &[99]));
        query.observe_chunk(ShardId::ONE, chunk(Id::new(30), nonce.clone(), 1, 0, &[99]));
        query.observe_chunk(ShardId::ONE, chunk(GUILD, None, 1, 0, &[99]));
        query.observe_chunk(ShardId::ONE, chunk(GUILD, nonce.clone(), 2, 1, &[2]));
        query.observe_chunk(ShardId::ONE, chunk(GUILD, nonce.clone(), 2, 1, &[99]));
        query.observe_chunk(ShardId::ONE, chunk(GUILD, nonce, 2, 0, &[1]));
        let result = task.await.unwrap().unwrap();
        let ids: Vec<_> = result
            .members()
            .iter()
            .map(|member| member.user.id)
            .collect();
        assert_eq!(ids, [Id::new(1), Id::new(2)]);
        assert!(result.not_found().is_empty());
    }

    #[tokio::test]
    async fn explicit_ids_keeps_reported_missing_users_and_rejects_unrequested_members() {
        let (query, mut gateway) = active();
        for accepted in [true, false] {
            let task = tokio::spawn({
                let query = query.clone();
                async move {
                    query
                        .query_ids(GUILD, &[Id::new(1), Id::new(2)], TIMEOUT)
                        .await
                }
            });
            let command = gateway.recv().await.unwrap();
            let request = query.request(ShardId::ONE, &command).unwrap();
            assert!(request.d.user_ids.is_some());
            assert!(request.d.query.is_none());
            let id = if accepted { 1 } else { 3 };
            let mut response = chunk(GUILD, request.d.nonce.clone(), 1, 0, &[id]);
            response.not_found = vec![Id::new(2)];
            query.observe_chunk(ShardId::ONE, response);
            let result = task.await.unwrap();
            if accepted {
                let result = result.unwrap();
                assert_eq!(result.not_found(), [Id::new(2)]);
                assert_eq!(result.into_members().first().unwrap().user.id, Id::new(1));
            } else {
                assert_eq!(result.unwrap_err(), MemberQueryError::MalformedChunk);
            }
        }
    }

    #[tokio::test]
    async fn invalid_chunk_counts_and_oversized_results_finish_without_retaining_partial_data() {
        let (query, mut gateway) = active();
        for count in [0, MAX_CHUNKS + 1, 1] {
            let task = tokio::spawn({
                let query = query.clone();
                async move { query.query_prefix(GUILD, "a", 1, TIMEOUT).await }
            });
            let command = gateway.recv().await.unwrap();
            let request = query.request(ShardId::ONE, &command).unwrap();
            query.observe_chunk(
                ShardId::ONE,
                chunk(GUILD, request.d.nonce.clone(), count, 0, &[1, 2]),
            );
            assert_eq!(
                task.await.unwrap().unwrap_err(),
                MemberQueryError::MalformedChunk
            );
        }
        assert!(query.state.lock().unwrap().pending.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_and_dropped_futures_cannot_send_abandoned_commands() {
        let (query, mut gateway) = active();
        let expired = tokio::spawn({
            let query = query.clone();
            async move { query.query_prefix(GUILD, "a", 5, TIMEOUT).await }
        });
        let command = gateway.recv().await.unwrap();
        tokio::time::advance(TIMEOUT).await;
        assert_eq!(
            expired.await.unwrap().unwrap_err(),
            MemberQueryError::TimedOut
        );
        assert!(query.request(ShardId::ONE, &command).is_none());
        let cancelled = tokio::spawn({
            let query = query.clone();
            async move { query.query_prefix(GUILD, "b", 5, TIMEOUT).await }
        });
        let command = gateway.recv().await.unwrap();
        cancelled.abort();
        assert!(cancelled.await.unwrap_err().is_cancelled());
        assert!(query.request(ShardId::ONE, &command).is_none());
        assert!(query.state.lock().unwrap().pending.is_empty());
    }

    #[tokio::test]
    async fn requests_follow_guild_shards_and_close_only_the_affected_shard() {
        let first = ShardId::new(0, 2);
        let second = ShardId::new(1, 2);
        let other_guild = Id::new((1 << 22) + 1);
        let query = MemberQuery::new();
        let mut gateways = query.initialize([first, second]).into_iter();
        let mut first_gateway = gateways.next().unwrap();
        let mut second_gateway = gateways.next().unwrap();
        query.ready(first);
        assert_eq!(
            query
                .query_prefix(other_guild, "a", 5, TIMEOUT)
                .await
                .unwrap_err(),
            MemberQueryError::NotReady
        );
        query.ready(second);
        let first_task = tokio::spawn({
            let query = query.clone();
            async move { query.query_prefix(GUILD, "a", 5, TIMEOUT).await }
        });
        let second_task = tokio::spawn({
            let query = query.clone();
            async move { query.query_prefix(other_guild, "a", 5, TIMEOUT).await }
        });
        let first_command = first_gateway.recv().await.unwrap();
        let second_command = second_gateway.recv().await.unwrap();
        assert_ne!(
            first_command.payload.d.nonce,
            second_command.payload.d.nonce
        );
        query.disconnect(first);
        assert_eq!(
            first_task.await.unwrap().unwrap_err(),
            MemberQueryError::Disconnected
        );
        assert!(query.request(first, &first_command).is_none());
        assert!(query.request(second, &second_command).is_some());
        query.close();
        assert_eq!(
            second_task.await.unwrap().unwrap_err(),
            MemberQueryError::Closed
        );
    }

    #[tokio::test]
    async fn pending_and_mailbox_limits_are_bounded_and_send_failure_settles_the_request() {
        let (query, mut gateway) = active();
        let deadline = Instant::now() + TIMEOUT;
        let mut active = Vec::new();
        for _ in 0..MAX_PENDING_PER_SHARD {
            active.push(
                query
                    .enqueue(GUILD, Selection::Prefix("a".into(), 1), deadline, false)
                    .unwrap(),
            );
        }
        let extra = query.enqueue(GUILD, Selection::Prefix("b".into(), 1), deadline, false);
        assert!(matches!(extra, Err(MemberQueryError::Busy)));
        let command = gateway.recv().await.unwrap();
        assert!(query.request(ShardId::ONE, &command).is_some());
        query.send_failed(ShardId::ONE, &command);
        let (_, failed) = active.remove(0);
        assert_eq!(
            failed.await.unwrap().unwrap_err(),
            MemberQueryError::SendFailed
        );
        assert!(
            query
                .enqueue(GUILD, Selection::Prefix("c".into(), 1), deadline, false)
                .is_ok()
        );
    }
}
