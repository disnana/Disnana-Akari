use std::{
    collections::HashMap,
    future::pending,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::{
    sync::{Notify, watch},
    time::{Instant, sleep_until},
};
use twilight_gateway::{Event, EventTypeFlags};
use twilight_model::{
    gateway::{ShardId, payload::incoming::GuildCreate},
    id::{Id, marker::GuildMarker},
};

pub(crate) const STARTUP_EVENTS: EventTypeFlags = EventTypeFlags::READY
    .union(EventTypeFlags::RESUMED)
    .union(EventTypeFlags::GUILD_CREATE)
    .union(EventTypeFlags::GUILD_DELETE);

/// Bot が最初のサーバー同期を終えるまで待つためのハンドルです。
///
/// 起動結果は一度だけ確定します。確定後に切断や再接続があってもこの結果は変わりません。
#[derive(Clone, Debug)]
pub struct StartupReadiness(watch::Receiver<Publication>);

impl StartupReadiness {
    /// 確定済みの起動結果です。まだ確定していない場合は [None] です。
    pub fn snapshot(&self) -> Option<Arc<StartupReport>> {
        match &*self.0.borrow() {
            Publication::Report(report) => Some(Arc::clone(report)),
            Publication::Pending | Publication::Closed => None,
        }
    }

    /// 同期の完了または制限時間到達まで待ちます。
    ///
    /// 確定前に Bot が終了または破棄された場合は [None] を返します。
    pub async fn wait(&self) -> Option<Arc<StartupReport>> {
        let mut subscription = self.0.clone();
        let publication = subscription
            .wait_for(|value| !matches!(value, Publication::Pending))
            .await
            .ok()?;
        match &*publication {
            Publication::Report(report) => Some(Arc::clone(report)),
            Publication::Pending | Publication::Closed => None,
        }
    }
}

/// 全 Gateway シャードについて、開始時の同期をいったん確認した結果です。
#[derive(Debug)]
pub struct StartupReport {
    shards: Vec<StartupShardReport>,
    timed_out: bool,
    warmup: StartupWarmup,
}

impl StartupReport {
    /// 起動対象のシャードをシャード番号順に返します。
    pub fn shards(&self) -> &[StartupShardReport] {
        &self.shards
    }

    /// シャード、サーバー、または追加取得の完了前に制限時間へ達したかを返します。
    pub const fn timed_out(&self) -> bool {
        self.timed_out
    }

    /// メンバーキャッシュの追加取得を待った結果です。
    pub const fn warmup(&self) -> StartupWarmup {
        self.warmup
    }
}

/// あるシャードの最初のサーバー同期について分かっている状態です。
#[derive(Debug)]
pub struct StartupShardReport {
    shard: ShardId,
    received_ready: bool,
    connected: bool,
    reliable: bool,
    available: usize,
    unavailable: Vec<Id<GuildMarker>>,
    unresolved: Vec<Id<GuildMarker>>,
    removed: Vec<Id<GuildMarker>>,
}

impl StartupShardReport {
    /// 対象の Gateway シャードです。
    pub const fn shard(&self) -> ShardId {
        self.shard
    }

    /// このシャードから有効な Ready を一度でも受け取ったかを返します。
    pub const fn received_ready(&self) -> bool {
        self.received_ready
    }

    /// 結果が確定した時点で、このシャードの Ready または Resume が有効だったかを返します。
    pub const fn connected(&self) -> bool {
        self.connected
    }

    /// 最新の Ready 以降に、初期同期に関わる通知を欠落していないかを返します。
    pub const fn reliable(&self) -> bool {
        self.reliable
    }

    /// 起動時に利用できると通知されたサーバー数です。
    pub const fn available_guilds(&self) -> usize {
        self.available
    }

    /// 利用不能と通知された起動時のサーバーの一覧です。
    pub fn unavailable_guilds(&self) -> &[Id<GuildMarker>] {
        &self.unavailable
    }

    /// Ready に含まれ、結果確定までに状態を確認できなかったサーバーの一覧です。
    pub fn unresolved_guilds(&self) -> &[Id<GuildMarker>] {
        &self.unresolved
    }

    /// 初期同期中に Bot が離脱したサーバーの一覧です。
    pub fn removed_guilds(&self) -> &[Id<GuildMarker>] {
        &self.removed
    }
}

/// 起動時にメンバーキャッシュの追加取得を待った状態です。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum StartupWarmup {
    /// 起動結果の確定時にメンバーキャッシュの追加取得を待たなかった。
    NotRequested,
    /// 確定時に追加取得の未完了ジョブがありませんでした。
    ///
    /// 取得結果の完全性や、各ジョブの成功を保証する値ではありません。
    Settled,
    /// 制限時間に達した時点で未完了の追加取得がありました。
    PendingAtDeadline,
}

#[derive(Debug)]
enum Publication {
    Pending,
    Report(Arc<StartupReport>),
    Closed,
}

enum Progress {
    Waiting { wait_bootstrap: bool },
    Finished(StartupReport),
    Unavailable,
}

#[derive(Debug)]
pub(crate) struct StartupTracker {
    timeout: Duration,
    wait_for_bootstrap: bool,
    shards: Mutex<HashMap<ShardId, ShardStartup>>,
    publication: watch::Sender<Publication>,
    changed: Notify,
}

impl StartupTracker {
    pub(crate) fn new(timeout: Duration, wait_for_bootstrap: bool) -> (Self, StartupReadiness) {
        let (publication, subscription) = watch::channel(Publication::Pending);
        (
            Self {
                timeout,
                wait_for_bootstrap,
                shards: Mutex::default(),
                publication,
                changed: Notify::new(),
            },
            StartupReadiness(subscription),
        )
    }

    pub(crate) const fn waits_for_bootstrap(&self) -> bool {
        self.wait_for_bootstrap
    }

    pub(crate) fn initialize(&mut self, shards: impl IntoIterator<Item = ShardId>) {
        if self.is_finished() {
            return;
        }
        if let Ok(state) = self.shards.get_mut() {
            state.extend(shards.into_iter().map(|id| (id, ShardStartup::default())));
        }
        self.changed.notify_one();
    }

    pub(crate) fn observe(&self, shard: ShardId, event: &Event) {
        if !matches!(
            event,
            Event::Ready(_) | Event::Resumed | Event::GuildCreate(_) | Event::GuildDelete(_)
        ) {
            return;
        }
        self.update(shard, |state| match event {
            Event::Ready(ready) => {
                if ready.shard.is_some_and(|id| id != shard) {
                    return state.invalidate();
                }
                state.received_ready = true;
                state.connected = true;
                state.reliable = true;
                state.guilds = ready
                    .guilds
                    .iter()
                    .map(|guild| (guild.id, GuildStartup::Pending))
                    .collect();
                state.unresolved = state.guilds.len();
                true
            }
            Event::Resumed if state.received_ready => {
                let changed = !state.connected;
                state.connected = true;
                changed
            }
            Event::GuildCreate(create) => {
                let status = if matches!(create.as_ref(), GuildCreate::Available(_)) {
                    GuildStartup::Available
                } else {
                    GuildStartup::Unavailable
                };
                state.resolve(create.id(), status)
            }
            Event::GuildDelete(delete) => {
                let status = if delete.unavailable == Some(true) {
                    GuildStartup::Unavailable
                } else {
                    GuildStartup::Removed
                };
                state.resolve(delete.id, status)
            }
            _ => false,
        });
    }

    pub(crate) fn disconnect(&self, shard: ShardId) {
        self.update(shard, |state| std::mem::take(&mut state.connected));
    }

    pub(crate) fn invalidate(&self, shard: ShardId) {
        self.update(shard, ShardStartup::invalidate);
    }

    pub(crate) fn close(&self) {
        self.publish(Publication::Closed);
    }

    fn update(&self, shard: ShardId, update: impl FnOnce(&mut ShardStartup) -> bool) {
        if self.is_finished() {
            return;
        }
        let changed = self
            .shards
            .lock()
            .ok()
            .and_then(|mut shards| shards.get_mut(&shard).map(update))
            .unwrap_or(false);
        if changed {
            self.changed.notify_one();
        }
    }

    fn is_finished(&self) -> bool {
        !matches!(*self.publication.borrow(), Publication::Pending)
    }

    fn publish(&self, value: Publication) {
        let published = self.publication.send_if_modified(|publication| {
            if !matches!(publication, Publication::Pending) {
                return false;
            }
            *publication = value;
            true
        });
        if published {
            // 確定結果には不要な初期同期の記録を解放する。
            self.shards
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
            self.changed.notify_one();
        }
    }

    pub(crate) async fn run(
        &self,
        bootstrap_pending: impl Fn() -> bool,
        bootstrap_changed: impl AsyncFn(),
    ) {
        let deadline = Instant::now().checked_add(self.timeout);
        while !self.is_finished() {
            let bootstrap_pending = self.wait_for_bootstrap && bootstrap_pending();
            let expired = deadline.is_some_and(|deadline| Instant::now() >= deadline);
            let wait_bootstrap = match self.progress(expired, bootstrap_pending) {
                Progress::Waiting { wait_bootstrap } => wait_bootstrap,
                Progress::Finished(report) => {
                    self.publish(Publication::Report(Arc::new(report)));
                    return;
                }
                Progress::Unavailable => {
                    self.close();
                    return;
                }
            };
            tokio::select! {
                () = self.changed.notified() => {}
                () = wait_deadline(deadline) => {}
                () = bootstrap_changed(), if wait_bootstrap => {}
            }
        }
    }

    fn progress(&self, expired: bool, bootstrap_pending: bool) -> Progress {
        let Ok(shards) = self.shards.lock() else {
            return Progress::Unavailable;
        };
        let resolved = !shards.is_empty() && shards.values().all(ShardStartup::is_resolved);
        let timed_out = !resolved || bootstrap_pending;
        if timed_out && !expired {
            return Progress::Waiting {
                wait_bootstrap: resolved && bootstrap_pending,
            };
        }
        let warmup = if !self.wait_for_bootstrap {
            StartupWarmup::NotRequested
        } else if bootstrap_pending {
            StartupWarmup::PendingAtDeadline
        } else {
            StartupWarmup::Settled
        };
        let mut reports: Vec<_> = shards.iter().map(|(&id, shard)| shard.report(id)).collect();
        reports.sort_unstable_by_key(|report| report.shard.number());
        Progress::Finished(StartupReport {
            shards: reports,
            timed_out,
            warmup,
        })
    }
}

async fn wait_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => sleep_until(deadline).await,
        None => pending().await,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GuildStartup {
    Pending,
    Available,
    Unavailable,
    Removed,
}

#[derive(Debug, Default)]
struct ShardStartup {
    received_ready: bool,
    connected: bool,
    reliable: bool,
    unresolved: usize,
    guilds: HashMap<Id<GuildMarker>, GuildStartup>,
}

impl ShardStartup {
    fn invalidate(&mut self) -> bool {
        if !self.reliable {
            return false;
        }
        self.reliable = false;
        self.unresolved = self.guilds.len();
        for status in self.guilds.values_mut() {
            *status = GuildStartup::Pending;
        }
        true
    }

    fn resolve(&mut self, guild: Id<GuildMarker>, status: GuildStartup) -> bool {
        if !self.reliable {
            return false;
        }
        let Some(current) = self.guilds.get_mut(&guild) else {
            return false;
        };
        if *current == GuildStartup::Removed {
            return false;
        }
        let became_resolved = *current == GuildStartup::Pending;
        if became_resolved {
            self.unresolved -= 1;
        }
        *current = status;
        became_resolved && self.unresolved == 0
    }

    fn is_resolved(&self) -> bool {
        self.received_ready && self.connected && self.reliable && self.unresolved == 0
    }

    fn report(&self, shard: ShardId) -> StartupShardReport {
        let mut report = StartupShardReport {
            shard,
            received_ready: self.received_ready,
            connected: self.connected,
            reliable: self.reliable,
            available: 0,
            unavailable: Vec::new(),
            unresolved: Vec::new(),
            removed: Vec::new(),
        };
        for (&guild, status) in &self.guilds {
            match status {
                GuildStartup::Pending => report.unresolved.push(guild),
                GuildStartup::Available => report.available += 1,
                GuildStartup::Unavailable => report.unavailable.push(guild),
                GuildStartup::Removed => report.removed.push(guild),
            }
        }
        report.unresolved.sort_unstable();
        report.unavailable.sort_unstable();
        report.removed.sort_unstable();
        report
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use serde_json::json;
    use twilight_model::gateway::payload::incoming::GuildDelete;

    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(5);
    const FIRST: ShardId = ShardId::new(0, 2);
    const SECOND: ShardId = ShardId::new(1, 2);

    fn ready(shard: ShardId, guilds: &[u64]) -> Event {
        let guilds = guilds
            .iter()
            .map(|id| json!({"id":id.to_string(),"unavailable":true}))
            .collect::<Vec<_>>();
        Event::Ready(serde_json::from_value(json!({
            "application":{"flags":0,"id":"2"}, "guilds":guilds, "session_id":"session",
            "resume_gateway_url":"wss://gateway.discord.gg", "v":10, "shard":shard,
            "user":{"id":"2","username":"akari","discriminator":"0","bot":true,"mfa_enabled":false}
        })).unwrap())
    }

    fn available(guild: u64) -> Event {
        Event::GuildCreate(Box::new(serde_json::from_value(json!({
            "id":guild.to_string(), "owner_id":"2", "name":"fixture", "afk_timeout":60,
            "default_message_notifications":0, "explicit_content_filter":0, "features":[],
            "mfa_level":0, "nsfw_level":0, "preferred_locale":"ja", "roles":[],
            "system_channel_flags":0, "premium_progress_bar_enabled":false, "verification_level":0
        })).unwrap()))
    }

    fn unavailable(guild: u64) -> Event {
        Event::GuildCreate(Box::new(
            serde_json::from_value(json!({
                "id":guild.to_string(), "unavailable":true
            }))
            .unwrap(),
        ))
    }

    fn delete(guild: u64, unavailable: Option<bool>) -> Event {
        Event::GuildDelete(GuildDelete {
            id: Id::new(guild),
            unavailable,
        })
    }

    #[tokio::test(start_paused = true)]
    async fn all_shards_and_initial_guild_resolution_are_required() {
        let (mut tracker, handle) = StartupTracker::new(TIMEOUT, false);
        tracker.initialize([SECOND, FIRST]);
        let tracker = Arc::new(tracker);
        let driver = tokio::spawn({
            let tracker = Arc::clone(&tracker);
            async move { tracker.run(|| false, async || pending().await).await }
        });
        tracker.observe(FIRST, &ready(FIRST, &[4, 3, 1, 1]));
        tracker.observe(FIRST, &available(99));
        tracker.observe(FIRST, &available(1));
        tracker.observe(FIRST, &unavailable(4));
        tracker.observe(FIRST, &delete(3, None));
        tokio::task::yield_now().await;
        assert!(handle.snapshot().is_none());

        tracker.disconnect(FIRST);
        tracker.observe(SECOND, &ready(SECOND, &[]));
        tokio::task::yield_now().await;
        assert!(handle.snapshot().is_none());
        tracker.observe(FIRST, &Event::Resumed);
        let report = handle.wait().await.unwrap();
        assert!(!report.timed_out());
        assert_eq!(report.warmup(), StartupWarmup::NotRequested);
        let first = &report.shards()[0];
        assert_eq!(first.shard(), FIRST);
        assert!(first.received_ready() && first.connected() && first.reliable());
        assert_eq!(first.available_guilds(), 1);
        assert_eq!(first.unavailable_guilds(), &[Id::new(4)]);
        assert_eq!(first.removed_guilds(), &[Id::new(3)]);
        assert!(first.unresolved_guilds().is_empty());
        assert_eq!(report.shards()[1].shard(), SECOND);
        tracker.close();
        assert!(Arc::ptr_eq(&report, &handle.snapshot().unwrap()));
        driver.await.unwrap();
        assert!(tracker.shards.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn deadline_reports_missing_and_disconnected_shards_and_unresolved_guilds() {
        let (mut tracker, handle) = StartupTracker::new(TIMEOUT, false);
        tracker.initialize([FIRST, SECOND]);
        let tracker = Arc::new(tracker);
        let driver = tokio::spawn({
            let tracker = Arc::clone(&tracker);
            async move { tracker.run(|| false, async || pending().await).await }
        });
        tracker.observe(FIRST, &ready(FIRST, &[1, 2, 3]));
        tracker.observe(FIRST, &delete(3, Some(true)));
        tracker.disconnect(FIRST);
        tokio::task::yield_now().await;
        let report = handle.wait().await.unwrap();
        assert!(report.timed_out());
        let first = &report.shards()[0];
        assert!(first.received_ready() && first.reliable());
        assert!(!first.connected());
        assert_eq!(first.unavailable_guilds(), &[Id::new(3)]);
        assert_eq!(first.unresolved_guilds(), &[Id::new(1), Id::new(2)]);
        assert!(!report.shards()[1].received_ready());
        driver.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn malformed_initial_evidence_is_not_restored_by_resume() {
        let (mut tracker, handle) = StartupTracker::new(TIMEOUT, false);
        tracker.initialize([ShardId::ONE]);
        let tracker = Arc::new(tracker);
        let driver = tokio::spawn({
            let tracker = Arc::clone(&tracker);
            async move { tracker.run(|| false, async || pending().await).await }
        });
        tracker.observe(ShardId::ONE, &ready(ShardId::ONE, &[1]));
        tracker.invalidate(ShardId::ONE);
        tracker.observe(ShardId::ONE, &Event::Resumed);
        tracker.observe(ShardId::ONE, &available(1));
        let report = handle.wait().await.unwrap();
        let only = report.shards().first().unwrap();
        assert!(report.timed_out());
        assert!(only.received_ready() && only.connected());
        assert!(!only.reliable());
        assert_eq!(only.unresolved_guilds(), &[Id::new(1)]);
        driver.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn optional_bootstrap_is_bounded_by_the_same_startup_deadline() {
        for settled in [true, false] {
            let pending = Arc::new(AtomicBool::new(true));
            let changed = Arc::new(Notify::new());
            let (mut tracker, handle) = StartupTracker::new(TIMEOUT, true);
            tracker.initialize([ShardId::ONE]);
            let tracker = Arc::new(tracker);
            let driver = tokio::spawn({
                let tracker = Arc::clone(&tracker);
                let pending = Arc::clone(&pending);
                let changed = Arc::clone(&changed);
                async move {
                    tracker
                        .run(
                            || pending.load(Ordering::Relaxed),
                            async || changed.notified().await,
                        )
                        .await;
                }
            });
            tracker.observe(ShardId::ONE, &ready(ShardId::ONE, &[]));
            tokio::task::yield_now().await;
            assert!(handle.snapshot().is_none());
            if settled {
                pending.store(false, Ordering::Relaxed);
                changed.notify_one();
            }
            let report = handle.wait().await.unwrap();
            assert_eq!(report.timed_out(), !settled);
            assert_eq!(
                report.warmup(),
                if settled {
                    StartupWarmup::Settled
                } else {
                    StartupWarmup::PendingAtDeadline
                }
            );
            driver.await.unwrap();
        }
    }

    #[tokio::test(start_paused = true)]
    async fn drop_and_close_cancel_the_await_without_late_publication() {
        let (unstarted, handle) = StartupTracker::new(TIMEOUT, false);
        drop(unstarted);
        assert!(handle.wait().await.is_none());

        let (mut tracker, handle) = StartupTracker::new(TIMEOUT, false);
        tracker.initialize([ShardId::ONE]);
        let tracker = Arc::new(tracker);
        let driver = tokio::spawn({
            let tracker = Arc::clone(&tracker);
            async move { tracker.run(|| false, async || pending().await).await }
        });
        tokio::task::yield_now().await;
        tracker.close();
        tracker.observe(ShardId::ONE, &ready(ShardId::ONE, &[]));
        assert!(handle.wait().await.is_none());
        driver.await.unwrap();
        assert!(handle.snapshot().is_none());
        assert!(tracker.shards.lock().unwrap().is_empty());

        let (mut tracker, handle) = StartupTracker::new(TIMEOUT, false);
        tracker.initialize([ShardId::ONE]);
        tracker.observe(ShardId::ONE, &ready(ShardId::ONE, &[]));
        let Progress::Finished(already_computed) = tracker.progress(false, false) else {
            panic!("expected an already computed startup report");
        };
        tracker.close();
        tracker.publish(Publication::Report(Arc::new(already_computed)));
        assert!(handle.wait().await.is_none());
    }
}
