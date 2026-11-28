use std::{
    collections::{BTreeSet, HashMap},
    fmt,
    panic::catch_unwind,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::{
    sync::oneshot,
    time::{Instant, sleep_until},
};
use twilight_model::{
    application::interaction::{
        Interaction, InteractionData, InteractionType,
        message_component::MessageComponentInteractionData,
    },
    channel::{Message, message::component::ComponentType},
    id::{
        Id,
        marker::{ChannelMarker, GuildMarker, InteractionMarker, MessageMarker, UserMarker},
    },
};

use crate::{slash::interaction_channel_id, ui::DynamicIdRef};

const NAMESPACE: &str = "akari-confirm";
const PREFIX: &str = "akari-confirm:";

/// 同時に開いておける確認の件数です。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfirmationLimits {
    /// Bot 全体の上限です。
    pub max_pending: usize,
    /// 同じユーザーについての上限です。
    pub per_user: usize,
}

impl Default for ConfirmationLimits {
    fn default() -> Self {
        Self {
            max_pending: 256,
            per_user: 4,
        }
    }
}

/// 確認を登録するか、元メッセージを指定する際のエラーです。
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ConfirmationError {
    /// 同時待機の上限が無効です。
    #[error("確認の同時待機枠の指定が無効です")]
    InvalidLimits,
    /// 待機時間がゼロ、または指定時刻を表せません。
    #[error("確認の制限時間が無効です")]
    InvalidTimeout,
    /// Bot が起動する前です。
    #[error("確認はまだ使えません")]
    NotActive,
    /// Bot が停止しました。
    #[error("確認の受付を終了しました")]
    Closed,
    /// Bot 全体の同時待機枠が埋まっています。
    #[error("確認の同時待機枠がいっぱいです")]
    Capacity,
    /// このユーザーの同時待機枠が埋まっています。
    #[error("このユーザーの確認の同時待機枠がいっぱいです")]
    UserCapacity,
    /// 元メッセージまたは操作範囲を確認できません。
    #[error("確認を表示するメッセージのサーバーを確認できません")]
    InvalidSource,
    /// 元メッセージはすでに指定されています。
    #[error("確認の元メッセージはすでに指定されています")]
    AlreadyBound,
    /// 確認はすでに終了しています。
    #[error("この確認は終了しています")]
    Ended,
}

/// 確認が終了した理由です。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ConfirmationOutcome {
    /// 本人が確定ボタンを押し、Discord が初回応答を受け付けました。
    Confirmed,
    /// 本人が取消ボタンを押し、Discord が初回応答を受け付けました。
    Cancelled,
    /// 制限時間内に確認が完了しませんでした。
    TimedOut,
    /// アプリが明示的に停止しました。
    Stopped,
    /// Bot の停止により待機が終了しました。
    Closed,
    /// Discord がボタン操作への初回応答を受け付けたか確認できませんでした。
    AcknowledgementFailed,
    /// Tokio のタイマーを利用できませんでした。
    TimerUnavailable,
}

/// 一回限りの確認を登録するためのハンドルです。
#[derive(Clone)]
pub struct Confirmations {
    shared: Arc<Shared>,
}

impl fmt::Debug for Confirmations {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Confirmations")
            .field("limits", &self.shared.limits)
            .finish_non_exhaustive()
    }
}

impl Confirmations {
    pub(crate) fn new(limits: ConfirmationLimits) -> Result<Self, ConfirmationError> {
        if limits.max_pending == 0 || limits.per_user == 0 || limits.per_user > limits.max_pending {
            return Err(ConfirmationError::InvalidLimits);
        }
        Ok(Self {
            shared: Arc::new(Shared {
                limits,
                state: Mutex::default(),
            }),
        })
    }

    pub(crate) fn activate(&self) {
        if let Ok(mut state) = self.shared.state.lock()
            && state.lifecycle == Lifecycle::Inactive
        {
            state.lifecycle = Lifecycle::Active;
        }
    }

    pub(crate) fn close(&self) {
        let entries = if let Ok(mut state) = self.shared.state.lock() {
            state.lifecycle = Lifecycle::Closed;
            state.users.clear();
            state.deadlines.clear();
            std::mem::take(&mut state.entries)
        } else {
            return;
        };
        for (_, entry) in entries {
            let _ = entry.result.send(ConfirmationOutcome::Closed);
        }
    }

    /// 確認を始めます。起点になった Interaction の ID と、操作を許可する本人を指定します。
    /// 時間はこの時点から進みます。ボタンを送った後、その投稿を [`Confirmation::bind`] で指定します。
    ///
    /// # エラー
    /// Bot の起動前・停止後、制限時間が無効、または同時待機枠がいっぱいの場合に失敗します。
    pub fn begin(
        &self,
        trigger: Id<InteractionMarker>,
        owner: Id<UserMarker>,
        timeout: Duration,
    ) -> Result<Confirmation, ConfirmationError> {
        let now = Instant::now();
        let deadline = now
            .checked_add(timeout)
            .filter(|_| !timeout.is_zero())
            .ok_or(ConfirmationError::InvalidTimeout)?;
        let (registered, expired) = {
            let mut state = self
                .shared
                .state
                .lock()
                .map_err(|_| ConfirmationError::Closed)?;
            let expired = state.take_expired(now);
            let registered = state.insert(self.shared.limits, trigger, owner, deadline);
            (registered, expired)
        };
        for sender in expired {
            let _ = sender.send(ConfirmationOutcome::TimedOut);
        }
        let (key, receiver) = registered?;
        Ok(Confirmation {
            stop: ConfirmationStop {
                confirmations: self.clone(),
                key,
            },
            receiver,
            deadline,
            confirm_id: key.custom_id("yes"),
            cancel_id: key.custom_id("no"),
        })
    }

    pub(crate) fn accepts(interaction: &Interaction) -> bool {
        component(interaction).is_some_and(|data| data.custom_id.starts_with(PREFIX))
    }

    pub(crate) fn claim(&self, interaction: &Interaction) -> ClaimResult {
        let Some(click) = Click::from_interaction(interaction) else {
            return ClaimResult::Unavailable;
        };
        let Ok(mut state) = self.shared.state.lock() else {
            return ClaimResult::Unavailable;
        };
        let Some(entry) = state.entries.get_mut(&click.key) else {
            return ClaimResult::Unavailable;
        };
        if entry.owner != click.owner
            || entry.source != Some(click.source)
            || entry.claimed
            || entry.deadline <= Instant::now()
        {
            return ClaimResult::Unavailable;
        }
        entry.claimed = true;
        ClaimResult::Claimed(ConfirmationClaim {
            confirmations: self.clone(),
            key: click.key,
            accepted: Some(click.choice),
        })
    }

    fn complete(&self, key: SessionKey, outcome: ConfirmationOutcome) -> bool {
        let (sender, outcome) = {
            let Ok(mut state) = self.shared.state.lock() else {
                return false;
            };
            let Some(entry) = state.remove(key) else {
                return false;
            };
            let outcome = if entry.deadline <= Instant::now() {
                ConfirmationOutcome::TimedOut
            } else {
                outcome
            };
            (entry.result, outcome)
        };
        let _ = sender.send(outcome);
        true
    }
}

/// 確認に使うボタンの ID と、その完了を待つためのハンドルです。
///
/// 破棄すると同時待機の枠が空きます。
#[must_use]
pub struct Confirmation {
    stop: ConfirmationStop,
    receiver: oneshot::Receiver<ConfirmationOutcome>,
    deadline: Instant,
    confirm_id: String,
    cancel_id: String,
}

impl fmt::Debug for Confirmation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Confirmation")
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

impl Confirmation {
    /// 確定ボタンの `custom_id` です。
    pub fn confirm_id(&self) -> &str {
        &self.confirm_id
    }

    /// 取消ボタンの `custom_id` です。
    pub fn cancel_id(&self) -> &str {
        &self.cancel_id
    }

    /// ボタンを送った元メッセージと、そのサーバーを一度だけ指定します。DM ならサーバーは `None` です。
    ///
    /// # エラー
    /// 指定済み、サーバーが一致しない、または確認が終了している場合に失敗します。
    pub fn bind(
        &self,
        guild_id: Option<Id<GuildMarker>>,
        message: &Message,
    ) -> Result<(), ConfirmationError> {
        let source = Source::new(guild_id, message).ok_or(ConfirmationError::InvalidSource)?;
        let mut state = self
            .stop
            .confirmations
            .shared
            .state
            .lock()
            .map_err(|_| ConfirmationError::Closed)?;
        let entry = state
            .entries
            .get_mut(&self.stop.key)
            .ok_or(ConfirmationError::Ended)?;
        if entry.deadline <= Instant::now() {
            return Err(ConfirmationError::Ended);
        }
        if entry.source.is_some() {
            return Err(ConfirmationError::AlreadyBound);
        }
        entry.source = Some(source);
        Ok(())
    }

    /// 別の処理からこの確認を停止できるハンドルです。
    pub fn stop_handle(&self) -> ConfirmationStop {
        self.stop.clone()
    }

    /// 本人の操作、明示的な停止、または制限時間まで待ちます。
    pub async fn wait(mut self) -> ConfirmationOutcome {
        match self.receiver.try_recv() {
            Ok(outcome) => return outcome,
            Err(oneshot::error::TryRecvError::Closed) => return ConfirmationOutcome::Closed,
            Err(oneshot::error::TryRecvError::Empty) => {}
        }
        let Ok(timer) = catch_unwind(|| sleep_until(self.deadline)) else {
            return ConfirmationOutcome::TimerUnavailable;
        };
        tokio::select! {
            biased;
            result = &mut self.receiver => result.unwrap_or(ConfirmationOutcome::Closed),
            () = timer => {
                self.stop.confirmations.complete(self.stop.key, ConfirmationOutcome::TimedOut);
                (&mut self.receiver).await.unwrap_or(ConfirmationOutcome::Closed)
            }
        }
    }
}

impl Drop for Confirmation {
    fn drop(&mut self) {
        self.stop.stop();
    }
}

/// 確認を別の処理から停止するハンドルです。
#[derive(Clone)]
pub struct ConfirmationStop {
    confirmations: Confirmations,
    key: SessionKey,
}

impl fmt::Debug for ConfirmationStop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfirmationStop").finish_non_exhaustive()
    }
}

impl ConfirmationStop {
    /// まだ待機中なら停止し、待機側を起こします。
    /// すでに終了している場合は `false` を返します。
    pub fn stop(&self) -> bool {
        self.confirmations
            .complete(self.key, ConfirmationOutcome::Stopped)
    }
}

pub(crate) enum ClaimResult {
    Claimed(ConfirmationClaim),
    Unavailable,
}

pub(crate) struct ConfirmationClaim {
    confirmations: Confirmations,
    key: SessionKey,
    accepted: Option<ConfirmationOutcome>,
}

impl ConfirmationClaim {
    pub(crate) fn finish(mut self, acknowledged: bool) {
        let accepted = self.accepted.take();
        let outcome = if acknowledged {
            accepted
        } else {
            Some(ConfirmationOutcome::AcknowledgementFailed)
        };
        if let Some(outcome) = outcome {
            self.confirmations.complete(self.key, outcome);
        }
    }
}

impl Drop for ConfirmationClaim {
    fn drop(&mut self) {
        if self.accepted.is_some() {
            self.confirmations
                .complete(self.key, ConfirmationOutcome::AcknowledgementFailed);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct SessionKey {
    trigger: Id<InteractionMarker>,
    serial: u64,
}

impl SessionKey {
    fn custom_id(self, action: &str) -> String {
        format!("{PREFIX}{}:{}:{action}", self.trigger, self.serial)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
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

struct Click {
    key: SessionKey,
    owner: Id<UserMarker>,
    source: Source,
    choice: ConfirmationOutcome,
}

impl Click {
    fn from_interaction(interaction: &Interaction) -> Option<Self> {
        let data = component(interaction)?;
        if data.component_type != ComponentType::Button {
            return None;
        }
        let dynamic = DynamicIdRef::parse(&data.custom_id)?;
        if dynamic.namespace() != NAMESPACE {
            return None;
        }
        let (ids, action) = dynamic.value().rsplit_once(':')?;
        let (trigger, serial) = ids.split_once(':')?;
        if !canonical_id(trigger) || !canonical_id(serial) {
            return None;
        }
        let choice = match action {
            "yes" => ConfirmationOutcome::Confirmed,
            "no" => ConfirmationOutcome::Cancelled,
            _ => return None,
        };
        let source = Source::new(interaction.guild_id, interaction.message.as_ref()?)?;
        if interaction_channel_id(interaction).is_some_and(|channel| channel != source.channel_id) {
            return None;
        }
        Some(Self {
            key: SessionKey {
                trigger: trigger.parse().ok()?,
                serial: serial.parse().ok()?,
            },
            owner: interaction.author_id()?,
            source,
            choice,
        })
    }
}

fn canonical_id(value: &str) -> bool {
    !value.is_empty() && !value.starts_with('0') && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn component(interaction: &Interaction) -> Option<&MessageComponentInteractionData> {
    match (interaction.kind, interaction.data.as_ref()) {
        (InteractionType::MessageComponent, Some(InteractionData::MessageComponent(data))) => {
            Some(data)
        }
        _ => None,
    }
}

struct Shared {
    limits: ConfirmationLimits,
    state: Mutex<State>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Lifecycle {
    #[default]
    Inactive,
    Active,
    Closed,
}

#[derive(Default)]
struct State {
    lifecycle: Lifecycle,
    serial: u64,
    entries: HashMap<SessionKey, Entry>,
    users: HashMap<Id<UserMarker>, usize>,
    deadlines: BTreeSet<(Instant, SessionKey)>,
}

struct Entry {
    owner: Id<UserMarker>,
    source: Option<Source>,
    deadline: Instant,
    claimed: bool,
    result: oneshot::Sender<ConfirmationOutcome>,
}

impl State {
    fn insert(
        &mut self,
        limits: ConfirmationLimits,
        trigger: Id<InteractionMarker>,
        owner: Id<UserMarker>,
        deadline: Instant,
    ) -> Result<(SessionKey, oneshot::Receiver<ConfirmationOutcome>), ConfirmationError> {
        match self.lifecycle {
            Lifecycle::Inactive => return Err(ConfirmationError::NotActive),
            Lifecycle::Closed => return Err(ConfirmationError::Closed),
            Lifecycle::Active => {}
        }
        if self.entries.len() >= limits.max_pending {
            return Err(ConfirmationError::Capacity);
        }
        if self.users.get(&owner).copied().unwrap_or_default() >= limits.per_user {
            return Err(ConfirmationError::UserCapacity);
        }
        self.serial = self
            .serial
            .checked_add(1)
            .ok_or(ConfirmationError::Capacity)?;
        let key = SessionKey {
            trigger,
            serial: self.serial,
        };
        let (result, receiver) = oneshot::channel();
        self.entries.insert(
            key,
            Entry {
                owner,
                source: None,
                deadline,
                claimed: false,
                result,
            },
        );
        *self.users.entry(owner).or_default() += 1;
        self.deadlines.insert((deadline, key));
        Ok((key, receiver))
    }

    fn remove(&mut self, key: SessionKey) -> Option<Entry> {
        let entry = self.entries.remove(&key)?;
        self.deadlines.remove(&(entry.deadline, key));
        if let Some(count) = self.users.get_mut(&entry.owner) {
            *count -= 1;
            if *count == 0 {
                self.users.remove(&entry.owner);
            }
        }
        Some(entry)
    }

    fn take_expired(&mut self, now: Instant) -> Vec<oneshot::Sender<ConfirmationOutcome>> {
        let mut expired = Vec::new();
        while let Some(&(deadline, key)) = self.deadlines.first() {
            if deadline > now {
                break;
            }
            if let Some(entry) = self.remove(key) {
                expired.push(entry.result);
            } else {
                self.deadlines.pop_first();
            }
        }
        expired
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const WAIT: Duration = Duration::from_secs(10);

    fn manager() -> Confirmations {
        let manager = Confirmations::new(ConfirmationLimits {
            max_pending: 2,
            per_user: 1,
        })
        .unwrap();
        manager.activate();
        manager
    }

    fn message(id: u64, channel: u64, guild: Option<u64>) -> Message {
        serde_json::from_value(json!({
            "id":id.to_string(), "channel_id":channel.to_string(),
            "guild_id":guild.map(|id| id.to_string()), "type":0,
            "author":{"id":"3","username":"fixture","discriminator":"0","bot":true},
            "content":"confirmation","timestamp":"2026-01-01T00:00:00.000000+00:00",
            "attachments":[],"embeds":[],"mention_everyone":false,"mention_roles":[],
            "mentions":[],"pinned":false,"tts":false
        }))
        .unwrap()
    }

    fn click(id: &str, owner: u64, guild: Option<u64>, source: &Message) -> Interaction {
        serde_json::from_value(json!({
            "id":"4", "application_id":"3", "type":3, "token":"fixture-only",
            "guild_id":guild.map(|id| id.to_string()), "message":source,
                "authorizing_integration_owners":{}, "attachment_size_limit":32,
            "data":{"custom_id":id,"component_type":2},
            "user":{"id":owner.to_string(),"username":"member","discriminator":"0"}
        }))
        .unwrap()
    }

    fn claim(manager: &Confirmations, event: &Interaction) -> ConfirmationClaim {
        let ClaimResult::Claimed(claim) = manager.claim(event) else {
            panic!("expected a valid confirmation click");
        };
        claim
    }

    #[tokio::test]
    async fn checks_owner_guild_channel_and_source_then_commits_after_acknowledgement() {
        let manager = manager();
        let confirmation = manager.begin(Id::new(10), Id::new(20), WAIT).unwrap();
        let message = message(30, 40, Some(50));
        let correct = click(confirmation.confirm_id(), 20, Some(50), &message);
        assert!(Confirmations::accepts(&correct));
        assert!(matches!(manager.claim(&correct), ClaimResult::Unavailable));
        assert_eq!(
            confirmation.bind(None, &message),
            Err(ConfirmationError::InvalidSource)
        );
        confirmation.bind(Some(Id::new(50)), &message).unwrap();
        assert_eq!(
            confirmation.bind(Some(Id::new(50)), &message),
            Err(ConfirmationError::AlreadyBound)
        );

        let mut wrong_message = message.clone();
        wrong_message.id = Id::new(31);
        let mut wrong_channel = message.clone();
        wrong_channel.channel_id = Id::new(41);
        let mut wrong_guild = message.clone();
        wrong_guild.guild_id = Some(Id::new(51));
        let mut without_source = correct.clone();
        without_source.message = None;
        for event in [
            click(confirmation.confirm_id(), 21, Some(50), &message),
            click(confirmation.confirm_id(), 20, Some(50), &wrong_message),
            click(confirmation.confirm_id(), 20, Some(50), &wrong_channel),
            click(confirmation.confirm_id(), 20, Some(51), &wrong_guild),
            without_source,
        ] {
            assert!(matches!(manager.claim(&event), ClaimResult::Unavailable));
        }
        let accepted = claim(&manager, &correct);
        assert!(matches!(manager.claim(&correct), ClaimResult::Unavailable));
        let completion = confirmation.wait();
        tokio::pin!(completion);
        tokio::select! {
            biased;
            outcome = &mut completion => panic!("completed before acknowledgement: {outcome:?}"),
            () = async {} => {}
        }
        accepted.finish(true);
        assert_eq!(completion.await, ConfirmationOutcome::Confirmed);
        assert!(matches!(manager.claim(&correct), ClaimResult::Unavailable));
    }

    #[tokio::test]
    async fn accepts_only_canonical_ids_and_reports_failed_acknowledgements() {
        let manager = manager();
        let source = message(30, 40, None);
        let first = manager.begin(Id::new(10), Id::new(20), WAIT).unwrap();
        first.bind(None, &source).unwrap();
        let malformed = click("akari-confirm:10:01:yes", 20, None, &source);
        assert!(Confirmations::accepts(&malformed));
        assert!(matches!(
            manager.claim(&malformed),
            ClaimResult::Unavailable
        ));
        let other = click("app:10:1:yes", 20, None, &source);
        assert!(!Confirmations::accepts(&other));
        let rejected = claim(&manager, &click(first.confirm_id(), 20, None, &source));
        rejected.finish(false);
        assert_eq!(
            first.wait().await,
            ConfirmationOutcome::AcknowledgementFailed
        );

        let second = manager.begin(Id::new(10), Id::new(20), WAIT).unwrap();
        second.bind(None, &source).unwrap();
        let accepted = claim(&manager, &click(second.cancel_id(), 20, None, &source));
        accepted.finish(true);
        assert_eq!(second.wait().await, ConfirmationOutcome::Cancelled);
    }

    #[tokio::test(start_paused = true)]
    async fn expiry_stop_and_drop_release_all_capacity() {
        let manager = manager();
        let first = manager.begin(Id::new(10), Id::new(20), WAIT).unwrap();
        assert_eq!(
            manager.begin(Id::new(11), Id::new(20), WAIT).unwrap_err(),
            ConfirmationError::UserCapacity
        );
        let second = manager.begin(Id::new(12), Id::new(21), WAIT * 2).unwrap();
        assert_eq!(
            manager.begin(Id::new(13), Id::new(22), WAIT).unwrap_err(),
            ConfirmationError::Capacity
        );
        tokio::time::advance(WAIT).await;
        let third = manager.begin(Id::new(13), Id::new(22), WAIT).unwrap();
        assert_eq!(first.wait().await, ConfirmationOutcome::TimedOut);
        drop(second);
        let fourth = manager.begin(Id::new(14), Id::new(21), WAIT).unwrap();
        let stop = third.stop_handle();
        assert!(stop.stop());
        assert!(!stop.stop());
        assert_eq!(third.wait().await, ConfirmationOutcome::Stopped);
        assert_eq!(fourth.wait().await, ConfirmationOutcome::TimedOut);
    }

    #[tokio::test(start_paused = true)]
    async fn completion_does_not_outlive_its_session() {
        let manager = manager();
        let source = message(30, 40, None);
        let first = manager.begin(Id::new(10), Id::new(20), WAIT).unwrap();
        first.bind(None, &source).unwrap();
        let pending = claim(&manager, &click(first.confirm_id(), 20, None, &source));
        tokio::time::advance(WAIT).await;
        pending.finish(true);
        assert_eq!(first.wait().await, ConfirmationOutcome::TimedOut);

        let second = manager.begin(Id::new(11), Id::new(20), WAIT).unwrap();
        second.bind(None, &source).unwrap();
        let pending = claim(&manager, &click(second.confirm_id(), 20, None, &source));
        let stop = second.stop_handle();
        assert!(stop.stop());
        pending.finish(true);
        assert_eq!(second.wait().await, ConfirmationOutcome::Stopped);

        let third = manager.begin(Id::new(12), Id::new(20), WAIT).unwrap();
        third.bind(None, &source).unwrap();
        let pending = claim(&manager, &click(third.confirm_id(), 20, None, &source));
        drop(pending);
        assert_eq!(
            third.wait().await,
            ConfirmationOutcome::AcknowledgementFailed
        );
    }

    #[tokio::test]
    async fn requires_activation_and_closing_wakes_pending_sessions() {
        let manager = Confirmations::new(ConfirmationLimits::default()).unwrap();
        let begin = || manager.begin(Id::new(10), Id::new(20), WAIT);
        assert_eq!(begin().unwrap_err(), ConfirmationError::NotActive);
        manager.activate();
        assert_eq!(
            manager
                .begin(Id::new(10), Id::new(20), Duration::ZERO)
                .unwrap_err(),
            ConfirmationError::InvalidTimeout
        );
        let pending = begin().unwrap();
        manager.close();
        manager.activate();
        assert_eq!(begin().unwrap_err(), ConfirmationError::Closed);
        assert_eq!(pending.wait().await, ConfirmationOutcome::Closed);
    }
}
