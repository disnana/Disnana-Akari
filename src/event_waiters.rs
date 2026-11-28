use std::{
    collections::HashMap,
    fmt,
    future::Future,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use tokio::{
    sync::{Semaphore, mpsc},
    time::{Instant, Sleep, sleep_until},
};
use twilight_model::{
    channel::Message,
    gateway::payload::incoming::ReactionAdd,
    id::{
        Id,
        marker::{ChannelMarker, GuildMarker, UserMarker},
    },
    voice::VoiceState,
};

/// 同時に待てるイベントの数と、各待機の受信枠です。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WaitLimits {
    /// 対象とイベント種類を問わず、同時待機の上限です。
    pub max_pending: usize,
    /// 対象やイベント種類を問わず、同じユーザーについての同時待機の上限です。
    pub per_user: usize,
    /// 1 件の待機について、一時的に受け取れるイベント候補の上限です。
    pub queue_capacity: usize,
}

impl Default for WaitLimits {
    fn default() -> Self {
        Self {
            max_pending: 256,
            per_user: 4,
            queue_capacity: 8,
        }
    }
}

/// イベント待ちの登録や完了に失敗した理由です。
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum WaitError {
    /// 受信枠や同時待機の上限が無効です。
    #[error("イベント待ちの枠の指定が無効です")]
    InvalidLimits,
    /// 待機時間が 0、または表現できる時刻の範囲を超えています。
    #[error("イベント待ちの制限時間が無効です")]
    InvalidTimeout,
    /// この種類の待機は Bot で有効になっていません。
    #[error("この種類のイベント待ちは有効になっていません")]
    NotEnabled,
    /// Tokio のタイマーを利用できません。
    #[error("イベント待ちに必要な Tokio のタイマーを利用できません")]
    TimerUnavailable,
    /// Bot 全体の同時待機の上限に達しました。
    #[error("イベント待ちの全体枠がいっぱいです")]
    Capacity,
    /// このユーザーについて同時待機の上限に達しました。
    #[error("このユーザーのイベント待ちの枠がいっぱいです")]
    UserCapacity,
    /// 期限までに条件を満たすイベントを受け取れませんでした。
    #[error("イベント待ちの制限時間を過ぎました")]
    TimedOut,
    /// 条件を確認する前に受信枠が埋まり、候補を取りこぼしました。
    #[error("イベント待ちの受信枠がいっぱいです")]
    Overflow,
    /// 条件を判定する関数がパニックしました。
    #[error("イベント待ちの条件を判定できませんでした")]
    PredicatePanicked,
    /// Bot が停止し、待機が終了しました。
    #[error("イベント待ちは終了しました")]
    Closed,
}

/// 指定したチャンネルまたはサーバーとユーザーについて、今後のイベントを待つハンドルです。
///
/// 一致する複数の待機には、同じイベントをそれぞれ配ります。
/// 登録の時点から期限が進み、返される [EventWait] を破棄すると待機を取り消します。
#[derive(Clone, Debug)]
pub struct EventWaiters {
    shared: Arc<Shared>,
}

type Predicate<T> = Box<dyn FnMut(&T) -> bool + Send>;

impl EventWaiters {
    /// メッセージを送信したユーザーとチャンネルを指定して待機を登録します。
    ///
    /// 一致した最初のメッセージを受け取ります。
    /// 本文を読む場合は、必要に応じて Discord の Message Content Intent を設定してください。
    ///
    /// # エラー
    ///
    /// 時間の指定、Intent、同時待機枠、または Tokio タイマーに問題がある場合に失敗します。
    pub fn wait_for_message(
        &self,
        channel: Id<ChannelMarker>,
        user: Id<UserMarker>,
        timeout: Duration,
    ) -> Result<EventWait<Message>, WaitError> {
        self.register(
            Key::new(Scope::Message(channel), user),
            timeout,
            None,
            Sender::Message,
        )
    }

    /// メッセージを追加の条件で絞り込んで待機します。
    ///
    /// 条件は呼び出し側のタスクで同期的に確認します。短時間で終わる処理を渡してください。
    /// 絞り込みが追いつかず待機の受信枠が埋まった場合は [WaitError::Overflow] になります。
    ///
    /// # エラー
    ///
    /// [Self::wait_for_message] と同じです。
    pub fn wait_for_message_where(
        &self,
        channel: Id<ChannelMarker>,
        user: Id<UserMarker>,
        timeout: Duration,
        predicate: impl FnMut(&Message) -> bool + Send + 'static,
    ) -> Result<EventWait<Message>, WaitError> {
        self.register(
            Key::new(Scope::Message(channel), user),
            timeout,
            Some(Box::new(predicate)),
            Sender::Message,
        )
    }

    /// リアクションを追加したユーザーとチャンネルを指定して待機を登録します。
    ///
    /// # エラー
    ///
    /// 時間の指定、Intent、同時待機枠、または Tokio タイマーに問題がある場合に失敗します。
    pub fn wait_for_reaction_add(
        &self,
        channel: Id<ChannelMarker>,
        user: Id<UserMarker>,
        timeout: Duration,
    ) -> Result<EventWait<ReactionAdd>, WaitError> {
        self.register(
            Key::new(Scope::Reaction(channel), user),
            timeout,
            None,
            Sender::Reaction,
        )
    }

    /// 追加されたリアクションを、絵文字やメッセージ ID などの条件で絞り込んで待機します。
    ///
    /// 条件の実行場所と受信枠については [Self::wait_for_message_where] を参照してください。
    ///
    /// # エラー
    ///
    /// [Self::wait_for_reaction_add] と同じです。
    pub fn wait_for_reaction_add_where(
        &self,
        channel: Id<ChannelMarker>,
        user: Id<UserMarker>,
        timeout: Duration,
        predicate: impl FnMut(&ReactionAdd) -> bool + Send + 'static,
    ) -> Result<EventWait<ReactionAdd>, WaitError> {
        self.register(
            Key::new(Scope::Reaction(channel), user),
            timeout,
            Some(Box::new(predicate)),
            Sender::Reaction,
        )
    }

    /// 指定したサーバーで、ユーザーの次のボイス状態更新を待ちます。
    ///
    /// 参加や移動のほか、ミュートなどの更新も届きます。退出時の
    /// `channel_id` は `None` です。`GUILD_VOICE_STATES` Intent が必要です。
    ///
    /// # エラー
    ///
    /// 時間の指定、Intent、同時待機枠、または Tokio タイマーに問題がある場合に失敗します。
    pub fn wait_for_voice_state(
        &self,
        guild: Id<GuildMarker>,
        user: Id<UserMarker>,
        timeout: Duration,
    ) -> Result<EventWait<VoiceState>, WaitError> {
        self.register(
            Key::new(Scope::VoiceState(guild), user),
            timeout,
            None,
            Sender::VoiceState,
        )
    }

    /// ボイス状態更新を、移動先や退出などの条件で絞り込んで待ちます。
    ///
    /// 条件の実行場所と受信枠については [Self::wait_for_message_where] を参照してください。
    ///
    /// # エラー
    ///
    /// [Self::wait_for_voice_state] と同じです。
    pub fn wait_for_voice_state_where(
        &self,
        guild: Id<GuildMarker>,
        user: Id<UserMarker>,
        timeout: Duration,
        predicate: impl FnMut(&VoiceState) -> bool + Send + 'static,
    ) -> Result<EventWait<VoiceState>, WaitError> {
        self.register(
            Key::new(Scope::VoiceState(guild), user),
            timeout,
            Some(Box::new(predicate)),
            Sender::VoiceState,
        )
    }

    pub(crate) fn new(limits: WaitLimits) -> Result<Self, WaitError> {
        if limits.max_pending == 0
            || limits.per_user == 0
            || limits.per_user > limits.max_pending
            || limits.queue_capacity == 0
            || limits.queue_capacity > Semaphore::MAX_PERMITS
        {
            return Err(WaitError::InvalidLimits);
        }
        Ok(Self {
            shared: Arc::new(Shared {
                limits,
                state: Mutex::default(),
                closed: AtomicBool::new(false),
            }),
        })
    }

    pub(crate) fn activate(&self, messages: bool, reactions: bool, voice_states: bool) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.messages = messages;
            state.reactions = reactions;
            state.voice_states = voice_states;
        }
    }

    pub(crate) fn close(&self) {
        let slots = if let Ok(mut state) = self.shared.state.lock() {
            self.shared.closed.store(true, Ordering::Release);
            state.users.clear();
            state.pending = 0;
            std::mem::take(&mut state.slots)
        } else {
            return;
        };
        // 送信元の破棄は待機側を起こすため、ロックを外してから行う。
        drop(slots);
    }

    pub(crate) fn message_cutoff(
        &self,
        channel: Id<ChannelMarker>,
        user: Id<UserMarker>,
    ) -> Option<u64> {
        self.cutoff(Key::new(Scope::Message(channel), user))
    }

    pub(crate) fn reaction_cutoff(
        &self,
        channel: Id<ChannelMarker>,
        user: Id<UserMarker>,
    ) -> Option<u64> {
        self.cutoff(Key::new(Scope::Reaction(channel), user))
    }

    pub(crate) fn voice_state_cutoff(
        &self,
        guild: Id<GuildMarker>,
        user: Id<UserMarker>,
    ) -> Option<u64> {
        self.cutoff(Key::new(Scope::VoiceState(guild), user))
    }

    pub(crate) fn deliver_message(&self, event: Arc<Message>, cutoff: u64) {
        let key = Key::new(Scope::Message(event.channel_id), event.author.id);
        self.deliver(key, cutoff, |sender| match sender {
            Sender::Message(sender) => {
                matches!(
                    sender.try_send(Arc::clone(&event)),
                    Err(mpsc::error::TrySendError::Full(_))
                )
            }
            _ => false,
        });
    }

    pub(crate) fn deliver_reaction(&self, event: Arc<ReactionAdd>, cutoff: u64) {
        let key = Key::new(Scope::Reaction(event.channel_id), event.user_id);
        self.deliver(key, cutoff, |sender| match sender {
            Sender::Reaction(sender) => {
                matches!(
                    sender.try_send(Arc::clone(&event)),
                    Err(mpsc::error::TrySendError::Full(_))
                )
            }
            _ => false,
        });
    }

    pub(crate) fn deliver_voice_state(&self, event: Arc<VoiceState>, cutoff: u64) {
        let Some(guild) = event.guild_id else {
            return;
        };
        let key = Key::new(Scope::VoiceState(guild), event.user_id);
        self.deliver(key, cutoff, |sender| match sender {
            Sender::VoiceState(sender) => matches!(
                sender.try_send(Arc::clone(&event)),
                Err(mpsc::error::TrySendError::Full(_))
            ),
            _ => false,
        });
    }

    fn register<T>(
        &self,
        key: Key,
        timeout: Duration,
        predicate: Option<Predicate<T>>,
        sender: impl FnOnce(mpsc::Sender<Arc<T>>) -> Sender,
    ) -> Result<EventWait<T>, WaitError> {
        let now = Instant::now();
        let deadline = now.checked_add(timeout).filter(|_| !timeout.is_zero());
        let deadline = deadline.ok_or(WaitError::InvalidTimeout)?;
        let shared = &self.shared;
        let expired;
        let mut state = shared.state.lock().map_err(|_| WaitError::Closed)?;
        if shared.closed.load(Ordering::Acquire) {
            return Err(WaitError::Closed);
        }
        if !state.enabled(key.scope) {
            return Err(WaitError::NotEnabled);
        }
        let limits = shared.limits;
        if state.pending >= limits.max_pending || state.user_count(key.user) >= limits.per_user {
            expired = state.prune(now);
        } else {
            expired = Vec::new();
        }
        if state.pending >= limits.max_pending {
            return Err(WaitError::Capacity);
        }
        if state.user_count(key.user) >= limits.per_user {
            return Err(WaitError::UserCapacity);
        }
        if tokio::runtime::Handle::try_current().is_err() {
            return Err(WaitError::TimerUnavailable);
        }
        let id = state.sequence.checked_add(1).ok_or(WaitError::Capacity)?;
        state.sequence = id;
        let capacity = if predicate.is_some() {
            limits.queue_capacity
        } else {
            1
        };
        let (inbox, receiver) = mpsc::channel(capacity);
        let overflow = predicate.as_ref().map(|_| Arc::new(AtomicBool::new(false)));
        state.slots.entry(key).or_default().push(Slot {
            id,
            deadline,
            sender: sender(inbox),
            overflow: overflow.clone(),
        });
        state.pending += 1;
        *state.users.entry(key.user).or_default() += 1;
        let registration = Registration {
            key,
            id,
            shared: Arc::clone(shared),
        };
        drop(state);
        drop(expired);
        let timer =
            catch_unwind(|| sleep_until(deadline)).map_err(|_| WaitError::TimerUnavailable)?;
        Ok(EventWait {
            receiver,
            registration: Some(registration),
            timer: Box::pin(timer),
            predicate,
            overflow,
        })
    }

    fn cutoff(&self, key: Key) -> Option<u64> {
        let state = self.shared.state.lock().ok()?;
        state.slots.contains_key(&key).then_some(state.sequence)
    }

    fn deliver(&self, key: Key, cutoff: u64, send: impl Fn(&Sender) -> bool) {
        let Ok(state) = self.shared.state.lock() else {
            return;
        };
        let Some(slots) = state.slots.get(&key) else {
            return;
        };
        let now = Instant::now();
        let targets: Vec<_> = slots
            .iter()
            .filter(|slot| slot.id <= cutoff && now < slot.deadline)
            .map(|slot| (slot.id, slot.sender.clone(), slot.overflow.clone()))
            .collect();
        // 送信で起きる Waker や、その先の条件判定にロックを渡さない。
        drop(state);
        for (id, sender, overflow) in targets {
            if overflow
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Acquire))
            {
                continue;
            }
            if send(&sender)
                && let Some(overflow) = overflow
            {
                overflow.store(true, Ordering::Release);
                // 候補の続きは受け付けず、受信側は既に届いた候補を先に確認する。
                self.shared.unregister(key, id);
            }
        }
    }
}

/// 一致するイベントの到着、制限時間、または Bot の停止を待つ Future です。
///
/// 待機を破棄すると、同時待機の枠がすぐに空きます。
#[must_use = "待機を使うには await してください"]
pub struct EventWait<T> {
    receiver: mpsc::Receiver<Arc<T>>,
    registration: Option<Registration>,
    timer: Pin<Box<Sleep>>,
    predicate: Option<Predicate<T>>,
    overflow: Option<Arc<AtomicBool>>,
}

impl<T> fmt::Debug for EventWait<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventWait")
            .field("deadline", &self.timer.deadline())
            .field("has_predicate", &self.predicate.is_some())
            .finish_non_exhaustive()
    }
}

impl<T> Future for EventWait<T> {
    type Output = Result<Arc<T>, WaitError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        for _ in 0..16 {
            if self.closed() {
                return self.finish(Err(WaitError::Closed));
            }
            if self.timer.as_mut().poll(cx).is_ready() {
                return self.finish(Err(WaitError::TimedOut));
            }
            match self.receiver.poll_recv(cx) {
                Poll::Pending | Poll::Ready(None) if self.closed() => {
                    return self.finish(Err(WaitError::Closed));
                }
                Poll::Pending | Poll::Ready(None)
                    if self
                        .overflow
                        .as_ref()
                        .is_some_and(|flag| flag.load(Ordering::Acquire)) =>
                {
                    return self.finish(Err(WaitError::Overflow));
                }
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return self.finish(Err(WaitError::TimedOut)),
                Poll::Ready(Some(event)) => {
                    let accepted = self.predicate.as_mut().map_or(Ok(true), |predicate| {
                        catch_unwind(AssertUnwindSafe(|| predicate(&event)))
                    });
                    match accepted {
                        Ok(true) if self.closed() => return self.finish(Err(WaitError::Closed)),
                        Ok(true) if Instant::now() >= self.timer.deadline() => {
                            return self.finish(Err(WaitError::TimedOut));
                        }
                        Ok(true) => return self.finish(Ok(event)),
                        Ok(false) => {}
                        Err(_) => return self.finish(Err(WaitError::PredicatePanicked)),
                    }
                }
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

impl<T> EventWait<T> {
    fn closed(&self) -> bool {
        self.registration
            .as_ref()
            .is_none_or(|registration| registration.shared.closed.load(Ordering::Acquire))
    }

    fn finish(&mut self, value: Result<Arc<T>, WaitError>) -> Poll<Result<Arc<T>, WaitError>> {
        self.registration.take();
        self.receiver.close();
        Poll::Ready(value)
    }
}

#[derive(Debug)]
struct Shared {
    limits: WaitLimits,
    state: Mutex<State>,
    closed: AtomicBool,
}

impl Shared {
    fn unregister(&self, key: Key, id: u64) {
        let removed = self
            .state
            .lock()
            .ok()
            .and_then(|mut state| state.remove(key, id));
        drop(removed);
    }
}

struct Registration {
    key: Key,
    id: u64,
    shared: Arc<Shared>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.shared.unregister(self.key, self.id);
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Scope {
    Message(Id<ChannelMarker>),
    Reaction(Id<ChannelMarker>),
    VoiceState(Id<GuildMarker>),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Key {
    scope: Scope,
    user: Id<UserMarker>,
}

impl Key {
    fn new(scope: Scope, user: Id<UserMarker>) -> Self {
        Self { scope, user }
    }
}

#[derive(Clone, Debug)]
enum Sender {
    Message(mpsc::Sender<Arc<Message>>),
    Reaction(mpsc::Sender<Arc<ReactionAdd>>),
    VoiceState(mpsc::Sender<Arc<VoiceState>>),
}

#[derive(Debug)]
struct Slot {
    id: u64,
    deadline: Instant,
    sender: Sender,
    overflow: Option<Arc<AtomicBool>>,
}

#[derive(Debug, Default)]
struct State {
    messages: bool,
    reactions: bool,
    voice_states: bool,
    sequence: u64,
    pending: usize,
    slots: HashMap<Key, Vec<Slot>>,
    users: HashMap<Id<UserMarker>, usize>,
}

impl State {
    fn enabled(&self, scope: Scope) -> bool {
        match scope {
            Scope::Message(_) => self.messages,
            Scope::Reaction(_) => self.reactions,
            Scope::VoiceState(_) => self.voice_states,
        }
    }

    fn user_count(&self, user: Id<UserMarker>) -> usize {
        self.users.get(&user).copied().unwrap_or(0)
    }

    fn remove(&mut self, key: Key, id: u64) -> Option<Slot> {
        let slots = self.slots.get_mut(&key)?;
        let index = slots.iter().position(|slot| slot.id == id)?;
        let removed = slots.swap_remove(index);
        if slots.is_empty() {
            self.slots.remove(&key);
        }
        decrement(&mut self.users, key.user, 1);
        self.pending -= 1;
        Some(removed)
    }

    fn prune(&mut self, now: Instant) -> Vec<Slot> {
        let mut removed = Vec::new();
        self.slots.retain(|key, slots| {
            let previous = removed.len();
            let mut index = 0;
            while let Some(slot) = slots.get(index) {
                if slot.deadline <= now {
                    removed.push(slots.swap_remove(index));
                } else {
                    index += 1;
                }
            }
            let expired = removed.len() - previous;
            if expired > 0 {
                self.pending -= expired;
                decrement(&mut self.users, key.user, expired);
            }
            !slots.is_empty()
        });
        removed
    }
}

fn decrement(users: &mut HashMap<Id<UserMarker>, usize>, user: Id<UserMarker>, count: usize) {
    if let Some(remaining) = users.get_mut(&user) {
        *remaining -= count;
        if *remaining == 0 {
            users.remove(&user);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::task::{Wake, Waker};

    use super::*;

    const CHANNEL: Id<ChannelMarker> = Id::new(2);
    const GUILD: Id<GuildMarker> = Id::new(2);
    const USER: Id<UserMarker> = Id::new(3);
    const TIMEOUT: Duration = Duration::from_secs(5);

    fn message(content: &str) -> Arc<Message> {
        let mut event: Message = serde_json::from_str(
            r#"{
            "id":"1", "channel_id":"2", "type":0,
            "author":{"id":"3","username":"member","discriminator":"0"},
            "content":"", "timestamp":"2026-01-01T00:00:00.000000+00:00",
            "attachments":[], "embeds":[], "mention_everyone":false, "mention_roles":[],
            "mentions":[], "pinned":false, "tts":false
        }"#,
        )
        .unwrap();
        event.content = content.into();
        Arc::new(event)
    }

    fn offer(waiters: &EventWaiters, text: &str) {
        if let Some(cutoff) = waiters.message_cutoff(CHANNEL, USER) {
            waiters.deliver_message(message(text), cutoff);
        }
    }

    fn voice(
        guild: Option<Id<GuildMarker>>,
        user: Id<UserMarker>,
        channel: Option<Id<ChannelMarker>>,
    ) -> Arc<VoiceState> {
        Arc::new(VoiceState {
            channel_id: channel,
            guild_id: guild,
            member: None,
            user_id: user,
            deaf: false,
            mute: false,
            self_deaf: false,
            self_mute: false,
            self_stream: false,
            self_video: false,
            suppress: false,
            request_to_speak_timestamp: None,
            session_id: "fixture".into(),
        })
    }

    fn active(limits: WaitLimits) -> EventWaiters {
        let waiters = EventWaiters::new(limits).unwrap();
        waiters.activate(true, true, true);
        waiters
    }

    #[tokio::test]
    async fn quotas_count_all_event_types_and_drop_releases_them() {
        let limits = WaitLimits {
            max_pending: 2,
            per_user: 1,
            queue_capacity: 1,
        };
        let waiters = EventWaiters::new(limits).unwrap();
        assert_eq!(
            waiters
                .wait_for_message(CHANNEL, USER, TIMEOUT)
                .unwrap_err(),
            WaitError::NotEnabled
        );
        waiters.activate(true, false, false);
        assert_eq!(
            waiters
                .wait_for_reaction_add(CHANNEL, USER, TIMEOUT)
                .unwrap_err(),
            WaitError::NotEnabled
        );
        assert_eq!(
            waiters
                .wait_for_voice_state(GUILD, USER, TIMEOUT)
                .unwrap_err(),
            WaitError::NotEnabled
        );
        waiters.activate(true, true, true);
        let first = waiters.wait_for_message(CHANNEL, USER, TIMEOUT).unwrap();
        assert_eq!(
            waiters
                .wait_for_reaction_add(Id::new(8), USER, TIMEOUT)
                .unwrap_err(),
            WaitError::UserCapacity
        );
        assert_eq!(
            waiters
                .wait_for_voice_state(GUILD, USER, TIMEOUT)
                .unwrap_err(),
            WaitError::UserCapacity
        );
        let second = waiters
            .wait_for_reaction_add(CHANNEL, Id::new(4), TIMEOUT)
            .unwrap();
        assert_eq!(
            waiters
                .wait_for_message(CHANNEL, Id::new(5), TIMEOUT)
                .unwrap_err(),
            WaitError::Capacity
        );
        drop(first);
        let replacement = waiters.wait_for_voice_state(GUILD, USER, TIMEOUT).unwrap();
        assert!(waiters.voice_state_cutoff(GUILD, USER).is_some());
        drop(second);
        drop(replacement);
        assert!(waiters.message_cutoff(CHANNEL, USER).is_none());
        assert!(waiters.reaction_cutoff(CHANNEL, Id::new(4)).is_none());
        assert!(waiters.voice_state_cutoff(GUILD, USER).is_none());
    }

    #[tokio::test]
    async fn voice_waiters_match_guild_user_predicate_and_registration_watermark() {
        let waiters = active(WaitLimits::default());
        let old = waiters.wait_for_voice_state(GUILD, USER, TIMEOUT).unwrap();
        let cutoff = waiters.voice_state_cutoff(GUILD, USER).unwrap();
        let first = waiters
            .wait_for_voice_state_where(GUILD, USER, TIMEOUT, |event| event.channel_id.is_none())
            .unwrap();
        let second = waiters
            .wait_for_voice_state_where(GUILD, USER, TIMEOUT, |event| event.channel_id.is_none())
            .unwrap();
        let other_guild = Id::new(8);
        let other = waiters
            .wait_for_voice_state(other_guild, USER, TIMEOUT)
            .unwrap();

        // 新しい待機を登録する前の退出イベントは、後から登録した条件に一致しても配らない。
        let stale_leave = voice(Some(GUILD), USER, None);
        waiters.deliver_voice_state(Arc::clone(&stale_leave), cutoff);
        assert!(Arc::ptr_eq(&old.await.unwrap(), &stale_leave));

        let cutoff = waiters.voice_state_cutoff(GUILD, USER).unwrap();
        for unrelated in [
            voice(None, USER, None),
            voice(Some(GUILD), Id::new(4), None),
            voice(Some(GUILD), USER, Some(CHANNEL)),
        ] {
            waiters.deliver_voice_state(unrelated, cutoff);
        }
        let other_leave = voice(Some(other_guild), USER, None);
        waiters.deliver_voice_state(Arc::clone(&other_leave), cutoff);
        let leave = voice(Some(GUILD), USER, None);
        waiters.deliver_voice_state(Arc::clone(&leave), cutoff);
        assert!(Arc::ptr_eq(&other.await.unwrap(), &other_leave));
        assert!(Arc::ptr_eq(&first.await.unwrap(), &leave));
        assert!(Arc::ptr_eq(&second.await.unwrap(), &leave));
        assert!(waiters.voice_state_cutoff(GUILD, USER).is_none());
        assert!(waiters.voice_state_cutoff(other_guild, USER).is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn deadlines_also_release_quota_for_futures_that_have_not_been_polled() {
        let limits = WaitLimits {
            max_pending: 1,
            per_user: 1,
            queue_capacity: 1,
        };
        let waiters = active(limits);
        let expired = waiters.wait_for_message(CHANNEL, USER, TIMEOUT).unwrap();
        tokio::time::advance(TIMEOUT).await;
        let live = waiters.wait_for_message(CHANNEL, USER, TIMEOUT).unwrap();
        assert_eq!(expired.await.unwrap_err(), WaitError::TimedOut);
        offer(&waiters, "fresh");
        assert_eq!(live.await.unwrap().content, "fresh");
        let timeout = waiters.wait_for_message(CHANNEL, USER, TIMEOUT).unwrap();
        assert_eq!(timeout.await.unwrap_err(), WaitError::TimedOut);
        assert!(waiters.message_cutoff(CHANNEL, USER).is_none());
    }

    #[tokio::test]
    async fn old_events_cannot_reach_new_waiters_and_each_match_gets_the_same_payload() {
        let waiters = active(WaitLimits::default());
        let first = waiters.wait_for_message(CHANNEL, USER, TIMEOUT).unwrap();
        let cutoff = waiters.message_cutoff(CHANNEL, USER).unwrap();
        let second = waiters.wait_for_message(CHANNEL, USER, TIMEOUT).unwrap();
        let third = waiters.wait_for_message(CHANNEL, USER, TIMEOUT).unwrap();
        waiters.deliver_message(message("old"), cutoff);
        assert_eq!(first.await.unwrap().content, "old");

        let mut wrong = message("wrong channel");
        Arc::get_mut(&mut wrong).unwrap().channel_id = Id::new(8);
        waiters.deliver_message(wrong, u64::MAX);
        let new = message("new");
        let cutoff = waiters.message_cutoff(CHANNEL, USER).unwrap();
        waiters.deliver_message(Arc::clone(&new), cutoff);
        assert!(Arc::ptr_eq(&second.await.unwrap(), &new));
        assert!(Arc::ptr_eq(&third.await.unwrap(), &new));
    }

    #[tokio::test]
    async fn overflow_keeps_earlier_buffered_matches_and_releases_the_slots() {
        let limits = WaitLimits {
            max_pending: 2,
            per_user: 2,
            queue_capacity: 1,
        };
        let waiters = active(limits);
        let matching = waiters
            .wait_for_message_where(CHANNEL, USER, TIMEOUT, |event| event.content == "first")
            .unwrap();
        let never = waiters
            .wait_for_message_where(CHANNEL, USER, TIMEOUT, |_| false)
            .unwrap();
        offer(&waiters, "first");
        offer(&waiters, "overflow");
        assert!(waiters.message_cutoff(CHANNEL, USER).is_none());
        assert_eq!(matching.await.unwrap().content, "first");
        assert_eq!(never.await.unwrap_err(), WaitError::Overflow);
    }

    #[tokio::test]
    async fn close_cancels_waits_even_if_a_candidate_is_already_buffered() {
        let waiters = active(WaitLimits::default());
        let ready = waiters.wait_for_message(CHANNEL, USER, TIMEOUT).unwrap();
        offer(&waiters, "buffered");
        waiters.close();
        assert_eq!(ready.await.unwrap_err(), WaitError::Closed);
        assert_eq!(
            waiters
                .wait_for_message(CHANNEL, USER, TIMEOUT)
                .unwrap_err(),
            WaitError::Closed
        );
    }

    struct ReenteringWake {
        waiters: EventWaiters,
        woke: AtomicBool,
    }

    impl Wake for ReenteringWake {
        fn wake(self: Arc<Self>) {
            assert!(self.waiters.shared.state.try_lock().is_ok());
            self.woke.store(true, Ordering::Relaxed);
        }
    }

    #[tokio::test]
    async fn waking_and_running_a_predicate_do_not_hold_the_registry_lock() {
        let waiters = active(WaitLimits::default());
        let predicate_calls = Arc::new(AtomicBool::new(false));
        let calls = Arc::clone(&predicate_calls);
        let reenter = waiters.clone();
        let mut waiting = waiters
            .wait_for_message_where(CHANNEL, USER, TIMEOUT, move |_| {
                assert!(reenter.shared.state.try_lock().is_ok());
                calls.store(true, Ordering::Relaxed);
                true
            })
            .unwrap();
        let check = Arc::new(ReenteringWake {
            waiters: waiters.clone(),
            woke: AtomicBool::new(false),
        });
        let waker = Waker::from(Arc::clone(&check));
        let mut cx = Context::from_waker(&waker);
        assert!(Pin::new(&mut waiting).poll(&mut cx).is_pending());
        offer(&waiters, "wake");
        assert!(check.woke.load(Ordering::Relaxed));
        assert!(!predicate_calls.load(Ordering::Relaxed));
        assert_eq!(waiting.await.unwrap().content, "wake");
        assert!(predicate_calls.load(Ordering::Relaxed));
    }
}
