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
            limit: limit.get().min(PAGE_MAX),
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
            },
        }
    }

    pub(super) fn worker(&self) -> BootstrapWorker {
        self.worker.clone()
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
                    limit: self.config.limit,
                    generation: Arc::new(()),
                },
                running: false,
                touched: HashSet::new(),
            },
        );
        self.wake();
    }
}

#[derive(Clone)]
struct Job {
    guild: Id<GuildMarker>,
    shard: ShardId,
    limit: u16,
    generation: Arc<()>,
}

struct Request {
    job: Job,
    running: bool,
    touched: HashSet<Id<UserMarker>>,
}

#[derive(Default)]
pub(super) struct Pending(HashMap<Id<GuildMarker>, Request>);

impl Pending {
    pub(super) fn cancel_shard(&mut self, shard: ShardId) {
        self.0.retain(|_, request| request.job.shard != shard);
    }

    pub(super) fn cancel_guild(&mut self, shard: ShardId, guild: Id<GuildMarker>) {
        if self
            .0
            .get(&guild)
            .is_some_and(|request| request.job.shard == shard)
        {
            self.0.remove(&guild);
        }
    }

    pub(super) fn touch(&mut self, shard: ShardId, guild: Id<GuildMarker>, user: Id<UserMarker>) {
        if let Some(request) = self.0.get_mut(&guild)
            && request.job.shard == shard
            && request.touched.insert(user)
            && request.touched.len() > usize::from(request.job.limit)
        {
            // 変化の多いサーバーでも、取得候補と同じ件数までしか競合履歴を保存しません。
            self.0.remove(&guild);
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
        if !Arc::ptr_eq(&request.job.generation, &job.generation) {
            return None;
        }
        self.0.remove(&job.guild)
    }
}

#[derive(Clone)]
pub(crate) struct BootstrapWorker {
    cache: MemberCacheHandle,
    wake: Arc<Notify>,
}

impl BootstrapWorker {
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
            cache.bootstrap.0.clear();
            return None;
        }
        cache.bootstrap.claim(&cache.suspended)
    }

    fn complete(&self, job: &Job, members: Vec<Member>) -> bool {
        let Ok(mut cache) = self.cache.0.write() else {
            return false;
        };
        let Some(cache) = cache.as_mut() else {
            return false;
        };
        let Some(request) = cache.bootstrap.complete(job) else {
            return false;
        };
        for member in members.into_iter().take(usize::from(job.limit)) {
            if cache.order.len() >= cache.capacity.get() {
                break;
            }
            if !request.touched.contains(&member.user.id)
                && cache.entry_mut((job.guild, member.user.id)).is_none()
            {
                cache.insert_snapshot(job.shard, job.guild, Arc::new(member));
            }
        }
        true
    }
}

async fn fetch(http: &Client, job: &Job) -> Result<Vec<Member>> {
    Ok(http
        .guild_members(job.guild)
        .limit(job.limit)
        .await?
        .models()
        .await?)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::TcpListener,
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
        route.observe(&Event::Resumed);
        assert!(!worker.complete(
            &second,
            vec![serde_json::from_value(member("8", "old")).unwrap()]
        ));
        assert!(handle.get(GUILD, Id::new(8)).is_none());
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
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = BufReader::new(socket);
            let mut line = String::new();
            socket.read_line(&mut line).await.unwrap();
            assert_eq!(
                line.trim_end(),
                "GET /api/v10/guilds/2/members?limit=4 HTTP/1.1"
            );
            while !line.trim_end().is_empty() {
                line.clear();
                assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
            }
            requested.send(()).unwrap();
            when_changed.await.unwrap();
            let body = json!([
                member("6", "rest"),
                member("7", "updated"),
                member("8", "left"),
                member("9", "loaded"),
                member("10", "too many")
            ]);
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.to_string().len()
            );
            socket.get_mut().write_all(reply.as_bytes()).await.unwrap();
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
}
