use std::{
    collections::{HashMap, HashSet, hash_map::Entry as MapEntry},
    fmt,
    future::Future,
    panic::catch_unwind,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::{
    runtime::Handle,
    sync::watch,
    task::AbortHandle,
    time::{Instant, sleep_until},
};
use twilight_model::{
    channel::Message,
    id::{
        Id,
        marker::{ChannelMarker, GuildMarker, InteractionMarker, MessageMarker, UserMarker},
    },
};

use crate::{
    ButtonContext, ChannelSelectContext, Events, MentionableSelectContext, ModalContext,
    RoleSelectContext, SelectContext, UserSelectContext,
    ui::{DynamicId, DynamicIdRef},
};

pub(crate) const UI_SESSION_NAMESPACE: &str = "akari-session";
const MAX_ACTION_CHARS: usize = 100 - UI_SESSION_NAMESPACE.len() - 1 - 2 * 21;

/// 同時に開ける UI セッション数と、各セッションが記憶する操作件数の上限です。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiSessionLimits {
    /// Bot 全体で同時に開けるセッションの数です。
    pub max_pending: usize,
    /// 同じユーザーについて同時に開けるセッションの数です。
    pub per_user: usize,
    /// 各セッションが登録できる操作の数です。
    pub max_actions: usize,
}

impl Default for UiSessionLimits {
    fn default() -> Self {
        Self {
            max_pending: 128,
            per_user: 4,
            max_actions: 20,
        }
    }
}

/// UI セッションまたは操作の準備に失敗した理由です。
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum UiSessionError {
    /// 指定した件数の上限の組み合わせが無効です。
    #[error("UI セッションの件数の上限が無効です")]
    InvalidLimits,
    /// 操作名が空か長すぎるか、制御文字を含んでいます。
    #[error("UI セッションの操作名は 1〜{MAX_ACTION_CHARS} 文字で、制御文字を含められません")]
    InvalidActionName,
    /// 同じ名前の操作がすでに登録されています。
    #[error("同じ名前の操作がすでに UI セッションへ登録されています")]
    DuplicateAction,
    /// UI セッションに操作が一つもありません。
    #[error("UI セッションの操作を登録してください")]
    NoActions,
    /// 設定した操作件数の上限を超えました。
    #[error("一つの UI セッションに登録できる操作は {max} 件までです")]
    TooManyActions {
        /// 設定で許可された操作の最大件数です。
        max: usize,
    },
    /// 操作 ID に使う名前が登録されていません。
    #[error("UI セッションに指定の操作はありません")]
    UnknownAction,
    /// 制限時間がゼロか、終了時刻を表現できません。
    #[error("UI セッションの制限時間が無効です")]
    InvalidTimeout,
    /// Tokio のタイマーを利用できません。
    #[error("UI セッションに必要な Tokio のタイマーを利用できません")]
    TimerUnavailable,
    /// Bot 全体の上限に達しました。
    #[error("UI セッションの全体枠がいっぱいです")]
    Capacity,
    /// このユーザーの上限に達しました。
    #[error("このユーザーの UI セッションの枠がいっぱいです")]
    UserCapacity,
    /// 操作の対象となるメッセージと、指定したサーバーが一致しません。
    #[error("UI セッションの元メッセージとサーバーが一致しません")]
    InvalidSource,
    /// 元メッセージには、別のセッションが紐づいているか、操作中です。
    #[error("元メッセージはすでに別の UI セッションで使われています")]
    SourceInUse,
    /// UI セッションが期限切れまたは停止済みです。
    #[error("この UI セッションは終了しています")]
    Ended,
    /// 管理ハンドルが終了し、新規セッションを受け付けません。
    #[error("UI セッションの受付は終了しています")]
    Closed,
}

/// UI セッションの終了理由です。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UiSessionOutcome {
    /// アプリが処理の完了を通知しました。
    Finished,
    /// アプリが明示的に操作の受付を停止しました。
    Stopped,
    /// 準備を始めてからの制限時間を過ぎました。
    TimedOut,
    /// 管理ハンドルが終了しました。
    Closed,
}

/// 一つの UI セッションに用意するボタン・選択メニュー・モーダルの操作です。
///
/// 操作名は、種類をまたいで一意にします。Discord に送る ID に含まれるため、秘密情報を含めないでください。
#[derive(Default)]
pub struct UiSessionActions {
    handlers: HashMap<String, Action>,
}

impl fmt::Debug for UiSessionActions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiSessionActions")
            .field("count", &self.handlers.len())
            .finish_non_exhaustive()
    }
}

impl UiSessionActions {
    /// 操作がまだ登録されていない状態を作ります。
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(&mut self, name: &str, action: Action) -> Result<(), UiSessionError> {
        if name.is_empty()
            || name.len() > 4 * MAX_ACTION_CHARS
            || name.chars().count() > MAX_ACTION_CHARS
            || name.chars().any(char::is_control)
        {
            return Err(UiSessionError::InvalidActionName);
        }
        let MapEntry::Vacant(slot) = self.handlers.entry(name.to_owned()) else {
            return Err(UiSessionError::DuplicateAction);
        };
        slot.insert(action);
        Ok(())
    }
}

/// 元メッセージと本人に紐づく、期限付き UI セッションの管理ハンドルです。
#[derive(Clone)]
pub struct UiSessions {
    shared: Arc<Shared>,
}

impl fmt::Debug for UiSessions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiSessions")
            .field("limits", &self.shared.limits)
            .finish_non_exhaustive()
    }
}

impl UiSessions {
    /// 上限を指定して UI セッションの管理ハンドルを作ります。
    ///
    /// # エラー
    /// 上限がゼロ、またはユーザーごとの上限が全体より大きい場合に失敗します。
    pub fn new(limits: UiSessionLimits) -> Result<Self, UiSessionError> {
        if limits.max_pending == 0
            || limits.per_user == 0
            || limits.per_user > limits.max_pending
            || limits.max_actions == 0
        {
            return Err(UiSessionError::InvalidLimits);
        }
        Ok(Self {
            shared: Arc::new(Shared {
                limits,
                state: Mutex::default(),
            }),
        })
    }

    /// 起点の Interaction と、操作を許可する本人を指定してセッションを準備します。
    /// 時間はこの時点から進みます。`register_*` で必要な操作の種類を事前に登録してください。
    ///
    /// # エラー
    /// 管理ハンドルが終了した、タイマーを使えない、または指定や件数が上限に合わない場合に失敗します。
    pub fn begin(
        &self,
        trigger: Id<InteractionMarker>,
        owner: Id<UserMarker>,
        timeout: Duration,
        actions: UiSessionActions,
    ) -> Result<UiSessionDraft, UiSessionError> {
        if actions.handlers.is_empty() {
            return Err(UiSessionError::NoActions);
        }
        if actions.handlers.len() > self.shared.limits.max_actions {
            return Err(UiSessionError::TooManyActions {
                max: self.shared.limits.max_actions,
            });
        }
        let deadline = Instant::now()
            .checked_add(timeout)
            .filter(|_| !timeout.is_zero())
            .ok_or(UiSessionError::InvalidTimeout)?;
        let runtime = Handle::try_current().map_err(|_| UiSessionError::TimerUnavailable)?;
        let timer =
            catch_unwind(|| sleep_until(deadline)).map_err(|_| UiSessionError::TimerUnavailable)?;
        let (end, outcome) = watch::channel(None);
        let (reserved, expired) = {
            let mut state = self
                .shared
                .state
                .lock()
                .map_err(|_| UiSessionError::Closed)?;
            let expired = state.take_expired(Instant::now());
            let reserved = match state.allocate(self.shared.limits, trigger, owner) {
                Ok(key) => {
                    let weak = Arc::downgrade(&self.shared);
                    let task = runtime.spawn(async move {
                        timer.await;
                        if let Some(shared) = weak.upgrade()
                            && let Some(slot) = shared.remove(key)
                        {
                            let _ = slot.end.send_replace(Some(UiSessionOutcome::TimedOut));
                        }
                    });
                    state.insert(
                        key,
                        Slot {
                            owner,
                            source: None,
                            deadline,
                            end,
                            actions: actions.handlers,
                            timer: task.abort_handle(),
                        },
                    );
                    Ok(key)
                }
                Err(error) => Err(error),
            };
            (reserved, expired)
        };
        for slot in expired {
            slot.notify(UiSessionOutcome::TimedOut);
        }
        Ok(UiSessionDraft {
            session: Some(UiSession {
                sessions: self.clone(),
                key: reserved?,
                outcome,
            }),
        })
    }

    /// 新規セッションを受け付けなくし、開いているセッションに終了を通知します。
    /// すでに実行中の操作は、終了の通知後も完了する場合があります。
    pub fn close(&self) {
        let slots = {
            let Ok(mut state) = self.shared.state.lock() else {
                return;
            };
            state.closed = true;
            state.bindings.clear();
            state.users.clear();
            std::mem::take(&mut state.slots)
        };
        for slot in slots.into_values() {
            slot.notify(UiSessionOutcome::Closed);
        }
    }

    fn claim(&self, click: Option<Click<'_>>, kind: ActionKind) -> Claim {
        let Some(click) = click else {
            return Claim::Unavailable;
        };
        let Ok(mut state) = self.shared.state.lock() else {
            return Claim::Unavailable;
        };
        let Some(slot) = state.slots.get(&click.key) else {
            return Claim::Unavailable;
        };
        if slot.deadline <= Instant::now()
            || slot.owner != click.owner
            || slot.source != Some(click.source)
        {
            return Claim::Unavailable;
        }
        let Some(action) = slot
            .actions
            .get(click.action)
            .filter(|action| action.matches(kind))
        else {
            return Claim::Unavailable;
        };
        if state.in_flight.len() >= self.shared.limits.max_pending
            || state.in_flight.contains(&click.source)
        {
            return Claim::Busy;
        }
        let action = action.clone();
        let outcome = slot.end.subscribe();
        state.in_flight.insert(click.source);
        Claim::Ready {
            lease: Lease {
                session: UiSession {
                    sessions: self.clone(),
                    key: click.key,
                    outcome,
                },
                source: click.source,
            },
            action,
        }
    }

    fn end(&self, key: Key, outcome: UiSessionOutcome) -> bool {
        let (slot, active) = {
            let Ok(mut state) = self.shared.state.lock() else {
                return false;
            };
            let Some(slot) = state.remove(key) else {
                return false;
            };
            let active = slot.deadline > Instant::now();
            (slot, active)
        };
        slot.notify(if active {
            outcome
        } else {
            UiSessionOutcome::TimedOut
        });
        active
    }
}

/// 元メッセージの送信を待つ UI セッションです。破棄すると予約枠を空けます。
#[must_use]
pub struct UiSessionDraft {
    session: Option<UiSession>,
}

impl fmt::Debug for UiSessionDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiSessionDraft").finish_non_exhaustive()
    }
}

impl UiSessionDraft {
    /// このセッション内の操作を指す ID を作ります。対応する操作部品またはモーダルに渡してください。
    ///
    /// # エラー
    /// 操作の名前が登録されていないか、セッションが終了している場合に失敗します。
    pub fn custom_id(&self, name: &str) -> Result<DynamicId, UiSessionError> {
        let session = self.session.as_ref().ok_or(UiSessionError::Ended)?;
        session.custom_id(name)
    }

    /// 元メッセージを登録し、このメッセージに対する操作を受け付けます。DM のサーバー ID は `None` です。
    ///
    /// # エラー
    /// 元メッセージが別のセッションで使われている、サーバーが一致しない、またはセッションが終了した場合に失敗します。
    pub fn bind(
        mut self,
        guild_id: Option<Id<GuildMarker>>,
        message: &Message,
    ) -> Result<UiSession, UiSessionError> {
        let source = Source::new(guild_id, message).ok_or(UiSessionError::InvalidSource)?;
        let session = self.session.as_ref().ok_or(UiSessionError::Ended)?;
        session
            .sessions
            .shared
            .state
            .lock()
            .map_err(|_| UiSessionError::Closed)?
            .bind(session.key, source, Instant::now())?;
        self.session.take().ok_or(UiSessionError::Ended)
    }
}

impl Drop for UiSessionDraft {
    fn drop(&mut self) {
        if let Some(session) = &self.session {
            session.stop();
        }
    }
}

/// 表示中の UI セッションを操作するためのハンドルです。
/// このハンドルを破棄しても、セッションは制限時間まで動き続けます。
#[derive(Clone)]
pub struct UiSession {
    sessions: UiSessions,
    key: Key,
    outcome: watch::Receiver<Option<UiSessionOutcome>>,
}

impl fmt::Debug for UiSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiSession")
            .field("outcome", &self.outcome())
            .finish_non_exhaustive()
    }
}

impl UiSession {
    /// 同じセッション内の別の操作 ID を作ります。ボタンで同じセッションのモーダルを開く場合にも使えます。
    ///
    /// # エラー
    /// 操作の名前が登録されていないか、セッションが終了している場合に失敗します。
    pub fn custom_id(&self, name: &str) -> Result<DynamicId, UiSessionError> {
        {
            let state = self
                .sessions
                .shared
                .state
                .lock()
                .map_err(|_| UiSessionError::Closed)?;
            let slot = state.slots.get(&self.key).ok_or(UiSessionError::Ended)?;
            if slot.deadline <= Instant::now() {
                return Err(UiSessionError::Ended);
            }
            if !slot.actions.contains_key(name) {
                return Err(UiSessionError::UnknownAction);
            }
        }
        let value = format!("{}:{}:{name}", self.key.trigger.get(), self.key.serial);
        DynamicId::new(UI_SESSION_NAMESPACE, &value).map_err(|_| UiSessionError::InvalidActionName)
    }

    /// セッションの終了理由を返します。受付中は `None` です。
    pub fn outcome(&self) -> Option<UiSessionOutcome> {
        *self.outcome.borrow()
    }

    /// セッションの終了を監視する受信ハンドルです。コマンドと独立した処理から待てます。
    pub fn subscribe(&self) -> watch::Receiver<Option<UiSessionOutcome>> {
        self.outcome.clone()
    }

    /// 新しい操作の受付を終了し、完了を通知します。すでに終了していれば `false` です。
    pub fn finish(&self) -> bool {
        self.sessions.end(self.key, UiSessionOutcome::Finished)
    }

    /// 新しい操作の受付を停止します。すでに開始した操作は完了する場合があります。
    pub fn stop(&self) -> bool {
        self.sessions.end(self.key, UiSessionOutcome::Stopped)
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct Key {
    trigger: Id<InteractionMarker>,
    serial: u64,
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
            .is_some_and(|actual| Some(actual) != guild_id)
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

struct Click<'a> {
    key: Key,
    action: &'a str,
    owner: Id<UserMarker>,
    source: Source,
}

impl<'a> Click<'a> {
    fn parse(
        dynamic: Option<DynamicIdRef<'a>>,
        owner: Option<Id<UserMarker>>,
        guild_id: Option<Id<GuildMarker>>,
        message: Option<&Message>,
    ) -> Option<Self> {
        let dynamic = dynamic?;
        if dynamic.namespace() != UI_SESSION_NAMESPACE {
            return None;
        }
        let mut fields = dynamic.value().splitn(3, ':');
        let trigger = fields.next()?;
        let serial = fields.next()?;
        if ![trigger, serial].into_iter().all(|part| {
            !part.is_empty()
                && !part.starts_with('0')
                && part.bytes().all(|byte| byte.is_ascii_digit())
        }) {
            return None;
        }
        Some(Self {
            key: Key {
                trigger: trigger.parse().ok()?,
                serial: serial.parse().ok()?,
            },
            action: fields.next()?,
            owner: owner?,
            source: Source::new(guild_id, message?)?,
        })
    }
}

enum Claim {
    Ready { lease: Lease, action: Action },
    Busy,
    Unavailable,
}

struct Lease {
    session: UiSession,
    source: Source,
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.session.sessions.shared.state.lock() {
            state.in_flight.remove(&self.source);
        }
    }
}

struct Shared {
    limits: UiSessionLimits,
    state: Mutex<State>,
}

impl Shared {
    fn remove(&self, key: Key) -> Option<Slot> {
        self.state.lock().ok()?.remove(key)
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        if let Ok(state) = self.state.get_mut() {
            for slot in state.slots.values() {
                slot.timer.abort();
                let _ = slot.end.send_replace(Some(UiSessionOutcome::Closed));
            }
        }
    }
}

#[derive(Default)]
struct State {
    serial: u64,
    closed: bool,
    slots: HashMap<Key, Slot>,
    users: HashMap<Id<UserMarker>, usize>,
    bindings: HashMap<Source, Key>,
    in_flight: HashSet<Source>,
}

struct Slot {
    owner: Id<UserMarker>,
    source: Option<Source>,
    deadline: Instant,
    actions: HashMap<String, Action>,
    end: watch::Sender<Option<UiSessionOutcome>>,
    timer: AbortHandle,
}

impl Slot {
    fn notify(self, outcome: UiSessionOutcome) {
        self.timer.abort();
        let _ = self.end.send_replace(Some(outcome));
    }
}

impl State {
    fn allocate(
        &mut self,
        limits: UiSessionLimits,
        trigger: Id<InteractionMarker>,
        owner: Id<UserMarker>,
    ) -> Result<Key, UiSessionError> {
        if self.closed {
            return Err(UiSessionError::Closed);
        }
        if self.slots.len() >= limits.max_pending {
            return Err(UiSessionError::Capacity);
        }
        if self.users.get(&owner).copied().unwrap_or_default() >= limits.per_user {
            return Err(UiSessionError::UserCapacity);
        }
        self.serial = self.serial.checked_add(1).ok_or(UiSessionError::Capacity)?;
        Ok(Key {
            trigger,
            serial: self.serial,
        })
    }

    fn insert(&mut self, key: Key, slot: Slot) {
        *self.users.entry(slot.owner).or_default() += 1;
        self.slots.insert(key, slot);
    }

    fn bind(&mut self, key: Key, source: Source, now: Instant) -> Result<(), UiSessionError> {
        let occupied = self
            .bindings
            .get(&source)
            .and_then(|bound| self.slots.get(bound))
            .is_some_and(|slot| slot.deadline > now);
        if occupied || self.in_flight.contains(&source) {
            return Err(UiSessionError::SourceInUse);
        }
        let slot = self.slots.get_mut(&key).ok_or(UiSessionError::Ended)?;
        if slot.deadline <= now {
            return Err(UiSessionError::Ended);
        }
        slot.source = Some(source);
        self.bindings.insert(source, key);
        Ok(())
    }

    fn remove(&mut self, key: Key) -> Option<Slot> {
        let slot = self.slots.remove(&key)?;
        if let Some(source) = slot.source
            && self.bindings.get(&source) == Some(&key)
        {
            self.bindings.remove(&source);
        }
        if let Some(count) = self.users.get_mut(&slot.owner) {
            *count -= 1;
            if *count == 0 {
                self.users.remove(&slot.owner);
            }
        }
        Some(slot)
    }

    fn take_expired(&mut self, now: Instant) -> Vec<Slot> {
        let keys: Vec<_> = self
            .slots
            .iter()
            .filter_map(|(&key, slot)| (slot.deadline <= now).then_some(key))
            .collect();
        keys.into_iter()
            .filter_map(|key| self.remove(key))
            .collect()
    }
}

macro_rules! action_kinds {
    ($((
        $variant:ident, $trait_name:ident, $context:ident, $register_action:ident,
        $register_route:ident, $register_event:ident, $message:ident, $name:literal
    )),+ $(,)?) => {
        $(
            trait $trait_name: Send + Sync {
                fn call<'a, 'ctx>(
                    &'a self,
                    ctx: &'a mut $context<'ctx>,
                    session: UiSession,
                ) -> Pin<Box<dyn Future<Output = crate::Result<()>> + 'a>>;
            }

            impl<F> $trait_name for F
            where
                F: AsyncFn(&mut $context<'_>, UiSession) -> crate::Result<()> + Send + Sync,
            {
                fn call<'a, 'ctx>(
                    &'a self,
                    ctx: &'a mut $context<'ctx>,
                    session: UiSession,
                ) -> Pin<Box<dyn Future<Output = crate::Result<()>> + 'a>> {
                    Box::pin(self(ctx, session))
                }
            }
        )+

        #[derive(Clone)]
        enum Action { $($variant(Arc<dyn $trait_name>)),+ }

        #[derive(Clone, Copy)]
        enum ActionKind { $($variant),+ }

        impl Action {
            fn matches(&self, kind: ActionKind) -> bool {
                matches!((self, kind), $((Self::$variant(_), ActionKind::$variant))|+)
            }
        }

        impl UiSessionActions {
            $(
                #[doc = concat!($name, "の操作を指定した名前で登録します。")]
                ///
                /// # エラー
                /// 操作名の長さが範囲外、制御文字を含む、または同名の操作が登録済みの場合に失敗します。
                pub fn $register_action(
                    &mut self,
                    name: &str,
                    handler: impl AsyncFn(&mut $context<'_>, UiSession) -> crate::Result<()>
                        + Send + Sync + 'static,
                ) -> Result<(), UiSessionError> {
                    self.insert(name, Action::$variant(Arc::new(handler)))
                }
            )+
        }

        impl UiSessions {
            $(
                #[doc = concat!("セッション内の", $name, "を Events で受け付けます。")]
                ///
                /// # エラー
                /// 同じ種類で `akari-session` の名前空間が登録済みの場合に失敗します。
                pub fn $register_route(&self, events: &mut Events) -> crate::Result<()> {
                    let sessions = self.clone();
                    events.$register_event(UI_SESSION_NAMESPACE, async move |ctx| {
                        let click = Click::parse(ctx.dynamic_id(), ctx.author_id(), ctx.guild_id(), ctx.$message());
                        match sessions.claim(click, ActionKind::$variant) {
                            Claim::Ready { lease, action: Action::$variant(handler) } => {
                                handler.call(ctx, lease.session.clone()).await
                            }
                            Claim::Busy => ctx.reply_ephemeral("この画面の操作を処理中です。").await,
                            Claim::Unavailable | Claim::Ready { .. } => {
                                ctx.reply_ephemeral("この画面は操作できません。").await
                            }
                        }
                    })
                }
            )+
        }
    };
}

action_kinds! {
    (Button, ButtonHandler, ButtonContext, on_button, register_button, on_dynamic_button, message, "ボタン"),
    (Select, SelectHandler, SelectContext, on_select, register_select, on_dynamic_select, message, "文字列セレクト"),
    (UserSelect, UserSelectHandler, UserSelectContext, on_user_select, register_user_select, on_dynamic_user_select, message, "ユーザーセレクト"),
    (RoleSelect, RoleSelectHandler, RoleSelectContext, on_role_select, register_role_select, on_dynamic_role_select, message, "ロールセレクト"),
    (ChannelSelect, ChannelSelectHandler, ChannelSelectContext, on_channel_select, register_channel_select, on_dynamic_channel_select, message, "チャンネルセレクト"),
    (MentionableSelect, MentionableSelectHandler, MentionableSelectContext, on_mentionable_select, register_mentionable_select, on_dynamic_mentionable_select, message, "ユーザーとロールのセレクト"),
    (Modal, ModalHandler, ModalContext, on_modal, register_modal, on_dynamic_modal, source_message, "モーダル"),
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use serde_json::json;

    use super::*;

    const WAIT: Duration = Duration::from_secs(10);

    fn manager() -> UiSessions {
        UiSessions::new(UiSessionLimits {
            max_pending: 2,
            per_user: 1,
            max_actions: 2,
        })
        .unwrap()
    }

    fn button_actions() -> UiSessionActions {
        let mut actions = UiSessionActions::new();
        actions.on_button("決定:確認", async |_, _| Ok(())).unwrap();
        actions
    }

    fn message(id: u64, channel: u64, guild: Option<u64>) -> Message {
        serde_json::from_value(json!({
            "id":id.to_string(), "channel_id":channel.to_string(),
            "guild_id":guild.map(|id| id.to_string()), "type":0,
            "author":{"id":"3","username":"fixture","discriminator":"0","bot":true},
            "content":"ui session","timestamp":"2026-01-01T00:00:00.000000+00:00",
            "attachments":[],"embeds":[],"mention_everyone":false,"mention_roles":[],
            "mentions":[],"pinned":false,"tts":false
        }))
        .unwrap()
    }

    fn input<'a>(id: &'a DynamicId, who: u64, source: &Message) -> Option<Click<'a>> {
        Click::parse(
            Some(id.parts()),
            Some(Id::new(who)),
            source.guild_id,
            Some(source),
        )
    }

    struct ReentrantOnDrop {
        sessions: UiSessions,
        dropped_outside_lock: Arc<AtomicBool>,
    }

    impl Drop for ReentrantOnDrop {
        fn drop(&mut self) {
            self.dropped_outside_lock.store(
                self.sessions.shared.state.try_lock().is_ok(),
                Ordering::Relaxed,
            );
        }
    }

    #[tokio::test]
    async fn checks_source_owner_and_kind_before_claiming_an_action() {
        let sessions = manager();
        let draft = sessions
            .begin(Id::new(10), Id::new(20), WAIT, button_actions())
            .unwrap();
        assert!(matches!(
            draft.custom_id("unregistered"),
            Err(UiSessionError::UnknownAction)
        ));
        let id = draft.custom_id("決定:確認").unwrap();
        let source = message(30, 40, Some(50));
        let click = || input(&id, 20, &source);
        assert!(matches!(
            sessions.claim(click(), ActionKind::Button),
            Claim::Unavailable
        ));
        let session = draft.bind(Some(Id::new(50)), &source).unwrap();
        assert!(matches!(
            sessions.claim(click(), ActionKind::Modal),
            Claim::Unavailable
        ));
        let no_message = Click::parse(Some(id.parts()), Some(Id::new(20)), Some(Id::new(50)), None);
        let no_user = Click::parse(Some(id.parts()), None, Some(Id::new(50)), Some(&source));
        for rejected in [
            input(&id, 21, &source),
            input(&id, 20, &message(31, 40, Some(50))),
            input(&id, 20, &message(30, 41, Some(50))),
            input(&id, 20, &message(30, 40, Some(51))),
            no_message,
            no_user,
        ] {
            assert!(matches!(
                sessions.claim(rejected, ActionKind::Button),
                Claim::Unavailable
            ));
        }
        let Claim::Ready {
            lease,
            action: Action::Button(_),
        } = sessions.claim(click(), ActionKind::Button)
        else {
            panic!("expected the registered button");
        };
        assert!(matches!(
            sessions.claim(click(), ActionKind::Button),
            Claim::Busy
        ));
        let mut updates = session.subscribe();
        assert!(lease.session.finish());
        updates.changed().await.unwrap();
        assert_eq!(*updates.borrow(), Some(UiSessionOutcome::Finished));
        assert!(!session.stop());
        assert!(matches!(
            sessions.claim(click(), ActionKind::Button),
            Claim::Unavailable
        ));
        assert!(matches!(
            session.custom_id("決定:確認"),
            Err(UiSessionError::Ended)
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn deadline_notifies_and_frees_action_captures_without_more_input() {
        let sessions = manager();
        let mut actions = UiSessionActions::new();
        let capture = Arc::new(AtomicUsize::new(0));
        let weak = Arc::downgrade(&capture);
        actions
            .on_role_select("role", async move |_, _| {
                capture.fetch_add(1, Ordering::Relaxed);
                Ok(())
            })
            .unwrap();
        let draft = sessions
            .begin(Id::new(10), Id::new(20), WAIT, actions)
            .unwrap();
        let source = message(30, 40, None);
        let role = draft.custom_id("role").unwrap();
        let session = draft.bind(None, &source).unwrap();
        let mut completion = session.subscribe();
        assert!(weak.upgrade().is_some());
        tokio::time::advance(WAIT + Duration::from_millis(1)).await;
        completion.changed().await.unwrap();
        assert_eq!(*completion.borrow(), Some(UiSessionOutcome::TimedOut));
        assert!(weak.upgrade().is_none());
        assert!(matches!(
            sessions.claim(input(&role, 20, &source), ActionKind::RoleSelect),
            Claim::Unavailable
        ));
        let next = sessions
            .begin(Id::new(11), Id::new(20), WAIT, button_actions())
            .unwrap();
        assert!(next.bind(None, &source).is_ok());
    }

    #[tokio::test]
    async fn completion_keeps_the_source_reserved_until_an_in_flight_action_exits() {
        let sessions = manager();
        let source = message(30, 40, None);
        let first = sessions
            .begin(Id::new(10), Id::new(20), WAIT, button_actions())
            .unwrap();
        let id = first.custom_id("決定:確認").unwrap();
        let first = first.bind(None, &source).unwrap();
        let replacement = || {
            sessions
                .begin(Id::new(11), Id::new(21), WAIT, button_actions())
                .unwrap()
        };
        assert!(matches!(
            replacement().bind(None, &source),
            Err(UiSessionError::SourceInUse)
        ));
        let Claim::Ready { lease, .. } =
            sessions.claim(input(&id, 20, &source), ActionKind::Button)
        else {
            panic!("expected the registered button");
        };
        assert!(first.stop());
        assert_eq!(first.outcome(), Some(UiSessionOutcome::Stopped));
        assert!(matches!(
            replacement().bind(None, &source),
            Err(UiSessionError::SourceInUse)
        ));
        drop(lease);
        let second = replacement().bind(None, &source).unwrap();
        let mut completion = second.subscribe();
        sessions.close();
        completion.changed().await.unwrap();
        assert_eq!(*completion.borrow(), Some(UiSessionOutcome::Closed));
        assert!(matches!(
            sessions.begin(Id::new(12), Id::new(22), WAIT, button_actions()),
            Err(UiSessionError::Closed)
        ));
    }

    #[tokio::test]
    async fn validates_registration_names_and_bounds_actions_and_sessions() {
        let sessions = manager();
        let mut actions = button_actions();
        assert_eq!(
            actions.on_modal("決定:確認", async |_, _| Ok(())),
            Err(UiSessionError::DuplicateAction)
        );
        assert_eq!(
            actions.on_modal("bad\nkey", async |_, _| Ok(())),
            Err(UiSessionError::InvalidActionName)
        );
        actions.on_modal("second", async |_, _| Ok(())).unwrap();
        actions
            .on_mentionable_select("third", async |_, _| Ok(()))
            .unwrap();
        assert!(matches!(
            sessions.begin(Id::new(10), Id::new(20), WAIT, actions),
            Err(UiSessionError::TooManyActions { max: 2 })
        ));
        let draft = sessions
            .begin(Id::new(10), Id::new(20), WAIT, button_actions())
            .unwrap();
        let dropped_outside_lock = Arc::new(AtomicBool::new(false));
        let capture = ReentrantOnDrop {
            sessions: sessions.clone(),
            dropped_outside_lock: Arc::clone(&dropped_outside_lock),
        };
        let mut rejected = UiSessionActions::new();
        rejected
            .on_button("reject", async move |_, _| {
                let _ = std::hint::black_box(&capture);
                Ok(())
            })
            .unwrap();
        assert!(matches!(
            sessions.begin(Id::new(11), Id::new(20), WAIT, rejected),
            Err(UiSessionError::UserCapacity)
        ));
        assert!(dropped_outside_lock.load(Ordering::Relaxed));
        let second = sessions
            .begin(Id::new(12), Id::new(21), WAIT, button_actions())
            .unwrap();
        assert!(matches!(
            sessions.begin(Id::new(13), Id::new(22), WAIT, button_actions()),
            Err(UiSessionError::Capacity)
        ));
        drop(draft);
        let third = sessions
            .begin(Id::new(14), Id::new(20), WAIT, button_actions())
            .unwrap();
        drop(second);
        drop(third);
        assert!(sessions.shared.state.lock().unwrap().slots.is_empty());
    }

    #[test]
    fn each_public_registration_routes_its_component_kind_and_preserves_duplicates() {
        let sessions = manager();
        let other = manager();
        let mut events = Events::new();
        sessions.register_button(&mut events).unwrap();
        sessions.register_select(&mut events).unwrap();
        sessions.register_user_select(&mut events).unwrap();
        sessions.register_role_select(&mut events).unwrap();
        sessions.register_channel_select(&mut events).unwrap();
        sessions.register_mentionable_select(&mut events).unwrap();
        sessions.register_modal(&mut events).unwrap();
        assert!(other.register_select(&mut events).is_err());
        assert!(other.register_modal(&mut events).is_err());
    }
}
