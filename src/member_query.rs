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
        ShardId,
        payload::{incoming::MemberChunk, outgoing::RequestGuildMembers},
    },
    guild::Member,
    id::{
        Id,
        marker::{GuildMarker, UserMarker},
    },
};

const MAX_MEMBERS: usize = 100;
const MAX_PREFIX_CHARS: usize = 32;
const MAX_CHUNKS: u32 = 16;
const MAX_PENDING: usize = 64;
const MAX_PENDING_PER_SHARD: usize = 16;
const NONCE_PREFIX: &str = "akari-";
static NEXT_NONCE: AtomicU64 = AtomicU64::new(1);

/// Gateway に対象を指定してサーバーのメンバーを問い合わせるハンドルです。
///
/// ユーザー名の先頭一致と、ユーザー ID を指定した問い合わせに対応します。
/// どちらも最大 100 件です。全メンバーの取得やプレゼンスの要求は行いません。
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

    /// 問い合わせ結果からメンバーの一覧を取り出します。
    pub fn into_members(self) -> Vec<Member> {
        self.members
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
        if prefix.chars().take(MAX_PREFIX_CHARS + 1).count() > MAX_PREFIX_CHARS
            || prefix.trim().is_empty()
        {
            return Err(MemberQueryError::InvalidPrefix);
        }
        if !(1..=MAX_MEMBERS).contains(&limit) {
            return Err(MemberQueryError::InvalidLimit);
        }
        self.query(guild, Selection::Prefix(prefix.to_owned(), limit), timeout)
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
        if !(1..=MAX_MEMBERS).contains(&users.len())
            || users.iter().copied().collect::<HashSet<_>>().len() != users.len()
        {
            return Err(MemberQueryError::InvalidUserIds);
        }
        self.query(guild, Selection::Users(users.to_vec()), timeout)
            .await
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
        if pending.shard != shard || pending.deadline <= Instant::now() || pending.sent {
            return None;
        }
        pending.sent = true;
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
            state.pending.remove(&command.id)
        };
        if let Some(pending) = pending {
            let _ = pending.reply.send(Err(MemberQueryError::SendFailed));
        }
    }

    pub(crate) fn observe_chunk(&self, shard: ShardId, chunk: &MemberChunk) {
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

    async fn query(
        &self,
        guild: Id<GuildMarker>,
        selection: Selection,
        timeout: Duration,
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
        let (_registration, reply) = self.enqueue(guild, selection, deadline)?;
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
    ) -> Result<(Registration, oneshot::Receiver<QueryResult>), MemberQueryError> {
        let id = next_nonce().ok_or(MemberQueryError::Busy)?;
        let payload = selection.request(guild, id)?;
        let (reply, receiver) = oneshot::channel();
        let expired;
        let permit = {
            let mut state = self.state.lock().map_err(|_| MemberQueryError::Closed)?;
            if state.closed {
                return Err(MemberQueryError::Closed);
            }
            let total = state
                .routes
                .keys()
                .next()
                .ok_or(MemberQueryError::NotReady)?
                .total();
            let number = ((guild.get() >> 22) % u64::from(total)) as u32;
            let shard = ShardId::new(number, total);
            expired = if state.is_full(shard) {
                let now = Instant::now();
                state
                    .pending
                    .extract_if(|_, pending| pending.deadline <= now)
                    .map(|(_, pending)| pending)
                    .collect()
            } else {
                Vec::new()
            };
            let route = state
                .routes
                .get(&shard)
                .filter(|route| route.ready)
                .ok_or(MemberQueryError::NotReady)?;
            if state.is_full(shard) {
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
                    remaining: selection.limit(),
                    ids: selection.into_ids(),
                    count: None,
                    chunks: BTreeMap::new(),
                    seen: HashSet::new(),
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
}

impl Selection {
    fn limit(&self) -> usize {
        match self {
            Self::Prefix(_, limit) => *limit,
            Self::Users(ids) => ids.len(),
        }
    }

    fn into_ids(self) -> Option<Vec<Id<UserMarker>>> {
        match self {
            Self::Users(ids) => Some(ids),
            Self::Prefix(_, _) => None,
        }
    }

    fn request(
        &self,
        guild: Id<GuildMarker>,
        id: u64,
    ) -> Result<RequestGuildMembers, MemberQueryError> {
        let builder = RequestGuildMembers::builder(guild).nonce(format!("{NONCE_PREFIX}{id:016x}"));
        match self {
            Self::Prefix(prefix, limit) => Ok(builder.query(prefix.as_str(), Some(*limit as u64))),
            Self::Users(ids) => builder
                .user_ids(ids.clone())
                .map_err(|_| MemberQueryError::InvalidUserIds),
        }
    }
}

#[derive(Debug, Default)]
struct State {
    routes: HashMap<ShardId, Route>,
    pending: HashMap<u64, Pending>,
    closed: bool,
}

impl State {
    fn is_full(&self, shard: ShardId) -> bool {
        self.pending.len() >= MAX_PENDING
            || self
                .pending
                .values()
                .filter(|entry| entry.shard == shard)
                .count()
                >= MAX_PENDING_PER_SHARD
    }
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
    remaining: usize,
    ids: Option<Vec<Id<UserMarker>>>,
    count: Option<u32>,
    chunks: BTreeMap<u32, MemberQueryResult>,
    seen: HashSet<Id<UserMarker>>,
    reply: oneshot::Sender<QueryResult>,
    deadline: Instant,
    sent: bool,
}

impl Pending {
    fn receive(&mut self, chunk: &MemberChunk) -> Result<bool, MemberQueryError> {
        let count = chunk.chunk_count;
        if count == 0
            || count > MAX_CHUNKS
            || chunk.chunk_index >= count
            || self.count.is_some_and(|expected| expected != count)
        {
            return Err(MemberQueryError::MalformedChunk);
        }
        if self.chunks.contains_key(&chunk.chunk_index) {
            return Ok(false);
        }
        let length = chunk.members.len().saturating_add(chunk.not_found.len());
        if length > self.remaining || !self.matches_requested(chunk) {
            return Err(MemberQueryError::MalformedChunk);
        }
        if !chunk
            .members
            .iter()
            .map(|member| member.user.id)
            .chain(chunk.not_found.iter().copied())
            .all(|id| self.seen.insert(id))
        {
            return Err(MemberQueryError::MalformedChunk);
        }
        self.remaining -= length;
        self.count = Some(count);
        self.chunks.insert(
            chunk.chunk_index,
            MemberQueryResult {
                members: chunk.members.clone(),
                not_found: chunk.not_found.clone(),
            },
        );
        Ok(self.chunks.len() == count as usize)
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

    fn complete(self, status: Result<bool, MemberQueryError>) {
        let result = status.map(|_| {
            let mut result = MemberQueryResult {
                members: Vec::new(),
                not_found: Vec::new(),
            };
            for chunk in self.chunks.into_values() {
                result.members.extend(chunk.members);
                result.not_found.extend(chunk.not_found);
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
        query.observe_chunk(ShardId::ONE, &chunk(GUILD, nonce.clone(), 1, 0, &[99]));
        let request = query.request(ShardId::ONE, &command).unwrap();
        assert_eq!(request.d.query.as_deref(), Some("名前"));
        assert_eq!(request.d.limit, Some(2));
        assert!(request.d.presences.is_none());
        assert!(query.request(ShardId::ONE, &command).is_none());
        query.observe_chunk(wrong_shard, &chunk(GUILD, nonce.clone(), 1, 0, &[99]));
        query.observe_chunk(
            ShardId::ONE,
            &chunk(Id::new(30), nonce.clone(), 1, 0, &[99]),
        );
        query.observe_chunk(ShardId::ONE, &chunk(GUILD, None, 1, 0, &[99]));
        query.observe_chunk(ShardId::ONE, &chunk(GUILD, nonce.clone(), 2, 1, &[2]));
        query.observe_chunk(ShardId::ONE, &chunk(GUILD, nonce.clone(), 2, 1, &[99]));
        query.observe_chunk(ShardId::ONE, &chunk(GUILD, nonce, 2, 0, &[1]));
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
            query.observe_chunk(ShardId::ONE, &response);
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
                &chunk(GUILD, request.d.nonce.clone(), count, 0, &[1, 2]),
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
                    .enqueue(GUILD, Selection::Prefix("a".into(), 1), deadline)
                    .unwrap(),
            );
        }
        let extra = query.enqueue(GUILD, Selection::Prefix("b".into(), 1), deadline);
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
                .enqueue(GUILD, Selection::Prefix("c".into(), 1), deadline)
                .is_ok()
        );
    }
}
