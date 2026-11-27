use std::time::Duration;

use tokio::{sync::watch, time::Instant};
use twilight_model::gateway::{
    OpCode,
    payload::outgoing::{UpdatePresence, update_presence::UpdatePresencePayload},
    presence::{MinimalActivity, Status},
};

// Discord のアクティビティ更新は20秒に最大5回。再接続による再送にも同じ間隔を使う。
const MIN_UPDATE_INTERVAL: Duration = Duration::from_secs(5);

/// Discord に表示する Bot のオンライン状態とアクティビティです。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presence(UpdatePresencePayload);

impl Presence {
    /// アクティビティなしの状態を作ります。`Status::Offline` は `Status::Invisible` として送ります。
    pub fn new(status: Status) -> Self {
        Self(UpdatePresencePayload {
            activities: Vec::new(),
            afk: false,
            since: None,
            status: gateway_status(status),
        })
    }

    /// プレイ中などのアクティビティを設定します。
    pub fn with_activity(mut self, activity: MinimalActivity) -> Self {
        self.0.activities = vec![activity.into()];
        self
    }
}

fn gateway_status(status: Status) -> Status {
    match status {
        Status::Offline => Status::Invisible,
        other => other,
    }
}

/// 実行中の Bot のステータスを更新するための共有ハンドルです。
#[derive(Clone, Debug)]
pub struct PresenceHandle(watch::Sender<Presence>);

impl PresenceHandle {
    /// 初期状態を指定します。[`crate::Bot::with_presence`] にこのハンドルを渡します。
    pub fn new(initial: Presence) -> Self {
        Self(watch::Sender::new(initial))
    }

    /// 表示したい状態を設定します。短時間に複数回呼ぶと最新の値を送ります。
    /// この呼び出しは Discord の表示が更新されるまで待機しません。
    pub fn set(&self, presence: Presence) {
        self.0.send_if_modified(|current| {
            if *current == presence {
                return false;
            }
            *current = presence;
            true
        });
    }

    /// オンライン状態だけを更新し、設定済みのアクティビティを保ちます。
    pub fn set_status(&self, status: Status) {
        let status = gateway_status(status);
        self.0.send_if_modified(|current| {
            if current.0.status == status {
                return false;
            }
            current.0.status = status;
            true
        });
    }

    /// アクティビティだけを更新します。`None` はアクティビティを消します。
    pub fn set_activity(&self, activity: Option<MinimalActivity>) {
        let activity = activity.map(Into::into);
        self.0.send_if_modified(|current| {
            if current.0.activities.first() == activity.as_ref() {
                return false;
            }
            current.0.activities.clear();
            current.0.activities.extend(activity);
            true
        });
    }

    pub(crate) fn subscribe(&self) -> PresenceUpdates {
        PresenceUpdates {
            desired: self.0.subscribe(),
            next_send: Instant::now(),
            replay: false,
        }
    }
}

pub(crate) struct PresenceUpdates {
    desired: watch::Receiver<Presence>,
    next_send: Instant,
    replay: bool,
}

impl PresenceUpdates {
    pub(crate) fn initial(&mut self) -> UpdatePresencePayload {
        self.desired.borrow_and_update().0.clone()
    }

    pub(crate) fn replay(&mut self) {
        self.replay = true;
    }

    pub(crate) async fn next(&mut self) -> UpdatePresence {
        // 先に間隔を空ける。Gateway イベントによって中断されても、更新を読み落とさない。
        tokio::time::sleep_until(self.next_send).await;
        if !self.replay && self.desired.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
        self.replay = false;
        self.next_send = Instant::now() + MIN_UPDATE_INTERVAL;
        UpdatePresence {
            d: self.desired.borrow_and_update().0.clone(),
            op: OpCode::PresenceUpdate,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn sends_latest_presence_and_replays_it_after_reconnect() {
        let handle = PresenceHandle::new(Presence::new(Status::Online));
        let mut updates = handle.subscribe();
        assert_eq!(updates.initial().status, Status::Online);

        handle.set(Presence::new(Status::Idle));
        handle.set(Presence::new(Status::DoNotDisturb));
        let update = updates.next().await;
        assert_eq!(update.d.status, Status::DoNotDisturb);
        assert!(update.d.activities.is_empty());
        assert!(updates.next_send > Instant::now());

        handle.set(Presence::new(Status::Idle));
        let before_deadline = MIN_UPDATE_INTERVAL - Duration::from_millis(1);
        assert!(
            tokio::time::timeout(before_deadline, updates.next())
                .await
                .is_err()
        );
        handle.set_status(Status::Offline);
        tokio::time::advance(Duration::from_millis(1)).await;
        let update = updates.next().await;
        assert_eq!(update.d.status, Status::Invisible);

        tokio::time::advance(MIN_UPDATE_INTERVAL).await;
        updates.replay();
        assert_eq!(updates.next().await, update);
    }

    #[tokio::test(start_paused = true)]
    async fn partial_updates_preserve_the_other_field_and_skip_duplicates() {
        use twilight_model::gateway::presence::ActivityType;

        let handle = PresenceHandle::new(Presence::new(Status::Offline));
        let mut updates = handle.subscribe();
        assert_eq!(updates.initial().status, Status::Invisible);
        handle.set_status(Status::Invisible);
        handle.set_activity(None);
        assert!(!updates.desired.has_changed().unwrap());

        handle.set_activity(Some(MinimalActivity {
            kind: ActivityType::Playing,
            name: "Akari".into(),
            url: None,
        }));
        handle.set_status(Status::Idle);
        let update = updates.next().await;
        assert_eq!(update.d.status, Status::Idle);
        assert_eq!(update.d.activities[0].name, "Akari");

        handle.set_activity(None);
        let update = updates.next().await;
        assert_eq!(update.d.status, Status::Idle);
        assert!(update.d.activities.is_empty());
    }
}
