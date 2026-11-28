use std::{
    collections::{HashMap, HashSet},
    fmt,
    panic::catch_unwind,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::{
    runtime::Handle,
    task::AbortHandle,
    time::{Instant, sleep_until},
};
use twilight_model::{
    channel::{Message, message::MessageFlags},
    id::{
        Id,
        marker::{ChannelMarker, GuildMarker, InteractionMarker, MessageMarker, UserMarker},
    },
};

use crate::{
    ButtonContext, Error, Events,
    ui::{Button, ComponentsV2, DynamicIdRef, UiMessage},
};

const NAMESPACE: &str = "akari-pager";

/// 同時に開けるページ送りと、1件あたりのページ数の上限です。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PagerLimits {
    /// Bot 全体で同時に開ける件数です。
    pub max_pending: usize,
    /// 同じユーザーについて同時に開ける件数です。
    pub per_user: usize,
    /// 一つのページ送りに指定できるページ数です。
    pub max_pages: usize,
}

impl Default for PagerLimits {
    fn default() -> Self {
        Self {
            max_pending: 128,
            per_user: 4,
            max_pages: 32,
        }
    }
}

/// ページ送りの準備や元メッセージの指定に失敗した理由です。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PagerError {
    /// 件数の上限が無効です。
    #[error("ページ送りの件数の上限が無効です")]
    InvalidLimits,
    /// 制限時間がゼロか、終了時刻を表現できません。
    #[error("ページ送りの制限時間が無効です")]
    InvalidTimeout,
    /// Tokio のタイマーを利用できません。
    #[error("ページ送りに必要な Tokio のタイマーを利用できません")]
    TimerUnavailable,
    /// 表示するページが一つもありません。
    #[error("表示するページを指定してください")]
    NoPages,
    /// 指定したページ数が上限を超えています。
    #[error("ページ送りに指定できるページ数は {max} 件までです")]
    TooManyPages {
        /// 設定により許可された最大ページ数です。
        max: usize,
    },
    /// 一つのページ送りに従来形式と Components V2 が混在しています。
    #[error("すべてのページを従来形式か Components V2 のどちらかに揃えてください")]
    MixedFormats,
    /// ページの内容と追加した操作行を組み合わせると、Discord の制約に合いません。
    #[error("{page} ページ目を表示できません")]
    InvalidPage {
        /// 最初を1としたページ番号です。
        page: usize,
        /// 検証に失敗した理由です。
        #[source]
        source: Box<Error>,
    },
    /// Bot 全体で開ける件数の上限に達しました。
    #[error("ページ送りの同時表示枠がいっぱいです")]
    Capacity,
    /// このユーザーについて開ける件数の上限に達しました。
    #[error("このユーザーのページ送りの同時表示枠がいっぱいです")]
    UserCapacity,
    /// 元メッセージが、指定したサーバーやページの形式と一致しません。
    #[error("ページ送りの元メッセージのサーバーまたは形式が一致しません")]
    InvalidSource,
    /// 同じメッセージを扱うページ送りが、すでに開いているか更新処理中です。
    #[error("このメッセージには、すでにページ送りが紐づいています")]
    SourceInUse,
    /// 期限切れや停止のため、ページ送りを利用できません。
    #[error("このページ送りは終了しています")]
    Ended,
}

/// ページ送りに表示する内容です。すべて同じ形式を選びます。
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum PagerPage {
    /// 本文、Embed、従来のコンポーネントを含むページです。
    Classic(UiMessage),
    /// Components V2 のページです。この機能ではファイルのアップロードを伴いません。
    V2(ComponentsV2),
}

impl From<UiMessage> for PagerPage {
    fn from(message: UiMessage) -> Self {
        Self::Classic(message)
    }
}

impl From<ComponentsV2> for PagerPage {
    fn from(message: ComponentsV2) -> Self {
        Self::V2(message)
    }
}

impl PagerPage {
    fn is_v2(&self) -> bool {
        matches!(self, Self::V2(_))
    }

    fn render(self, buttons: Vec<Button>) -> crate::Result<Self> {
        match self {
            Self::Classic(page) => {
                let page = page.buttons(buttons);
                page.validate()?;
                Ok(Self::Classic(page))
            }
            Self::V2(page) => {
                if !page.attachments.is_empty() {
                    return Err(Error::InvalidUi(
                        "ページ送りにはアップロード用の添付ファイルを指定できません。",
                    ));
                }
                let page = page.buttons(buttons);
                page.validate()?;
                page.validate_attachment_references(&[])?;
                Ok(Self::V2(page))
            }
        }
    }

    async fn update(&self, ctx: &mut ButtonContext<'_>) -> crate::Result<()> {
        match self {
            Self::Classic(page) => ctx.update_message(page).await,
            Self::V2(page) => ctx.update_v2(page).await,
        }
    }
}

/// 本人だけが操作できる、期限付きページ送りの管理ハンドルです。
///
/// [`Self::register`] でハンドラーを登録し、表示するたびに [`Self::begin`] を呼びます。
/// クリックの応答を待つために、コマンドのハンドラーを実行し続ける必要はありません。
#[derive(Clone)]
pub struct Pagers {
    shared: Arc<Shared>,
}

impl fmt::Debug for Pagers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pagers")
            .field("limits", &self.shared.limits)
            .finish_non_exhaustive()
    }
}

impl Pagers {
    /// 上限を指定して、ページ送りを管理するハンドルを作ります。
    ///
    /// # エラー
    /// 上限がゼロ、またはユーザーごとの上限が全体の上限より大きい場合に失敗します。
    pub fn new(limits: PagerLimits) -> Result<Self, PagerError> {
        if limits.max_pending == 0
            || limits.per_user == 0
            || limits.max_pages == 0
            || limits.per_user > limits.max_pending
        {
            return Err(PagerError::InvalidLimits);
        }
        Ok(Self {
            shared: Arc::new(Shared {
                limits,
                state: Mutex::default(),
            }),
        })
    }

    /// ページ送りのボタンを扱うハンドラーを登録します。
    /// `akari-pager` はこの機能が使う名前空間です。
    ///
    /// # エラー
    /// 同じ名前空間にボタンのハンドラーが登録済みの場合に失敗します。
    pub fn register(&self, events: &mut Events) -> crate::Result<()> {
        let pagers = self.clone();
        events.on_dynamic_button(NAMESPACE, async move |ctx| pagers.dispatch(ctx).await)
    }

    /// 起点の Interaction と操作を許可する本人を指定して、ページ送りを準備します。
    /// 先頭ページで返信し、送信後に [`PagerDraft::bind`] でメッセージを指定します。
    ///
    /// # エラー
    /// ページ、制限時間、または件数が無効な場合や、Tokio のタイマーを利用できない場合に失敗します。
    pub fn begin(
        &self,
        trigger: Id<InteractionMarker>,
        owner: Id<UserMarker>,
        timeout: Duration,
        pages: Vec<PagerPage>,
    ) -> Result<PagerDraft, PagerError> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .filter(|_| !timeout.is_zero())
            .ok_or(PagerError::InvalidTimeout)?;
        let Some(first) = pages.first() else {
            return Err(PagerError::NoPages);
        };
        if pages.len() > self.shared.limits.max_pages {
            return Err(PagerError::TooManyPages {
                max: self.shared.limits.max_pages,
            });
        }
        if pages.iter().any(|page| first.is_v2() != page.is_v2()) {
            return Err(PagerError::MixedFormats);
        }
        let runtime = Handle::try_current().map_err(|_| PagerError::TimerUnavailable)?;
        let timer =
            catch_unwind(|| sleep_until(deadline)).map_err(|_| PagerError::TimerUnavailable)?;
        let key = self.allocate(trigger, owner)?;
        let rendered = render_pages(key, pages)?;
        let first = Arc::clone(rendered.first().ok_or(PagerError::NoPages)?);
        let weak = Arc::downgrade(&self.shared);
        {
            let mut state = self.shared.state.lock().map_err(|_| PagerError::Ended)?;
            if deadline <= Instant::now() {
                return Err(PagerError::Ended);
            }
            state.check_capacity(owner, self.shared.limits)?;
            let task = runtime.spawn(async move {
                timer.await;
                if let Some(shared) = weak.upgrade() {
                    shared.remove(key);
                }
            });
            state.insert(
                key,
                Entry {
                    owner,
                    deadline,
                    source: None,
                    pages: rendered,
                    current: 0,
                    timer: task.abort_handle(),
                },
            );
        }
        Ok(PagerDraft {
            first,
            pager: Some(Pager {
                registry: self.clone(),
                key,
            }),
        })
    }

    fn allocate(
        &self,
        trigger: Id<InteractionMarker>,
        owner: Id<UserMarker>,
    ) -> Result<Key, PagerError> {
        let (expired, key) = {
            let mut state = self.shared.state.lock().map_err(|_| PagerError::Ended)?;
            let expired = state.take_expired(Instant::now());
            let key = state.next_key(trigger, owner, self.shared.limits);
            (expired, key)
        };
        for entry in expired {
            entry.timer.abort();
        }
        key
    }

    async fn dispatch(&self, ctx: &mut ButtonContext<'_>) -> crate::Result<()> {
        let candidate = parse_click(
            ctx.dynamic_id(),
            ctx.author_id(),
            ctx.guild_id(),
            ctx.message(),
        );
        let claim = candidate.map_or(Claim::Unavailable, |click| self.claim(click));
        let update = match claim {
            Claim::Unavailable => {
                return ctx
                    .reply_ephemeral("このページ送りは利用できません。")
                    .await;
            }
            Claim::Busy => return ctx.reply_ephemeral("ページを更新中です。").await,
            Claim::Ready(update) => update,
        };
        ctx.defer_update().await?;
        if !update.is_active() {
            return Ok(());
        }
        update.page.update(ctx).await?;
        update.finish();
        Ok(())
    }

    fn claim(&self, click: Click) -> Claim {
        let Ok(mut state) = self.shared.state.lock() else {
            return Claim::Unavailable;
        };
        let Some(entry) = state.entries.get(&click.key) else {
            return Claim::Unavailable;
        };
        if entry.deadline <= Instant::now()
            || entry.owner != click.owner
            || entry.source != Some(click.source)
            || entry.current != click.current
        {
            return Claim::Unavailable;
        }
        if state.updates.contains(&click.source)
            || state.updates.len() >= self.shared.limits.max_pending
        {
            return Claim::Busy;
        }
        let next = match click.direction {
            Direction::Previous => entry.current.checked_sub(1),
            Direction::Next => entry.current.checked_add(1),
        };
        let Some((next, page)) =
            next.and_then(|next| entry.pages.get(next).map(|page| (next, page)))
        else {
            return Claim::Unavailable;
        };
        let page = Arc::clone(page);
        state.updates.insert(click.source);
        Claim::Ready(Update {
            registry: self.clone(),
            key: click.key,
            source: click.source,
            next,
            page,
            committed: false,
        })
    }
}

/// 先頭ページを送信し終えるまでのページ送りです。破棄すると予約枠が空きます。
#[must_use]
pub struct PagerDraft {
    first: Arc<PagerPage>,
    pager: Option<Pager>,
}

impl fmt::Debug for PagerDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PagerDraft").finish_non_exhaustive()
    }
}

impl PagerDraft {
    /// 操作部品を付けた先頭ページを借ります。
    pub fn page(&self) -> &PagerPage {
        &self.first
    }

    /// 送ったメッセージを指定し、そのメッセージの操作を受け付けます。
    /// DM の場合、サーバー ID は `None` です。
    ///
    /// # エラー
    /// 制限時間を過ぎた、元メッセージがすでに使われている、またはサーバーや形式が一致しない場合に失敗します。
    pub fn bind(
        mut self,
        guild_id: Option<Id<GuildMarker>>,
        message: &Message,
    ) -> Result<Pager, PagerError> {
        if message
            .flags
            .is_some_and(|flags| flags.contains(MessageFlags::IS_COMPONENTS_V2))
            != self.first.is_v2()
        {
            return Err(PagerError::InvalidSource);
        }
        let source = Source::new(guild_id, message).ok_or(PagerError::InvalidSource)?;
        let pager = self.pager.as_ref().ok_or(PagerError::Ended)?;
        {
            let mut state = pager
                .registry
                .shared
                .state
                .lock()
                .map_err(|_| PagerError::Ended)?;
            state.bind(pager.key, source, Instant::now())?;
        }
        self.pager.take().ok_or(PagerError::Ended)
    }
}

impl Drop for PagerDraft {
    fn drop(&mut self) {
        if let Some(pager) = &self.pager {
            pager.stop();
        }
    }
}

/// 表示中のページ送りを調べたり停止したりするためのハンドルです。
/// このハンドルを破棄しても、元のページ送りは制限時間まで動き続けます。
#[derive(Clone)]
pub struct Pager {
    registry: Pagers,
    key: Key,
}

impl fmt::Debug for Pager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pager").finish_non_exhaustive()
    }
}

impl Pager {
    /// 最初を1とした現在のページ番号と、全ページ数を返します。
    /// 停止済みまたは期限切れなら `None` です。
    pub fn position(&self) -> Option<(usize, usize)> {
        let state = self.registry.shared.state.lock().ok()?;
        let entry = state.entries.get(&self.key)?;
        (entry.deadline > Instant::now()).then_some((entry.current + 1, entry.pages.len()))
    }

    /// 新しい操作の受付を停止します。すでに開始した Discord への更新は完了することがあります。
    /// すでに停止済みまたは期限切れなら `false` を返します。
    pub fn stop(&self) -> bool {
        let Some(entry) = self.registry.shared.remove(self.key) else {
            return false;
        };
        entry.timer.abort();
        entry.deadline > Instant::now()
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct Key {
    trigger: Id<InteractionMarker>,
    serial: u64,
}

impl Key {
    fn id(self, current: usize, direction: &str) -> String {
        format!(
            "{NAMESPACE}:{}:{}:{current}:{direction}",
            self.trigger.get(),
            self.serial
        )
    }
}

fn render_pages(key: Key, pages: Vec<PagerPage>) -> Result<Vec<Arc<PagerPage>>, PagerError> {
    let len = pages.len();
    pages
        .into_iter()
        .enumerate()
        .map(|(index, page)| {
            let buttons = vec![
                Button::secondary(key.id(index, "prev"), "前へ").disabled(index == 0),
                Button::secondary(key.id(index, "next"), "次へ").disabled(index + 1 == len),
            ];
            page.render(buttons)
                .map(Arc::new)
                .map_err(|source| PagerError::InvalidPage {
                    page: index + 1,
                    source: Box::new(source),
                })
        })
        .collect()
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct Source {
    guild_id: Option<Id<GuildMarker>>,
    channel_id: Id<ChannelMarker>,
    message_id: Id<MessageMarker>,
}

impl Source {
    fn new(guild_id: Option<Id<GuildMarker>>, message: &Message) -> Option<Self> {
        if message
            .guild_id
            .is_some_and(|guild| Some(guild) != guild_id)
        {
            return None;
        }
        Some(Self {
            guild_id,
            channel_id: message.channel_id,
            message_id: message.id,
        })
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Previous,
    Next,
}

struct Click {
    key: Key,
    current: usize,
    owner: Id<UserMarker>,
    source: Source,
    direction: Direction,
}

fn parse_click(
    dynamic: Option<DynamicIdRef<'_>>,
    owner: Option<Id<UserMarker>>,
    guild_id: Option<Id<GuildMarker>>,
    message: Option<&Message>,
) -> Option<Click> {
    let dynamic = dynamic?;
    if dynamic.namespace() != NAMESPACE {
        return None;
    }
    let mut fields = dynamic.value().split(':');
    let trigger = fields.next()?;
    let serial = fields.next()?;
    let current = fields.next()?;
    if ![trigger, serial, current].into_iter().all(|field| {
        (field.len() == 1 || !field.starts_with('0'))
            && !field.is_empty()
            && field.bytes().all(|byte| byte.is_ascii_digit())
    }) {
        return None;
    }
    let direction = match fields.next()? {
        "prev" => Direction::Previous,
        "next" => Direction::Next,
        _ => return None,
    };
    if fields.next().is_some() {
        return None;
    }
    Some(Click {
        key: Key {
            trigger: trigger.parse().ok()?,
            serial: serial.parse().ok()?,
        },
        current: current.parse().ok()?,
        owner: owner?,
        source: Source::new(guild_id, message?)?,
        direction,
    })
}

enum Claim {
    Ready(Update),
    Busy,
    Unavailable,
}

struct Update {
    registry: Pagers,
    key: Key,
    source: Source,
    next: usize,
    page: Arc<PagerPage>,
    committed: bool,
}

impl Update {
    fn is_active(&self) -> bool {
        self.registry.shared.state.lock().ok().is_some_and(|state| {
            state
                .entries
                .get(&self.key)
                .is_some_and(|entry| entry.deadline > Instant::now())
        })
    }

    fn finish(mut self) {
        self.committed = true;
    }
}

impl Drop for Update {
    fn drop(&mut self) {
        let Ok(mut state) = self.registry.shared.state.lock() else {
            return;
        };
        state.updates.remove(&self.source);
        if self.committed
            && let Some(entry) = state.entries.get_mut(&self.key)
            && entry.deadline > Instant::now()
        {
            entry.current = self.next;
        }
    }
}

struct Shared {
    limits: PagerLimits,
    state: Mutex<State>,
}

impl Shared {
    fn remove(&self, key: Key) -> Option<Entry> {
        self.state.lock().ok()?.remove(key)
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        if let Ok(state) = self.state.get_mut() {
            for entry in state.entries.values() {
                entry.timer.abort();
            }
        }
    }
}

#[derive(Default)]
struct State {
    serial: u64,
    entries: HashMap<Key, Entry>,
    users: HashMap<Id<UserMarker>, usize>,
    bindings: HashMap<Source, Key>,
    updates: HashSet<Source>,
}

struct Entry {
    owner: Id<UserMarker>,
    deadline: Instant,
    source: Option<Source>,
    pages: Vec<Arc<PagerPage>>,
    current: usize,
    timer: AbortHandle,
}

impl State {
    fn next_key(
        &mut self,
        trigger: Id<InteractionMarker>,
        owner: Id<UserMarker>,
        limits: PagerLimits,
    ) -> Result<Key, PagerError> {
        self.check_capacity(owner, limits)?;
        self.serial = self.serial.checked_add(1).ok_or(PagerError::Capacity)?;
        Ok(Key {
            trigger,
            serial: self.serial,
        })
    }

    fn check_capacity(&self, owner: Id<UserMarker>, limits: PagerLimits) -> Result<(), PagerError> {
        if self.entries.len() >= limits.max_pending {
            return Err(PagerError::Capacity);
        }
        if self.users.get(&owner).copied().unwrap_or_default() >= limits.per_user {
            return Err(PagerError::UserCapacity);
        }
        Ok(())
    }

    fn insert(&mut self, key: Key, entry: Entry) {
        *self.users.entry(entry.owner).or_default() += 1;
        self.entries.insert(key, entry);
    }

    fn bind(&mut self, key: Key, source: Source, now: Instant) -> Result<(), PagerError> {
        let occupied = self
            .bindings
            .get(&source)
            .and_then(|bound| self.entries.get(bound))
            .is_some_and(|entry| entry.deadline > now);
        if occupied || self.updates.contains(&source) {
            return Err(PagerError::SourceInUse);
        }
        let entry = self.entries.get_mut(&key).ok_or(PagerError::Ended)?;
        if entry.deadline <= now {
            return Err(PagerError::Ended);
        }
        entry.source = Some(source);
        self.bindings.insert(source, key);
        Ok(())
    }

    fn remove(&mut self, key: Key) -> Option<Entry> {
        let entry = self.entries.remove(&key)?;
        if let Some(source) = entry.source
            && self.bindings.get(&source) == Some(&key)
        {
            self.bindings.remove(&source);
        }
        if let Some(count) = self.users.get_mut(&entry.owner) {
            *count -= 1;
            if *count == 0 {
                self.users.remove(&entry.owner);
            }
        }
        Some(entry)
    }

    fn take_expired(&mut self, now: Instant) -> Vec<Entry> {
        let keys: Vec<_> = self
            .entries
            .iter()
            .filter_map(|(&key, entry)| (entry.deadline <= now).then_some(key))
            .collect();
        keys.into_iter()
            .filter_map(|key| self.remove(key))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use twilight_model::{channel::message::Component, http::attachment::Attachment};

    use super::*;

    const WAIT: Duration = Duration::from_secs(10);

    fn manager() -> Pagers {
        Pagers::new(PagerLimits {
            max_pending: 2,
            per_user: 1,
            max_pages: 3,
        })
        .unwrap()
    }

    fn pages() -> Vec<PagerPage> {
        (1..=3)
            .map(|index| UiMessage::new().content(index.to_string()).into())
            .collect()
    }

    fn message(id: u64, channel: u64, guild: Option<u64>) -> Message {
        serde_json::from_value(json!({
            "id":id.to_string(), "channel_id":channel.to_string(),
            "guild_id":guild.map(|id| id.to_string()), "type":0,
            "author":{"id":"3","username":"fixture","discriminator":"0","bot":true},
            "content":"pagination","timestamp":"2026-01-01T00:00:00.000000+00:00",
            "attachments":[],"embeds":[],"mention_everyone":false,"mention_roles":[],
            "mentions":[],"pinned":false,"tts":false
        }))
        .unwrap()
    }

    fn button_click(id: &str, who: u64, message: &Message) -> Click {
        parse_click(
            DynamicIdRef::parse(id),
            Some(Id::new(who)),
            message.guild_id,
            Some(message),
        )
        .unwrap()
    }

    fn ready(claim: Claim) -> Update {
        let Claim::Ready(update) = claim else {
            panic!("expected a valid page change");
        };
        update
    }

    #[tokio::test]
    async fn authenticates_owner_and_source_before_claiming() {
        let registry = manager();
        let draft = registry
            .begin(Id::new(10), Id::new(20), WAIT, pages())
            .unwrap();
        let key = draft.pager.as_ref().unwrap().key;
        let nav = key.id(0, "next");
        let source = message(30, 40, Some(50));
        let click = || button_click(&nav, 20, &source);
        assert!(matches!(registry.claim(click()), Claim::Unavailable));
        let pager = draft.bind(Some(Id::new(50)), &source).unwrap();
        for input in [
            button_click(&nav, 21, &source),
            button_click(&nav, 20, &message(31, 40, Some(50))),
            button_click(&nav, 20, &message(30, 41, Some(50))),
            button_click(&nav, 20, &message(30, 40, Some(51))),
            button_click(&key.id(0, "prev"), 20, &source),
        ] {
            assert!(matches!(registry.claim(input), Claim::Unavailable));
        }
        let update = ready(registry.claim(click()));
        assert_eq!(pager.position(), Some((1, 3)));
        assert!(matches!(registry.claim(click()), Claim::Busy));
        drop(update);
        assert_eq!(pager.position(), Some((1, 3)));
        ready(registry.claim(click())).finish();
        assert_eq!(pager.position(), Some((2, 3)));
        assert!(matches!(registry.claim(click()), Claim::Unavailable));
        let backwards = key.id(1, "prev");
        ready(registry.claim(button_click(&backwards, 20, &source))).finish();
        assert_eq!(pager.position(), Some((1, 3)));
    }

    #[tokio::test(start_paused = true)]
    async fn expiration_actively_releases_storage_and_stop_cancels_work() {
        let registry = manager();
        let draft = registry
            .begin(Id::new(10), Id::new(20), WAIT, pages())
            .unwrap();
        let key = draft.pager.as_ref().unwrap().key;
        let weak_page = {
            let state = registry.shared.state.lock().unwrap();
            Arc::downgrade(&state.entries[&key].pages[1])
        };
        let source = message(30, 40, None);
        let pager = draft.bind(None, &source).unwrap();
        let update = ready(registry.claim(button_click(&key.id(0, "next"), 20, &source)));
        let weak_update_page = Arc::downgrade(&update.page);
        drop(update);
        assert!(weak_page.upgrade().is_some());
        tokio::time::advance(WAIT + Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        // No registration or click has pruned the registry. The timer removed the page.
        assert!(weak_page.upgrade().is_none());
        assert!(weak_update_page.upgrade().is_none());
        assert_eq!(pager.position(), None);

        let replacement = registry
            .begin(Id::new(11), Id::new(20), WAIT, pages())
            .unwrap();
        let key = replacement.pager.as_ref().unwrap().key;
        let pager = replacement.bind(None, &source).unwrap();
        let in_flight = ready(registry.claim(button_click(&key.id(0, "next"), 20, &source)));
        assert!(pager.stop());
        assert!(!in_flight.is_active());
        in_flight.finish();
        assert_eq!(pager.position(), None);
    }

    #[tokio::test]
    async fn only_one_pager_can_edit_a_source_even_after_stop_while_a_request_is_in_flight() {
        let registry = manager();
        let source = message(30, 40, None);
        let first = registry
            .begin(Id::new(10), Id::new(20), WAIT, pages())
            .unwrap();
        let key = first.pager.as_ref().unwrap().key;
        let pager = first.bind(None, &source).unwrap();
        let second = || {
            registry
                .begin(Id::new(11), Id::new(21), WAIT, pages())
                .unwrap()
        };
        assert!(matches!(
            second().bind(None, &source),
            Err(PagerError::SourceInUse)
        ));

        let update = ready(registry.claim(button_click(&key.id(0, "next"), 20, &source)));
        assert!(pager.stop());
        assert!(matches!(
            second().bind(None, &source),
            Err(PagerError::SourceInUse)
        ));
        drop(update);
        let replacement = second().bind(None, &source).unwrap();
        assert_eq!(replacement.position(), Some((1, 3)));
    }

    #[tokio::test]
    async fn limits_apply_and_dropping_unbound_drafts_releases_the_reservation() {
        let registry = manager();
        let first = registry
            .begin(Id::new(10), Id::new(20), WAIT, pages())
            .unwrap();
        assert!(matches!(
            registry.begin(Id::new(11), Id::new(20), WAIT, pages()),
            Err(PagerError::UserCapacity)
        ));
        let second = registry
            .begin(Id::new(12), Id::new(21), WAIT, pages())
            .unwrap();
        assert!(matches!(
            registry.begin(Id::new(13), Id::new(22), WAIT, pages()),
            Err(PagerError::Capacity)
        ));
        drop(second);
        let third = registry
            .begin(Id::new(13), Id::new(22), WAIT, pages())
            .unwrap();
        drop(first);
        let fourth = registry
            .begin(Id::new(14), Id::new(20), WAIT, pages())
            .unwrap();
        drop(third);
        drop(fourth);
        assert!(registry.shared.state.lock().unwrap().entries.is_empty());
    }

    #[tokio::test]
    async fn renders_navigation_without_modifying_the_existing_rows_and_rejects_invalid_pages() {
        let registry = manager();
        let mut source = pages();
        source[0] = UiMessage::new()
            .content("first")
            .buttons(vec![Button::primary("app:save", "Save")])
            .into();
        let draft = registry
            .begin(Id::new(10), Id::new(20), WAIT, source)
            .unwrap();
        let PagerPage::Classic(page) = draft.page() else {
            panic!("expected classic page");
        };
        let [
            Component::ActionRow(user_row),
            Component::ActionRow(nav_row),
        ] = page.components.as_slice()
        else {
            panic!("missing navigation alongside the existing row");
        };
        let [Component::Button(save)] = user_row.components.as_slice() else {
            panic!("missing user button");
        };
        assert_eq!(save.custom_id.as_deref(), Some("app:save"));
        let [Component::Button(previous), Component::Button(next)] = nav_row.components.as_slice()
        else {
            panic!("missing direction buttons");
        };
        assert!(previous.disabled);
        assert!(!next.disabled);
        drop(draft);

        assert!(matches!(
            registry.begin(
                Id::new(10),
                Id::new(20),
                WAIT,
                vec![UiMessage::new().into(), ComponentsV2::new().into()]
            ),
            Err(PagerError::MixedFormats)
        ));
        let upload = ComponentsV2::new()
            .text("file")
            .attachment(Attachment::from_bytes(
                "fixture.txt".into(),
                b"fixture".to_vec(),
                1,
            ));
        assert!(matches!(
            registry.begin(Id::new(10), Id::new(20), WAIT, vec![upload.into()]),
            Err(PagerError::InvalidPage { page: 1, .. })
        ));
        let v2 = vec![ComponentsV2::new().text("v2").into()];
        let draft = registry.begin(Id::new(10), Id::new(20), WAIT, v2).unwrap();
        assert!(matches!(draft.page(), PagerPage::V2(_)));
        assert!(matches!(
            draft.bind(None, &message(30, 40, None)),
            Err(PagerError::InvalidSource)
        ));
    }

    #[test]
    fn duplicate_route_registration_preserves_the_first_handler() {
        let first = manager();
        let second = manager();
        let mut events = Events::new();
        assert!(first.register(&mut events).is_ok());
        assert!(second.register(&mut events).is_err());
    }
}
