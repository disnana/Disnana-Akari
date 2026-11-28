use std::{
    collections::{HashMap, HashSet},
    convert::Infallible,
    future::Future,
    num::{NonZeroU16, NonZeroUsize},
    sync::Arc,
    time::Duration,
};

use tokio::{sync::Notify, time::timeout};
use twilight_gateway::ShardId;
use twilight_http::Client;
use twilight_model::{
    guild::{Guild, Member},
    id::{
        Id,
        marker::{GuildMarker, UserMarker},
    },
};

use super::{MemberCache, MemberCacheHandle};
use crate::{Error, ErrorSource, Result, hooks::Reporter};

const PAGE_MAX: u16 = 1_000;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct BootstrapConfig {
    guilds: HashSet<Id<GuildMarker>>,
    limit: u16,
}

impl BootstrapConfig {
    pub(crate) fn new(
        guilds: impl IntoIterator<Item = Id<GuildMarker>>,
        limit: NonZeroU16,
    ) -> Option<Self> {
        let guilds: HashSet<_> = guilds.into_iter().collect();
        (!guilds.is_empty()).then_some(Self {
            guilds,
            limit: limit.get(),
        })
    }
}

pub(super) struct Bootstrap {
    config: BootstrapConfig,
    worker: BootstrapWorker,
}

impl Bootstrap {
    pub(super) fn new(
        mut config: BootstrapConfig,
        cache: MemberCacheHandle,
        capacity: NonZeroUsize,
    ) -> Self {
        config.limit = config
            .limit
            .min(u16::try_from(capacity.get()).unwrap_or(u16::MAX));
        Self {
            config,
            worker: BootstrapWorker {
                cache,
                wake: Arc::default(),
                progress: Arc::default(),
            },
        }
    }

    pub(super) fn worker(&self) -> BootstrapWorker {
        self.worker.clone()
    }

    pub(super) fn attach_progress(&self, cache: &mut MemberCache) {
        cache.bootstrap.1 = Some(Arc::clone(&self.worker.progress));
    }

    pub(super) fn wake(&self) {
        self.worker.wake.notify_one();
    }

    pub(super) fn schedule(&self, cache: &mut MemberCache, shard: ShardId, guild: &Guild) {
        if !self.config.guilds.contains(&guild.id)
            || cache.order.len() >= cache.capacity.get()
            || guild
                .member_count
                .is_some_and(|total| total <= guild.members.len() as u64)
        {
            return;
        }
        cache.bootstrap.0.insert(
            guild.id,
            Request {
                job: Job {
                    guild: guild.id,
                    shard,
                    limit: self.config.limit.min(PAGE_MAX),
                    after: None,
                    generation: Arc::new(()),
                },
                remaining: self.config.limit,
                touch_limit: usize::from(self.config.limit),
                running: false,
                touched: HashSet::new(),
            },
        );
        self.wake();
        cache.bootstrap.notify_progress();
    }
}

#[derive(Clone)]
struct Job {
    guild: Id<GuildMarker>,
    shard: ShardId,
    limit: u16,
    after: Option<Id<UserMarker>>,
    generation: Arc<()>,
}

struct Request {
    job: Job,
    remaining: u16,
    touch_limit: usize,
    running: bool,
    touched: HashSet<Id<UserMarker>>,
}

impl Request {
    fn next_page(mut self, received: usize, last: Option<Id<UserMarker>>) -> Option<Self> {
        if received != usize::from(self.job.limit) {
            return None;
        }
        let after = last.filter(|id| Some(*id) != self.job.after)?;
        self.remaining -= self.job.limit;
        if self.remaining == 0 {
            return None;
        }
        self.job.limit = self.remaining.min(PAGE_MAX);
        self.job.after = Some(after);
        self.job.generation = Arc::new(());
        self.running = false;
        self.touched.retain(|id| *id > after);
        Some(self)
    }
}

#[derive(Default)]
pub(super) struct Pending(HashMap<Id<GuildMarker>, Request>, Option<Arc<Notify>>);

impl Pending {
    fn notify_progress(&self) {
        if let Some(progress) = &self.1 {
            progress.notify_one();
        }
    }

    pub(super) fn cancel_shard(&mut self, shard: ShardId) {
        let count = self.0.len();
        self.0.retain(|_, request| request.job.shard != shard);
        if count != self.0.len() {
            self.notify_progress();
        }
    }

    pub(super) fn pause_shard(&mut self, shard: ShardId) {
        for request in self
            .0
            .values_mut()
            .filter(|request| request.job.shard == shard)
        {
            if request.running {
                // 再開後は最後に保存できたページの次から始める。
                request.job.generation = Arc::new(());
                request.running = false;
            }
        }
    }

    pub(super) fn cancel_guild(&mut self, shard: ShardId, guild: Id<GuildMarker>) {
        if self
            .0
            .get(&guild)
            .is_some_and(|request| request.job.shard == shard)
        {
            self.0.remove(&guild);
            self.notify_progress();
        }
    }

    pub(super) fn touch(&mut self, shard: ShardId, guild: Id<GuildMarker>, user: Id<UserMarker>) {
        if let Some(request) = self.0.get_mut(&guild)
            && request.job.shard == shard
            && request.job.after.is_none_or(|after| user > after)
            && request.touched.insert(user)
            && request.touched.len() > request.touch_limit
        {
            // 変化の多いサーバーでも、取得候補と同じ件数までしか競合履歴を保存しません。
            self.0.remove(&guild);
            self.notify_progress();
        }
    }

    fn claim(&mut self, suspended: &HashSet<ShardId>) -> Option<Job> {
        let request = self
            .0
            .values_mut()
            .find(|request| !request.running && !suspended.contains(&request.job.shard))?;
        request.running = true;
        Some(request.job.clone())
    }

    fn complete(&mut self, job: &Job) -> Option<Request> {
        let request = self.0.get(&job.guild)?;
        if !request.running || !Arc::ptr_eq(&request.job.generation, &job.generation) {
            return None;
        }
        self.0.remove(&job.guild)
    }
}

#[derive(Clone)]
pub(crate) struct BootstrapWorker {
    cache: MemberCacheHandle,
    wake: Arc<Notify>,
    progress: Arc<Notify>,
}

impl BootstrapWorker {
    pub(crate) async fn changed(&self) {
        self.progress.notified().await;
    }

    pub(crate) fn has_pending(&self) -> bool {
        self.cache
            .0
            .read()
            .ok()
            .and_then(|cache| cache.as_ref().map(|state| !state.bootstrap.0.is_empty()))
            .unwrap_or_default()
    }

    pub(crate) async fn drive<F: Future>(
        worker: Option<Self>,
        http: &Client,
        reporter: &Reporter,
        drive: F,
    ) -> F::Output {
        let Some(worker) = worker else {
            return drive.await;
        };
        tokio::select! {
            biased;
            result = drive => result,
            never = worker.run(http, reporter) => match never {},
        }
    }

    async fn run(self, http: &Client, reporter: &Reporter) -> Infallible {
        loop {
            let Some(job) = self.claim() else {
                self.wake.notified().await;
                continue;
            };
            let response = timeout(REQUEST_TIMEOUT, fetch(http, &job))
                .await
                .map_err(Error::handler)
                .and_then(|response| response);
            match response {
                Ok(members) => {
                    self.complete(&job, members);
                }
                Err(error) => {
                    if self.complete(&job, Vec::new()) {
                        reporter.report(ErrorSource::Event, Some("member_cache_bootstrap"), &error);
                    }
                }
            }
        }
    }

    fn claim(&self) -> Option<Job> {
        let mut cache = self.cache.0.write().ok()?;
        let cache = cache.as_mut()?;
        if cache.order.len() >= cache.capacity.get() {
            // この追加取得は空いているキャッシュだけに使い、満杯時は次のサーバー参加を待ちます。
            if !cache.bootstrap.0.is_empty() {
                cache.bootstrap.0.clear();
                cache.bootstrap.notify_progress();
            }
            return None;
        }
        cache.bootstrap.claim(&cache.suspended)
    }

    fn complete(&self, job: &Job, mut members: Vec<Member>) -> bool {
        let Ok(mut cache) = self.cache.0.write() else {
            return false;
        };
        let Some(cache) = cache.as_mut() else {
            return false;
        };
        let Some(request) = cache.bootstrap.complete(job) else {
            return false;
        };
        members.truncate(usize::from(job.limit));
        let received = members.len();
        let mut last = job.after;
        for member in members {
            if cache.order.len() >= cache.capacity.get() {
                break;
            }
            let user = member.user.id;
            if job.after.is_some_and(|after| user <= after) {
                continue;
            }
            last = last.max(Some(user));
            if !request.touched.contains(&user) && cache.entry_mut((job.guild, user)).is_none() {
                cache.insert_snapshot(job.shard, job.guild, Arc::new(member));
            }
        }
        if cache.order.len() < cache.capacity.get()
            && let Some(next) = request.next_page(received, last)
        {
            cache.bootstrap.0.insert(job.guild, next);
        }
        cache.bootstrap.notify_progress();
        true
    }
}

async fn fetch(http: &Client, job: &Job) -> Result<Vec<Member>> {
    let mut request = http.guild_members(job.guild).limit(job.limit);
    if let Some(after) = job.after {
        request = request.after(after);
    }
    Ok(request.await?.models().await?)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::{TcpListener, TcpStream},
        sync::oneshot,
    };
    use twilight_gateway::{Event, EventTypeFlags, Intents};

    use super::*;
    use crate::{
        Events,
        member_cache::{MemberCacheRoute, MemberCacheRuntime},
        message_events::{GatewayDecodeOptions, IncomingEvent, decode_text_with_options},
    };

    const GUILD: Id<GuildMarker> = Id::new(2);

    fn make_runtime(capacity: usize) -> (MemberCacheRuntime, MemberCacheHandle, BootstrapWorker) {
        let handle = MemberCacheHandle::new();
        let runtime = MemberCacheRuntime::prepare(
            capacity,
            &Events::new(),
            Intents::GUILDS | Intents::GUILD_MEMBERS,
            Some(handle.clone()),
            BootstrapConfig::new([GUILD, GUILD], NonZeroU16::MAX),
        )
        .unwrap()
        .unwrap();
        let worker = runtime.bootstrap_worker().unwrap();
        (runtime, handle, worker)
    }

    fn member(id: &str, nick: &str) -> Value {
        json!({
            "user": {"id": id, "username": "member", "discriminator": "0"},
            "deaf": false, "mute": false, "flags": 0, "roles": [], "nick": nick
        })
    }

    fn create(guild: &str, members: Vec<Value>, count: usize) -> Value {
        json!({
            "id": guild, "owner_id":"1", "name":"seed", "afk_timeout":60,
            "default_message_notifications":0, "explicit_content_filter":0, "features":[],
            "mfa_level":0, "nsfw_level":0, "preferred_locale":"ja", "roles":[],
            "system_channel_flags":0, "premium_progress_bar_enabled":false, "verification_level":0,
            "member_count": count, "members": members
        })
    }

    fn observe(route: MemberCacheRoute<'_>, kind: &str, payload: Value, types: EventTypeFlags) {
        let incoming = decode_text_with_options(
            json!({"op":0,"t":kind,"s":2,"d":payload}).to_string(),
            types,
            GatewayDecodeOptions {
                capture_member_fields: true,
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        match incoming {
            IncomingEvent::Standard(event) => {
                route.observe(&event);
            }
            IncomingEvent::MemberUpdate(event, fields) => {
                route.update(&event, fields);
            }
            _ => panic!("expected guild or member event"),
        }
    }

    async fn accept_request(listener: &TcpListener, path: &str) -> BufReader<TcpStream> {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = BufReader::new(socket);
        let mut line = String::new();
        socket.read_line(&mut line).await.unwrap();
        assert_eq!(line.trim_end(), format!("GET {path} HTTP/1.1"));
        while !line.trim_end().is_empty() {
            line.clear();
            assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
        }
        socket
    }

    async fn send_members(socket: &mut BufReader<TcpStream>, body: Vec<Value>) {
        let body = serde_json::to_string(&body).unwrap();
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.get_mut().write_all(reply.as_bytes()).await.unwrap();
    }

    #[test]
    fn replacement_guild_and_disconnected_shard_cancel_in_flight_pages() {
        let (runtime, handle, worker) = make_runtime(4);
        let route = runtime.shard(ShardId::ONE);
        let types = runtime.event_types();
        observe(route, "GUILD_CREATE", create("3", vec![], 100), types);
        observe(
            route,
            "GUILD_CREATE",
            create("2", vec![member("6", "live")], 1),
            types,
        );
        assert!(worker.claim().is_none());
        observe(route, "GUILD_CREATE", create("2", vec![], 100), types);
        let first = worker.claim().unwrap();
        assert_eq!(first.limit, 4);
        observe(route, "GUILD_CREATE", create("2", vec![], 100), types);
        assert!(!worker.complete(
            &first,
            vec![serde_json::from_value(member("8", "old")).unwrap()]
        ));
        let second = worker.claim().unwrap();
        runtime.suspend_shard(ShardId::ONE);
        assert!(worker.claim().is_none());
        route.observe(&Event::Resumed);
        assert!(!worker.complete(
            &second,
            vec![serde_json::from_value(member("8", "old")).unwrap()]
        ));
        assert!(handle.get(GUILD, Id::new(8)).is_none());
        let current = worker.claim().unwrap();
        assert!(worker.complete(
            &current,
            vec![serde_json::from_value(member("8", "current")).unwrap()]
        ));
        assert!(handle.get(GUILD, Id::new(8)).is_some());
    }

    #[tokio::test]
    async fn rest_worker_fills_only_untouched_empty_slots_and_keeps_driving() {
        let (runtime, handle, worker) = make_runtime(4);
        let route = runtime.shard(ShardId::ONE);
        let types = runtime.event_types();
        observe(
            route,
            "GUILD_CREATE",
            create("2", vec![member("6", "gateway")], 100),
            types,
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http = Client::builder()
            .token("fixture".to_owned())
            .proxy(listener.local_addr().unwrap().to_string(), true)
            .build();
        let (requested, when_requested) = oneshot::channel();
        let (changed, when_changed) = oneshot::channel();
        let reporter = Reporter::default();
        let server = async {
            let mut socket = accept_request(&listener, "/api/v10/guilds/2/members?limit=4").await;
            requested.send(()).unwrap();
            when_changed.await.unwrap();
            send_members(
                &mut socket,
                vec![
                    member("6", "rest"),
                    member("7", "updated"),
                    member("8", "left"),
                    member("9", "loaded"),
                    member("10", "too many"),
                ],
            )
            .await;
        };
        let driving = async {
            when_requested.await.unwrap();
            observe(
                route,
                "GUILD_MEMBER_UPDATE",
                json!({"guild_id":"2","user": member("7", "gateway")["user"], "roles":[],"nick":"now"}),
                types,
            );
            observe(
                route,
                "GUILD_MEMBER_REMOVE",
                json!({"guild_id":"2","user": member("8", "gateway")["user"]}),
                types,
            );
            changed.send(()).unwrap();
            while handle.get(GUILD, Id::new(9)).is_none() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        };
        timeout(Duration::from_secs(5), async {
            tokio::join!(
                server,
                BootstrapWorker::drive(Some(worker), &http, &reporter, driving)
            );
        })
        .await
        .unwrap();
        assert_eq!(
            handle.get(GUILD, Id::new(6)).unwrap().nick.as_deref(),
            Some("gateway")
        );
        assert!(handle.get(GUILD, Id::new(7)).is_none());
        assert!(handle.get(GUILD, Id::new(8)).is_none());
        assert!(handle.get(GUILD, Id::new(9)).is_some());
        assert!(handle.get(GUILD, Id::new(10)).is_none());
    }

    #[tokio::test]
    async fn paginated_bootstrap_resumes_from_last_completed_page() {
        let (runtime, handle, worker) = make_runtime(1_002);
        let route = runtime.shard(ShardId::ONE);
        let types = runtime.event_types();
        observe(route, "GUILD_CREATE", create("2", vec![], 2_000), types);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http = Client::builder()
            .token("fixture".to_owned())
            .proxy(listener.local_addr().unwrap().to_string(), true)
            .build();
        let (requested, when_requested) = oneshot::channel();
        let (resumed, when_resumed) = oneshot::channel();
        let reporter = Reporter::default();
        let server = async {
            let mut first = accept_request(&listener, "/api/v10/guilds/2/members?limit=1000").await;
            let descending = (1..=1_000)
                .rev()
                .map(|id| member(&id.to_string(), "first"))
                .collect();
            send_members(&mut first, descending).await;
            let path = "/api/v10/guilds/2/members?after=1000&limit=2";
            let mut obsolete = accept_request(&listener, path).await;
            requested.send(()).unwrap();
            when_resumed.await.unwrap();
            send_members(&mut obsolete, vec![member("1002", "obsolete")]).await;
            let mut current = accept_request(&listener, path).await;
            send_members(
                &mut current,
                vec![member("1001", "removed"), member("1002", "current")],
            )
            .await;
        };
        let driving = async {
            when_requested.await.unwrap();
            runtime.suspend_shard(ShardId::ONE);
            assert!(handle.get(GUILD, Id::new(1)).is_none());
            observe(
                route,
                "GUILD_MEMBER_REMOVE",
                json!({"guild_id":"2","user": member("1001", "removed")["user"]}),
                types,
            );
            route.observe(&Event::Resumed);
            resumed.send(()).unwrap();
            loop {
                if let Some(member) = handle.get(GUILD, Id::new(1_002)) {
                    assert_eq!(member.nick.as_deref(), Some("current"));
                    break;
                }
                tokio::task::yield_now().await;
            }
        };
        timeout(Duration::from_secs(5), async {
            tokio::join!(
                server,
                BootstrapWorker::drive(Some(worker), &http, &reporter, driving)
            );
        })
        .await
        .unwrap();
        assert_eq!(handle.guild_members(GUILD).len(), 1_001);
        assert!(handle.get(GUILD, Id::new(1_001)).is_none());
    }
}
