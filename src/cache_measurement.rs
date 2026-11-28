//! キャッシュを空の状態から埋め、上限に達した後も新しい要素に置き換え続ける手動計測です。
//! OS のメモリを比較するときは、各テストを個別のプロセスで実行してください。
//! `AKARI_CACHE_MEASURE_PAUSE_MS` を指定すると、表示した PID を各段階で外部から測れます。
//! Rust の型のサイズと保持中の文字列バイト数は、アロケーターやコンテナーを含まない内訳です。

use std::{
    env,
    hint::black_box,
    mem::size_of,
    sync::Arc,
    time::{Duration, Instant},
};

use serde_json::json;
use twilight_gateway::{Event, Intents, ShardId};
use twilight_model::{
    channel::Message,
    gateway::payload::incoming::MemberAdd,
    guild::Member,
    id::{Id, marker::GuildMarker},
};

use crate::{
    CachedMessageEdit, Events, MemberCacheHandle, MessageUpdate, member_cache::MemberCacheRuntime,
    message_cache::MessageCacheRuntime,
};

const DEFAULT_CAPACITY: u64 = 16_384;
const CHURN_ROUNDS: u64 = 4;
const MESSAGE_BYTES: usize = 256;

#[test]
#[ignore = "manual release-mode cache measurement"]
fn message_cache_fill_and_churn() {
    let measurement = Measurement::new("message");
    let mut events = Events::new();
    events.on_message_edit(async |_: &CachedMessageEdit| Ok::<(), crate::Error>(()));
    let runtime = MessageCacheRuntime::prepare(
        measurement.capacity.try_into().unwrap(),
        &events,
        Intents::GUILD_MESSAGES,
    )
    .unwrap()
    .unwrap();
    let route = runtime.shard(ShardId::ONE);
    let template: Message = serde_json::from_value(json!({
        "id":"1", "channel_id":"2", "guild_id":"3", "type":0,
        "author":{"id":"4", "username":"member", "discriminator":"0"},
        "content":"x".repeat(MESSAGE_BYTES), "timestamp":"2026-01-01T00:00:00+00:00",
        "attachments":[], "embeds":[], "mention_everyone":false, "mention_roles":[],
        "mentions":[], "pinned":false, "tts":false
    }))
    .unwrap();
    let total = measurement.fill::<Message>(|id| {
        let mut message = template.clone();
        message.id = Id::new(id);
        let message = Arc::new(message);
        let started = Instant::now();
        route.store(&message);
        started.elapsed()
    });

    // キャッシュ済みメッセージへのアクセスは実際の部分更新を通す。
    // ヒット時には更新前後のモデルが作られるため、単純なマップ検索より処理が多い。
    let mut update: MessageUpdate = serde_json::from_value(json!({
        "id":"1", "channel_id":"2", "guild_id":"3", "pinned":true
    }))
    .unwrap();
    measurement.verify::<Message>(total, "edit", |id| {
        update.id = Id::new(id);
        let started = Instant::now();
        let value = black_box(route.edit(&update));
        let elapsed = started.elapsed();
        let payload = value.as_ref().map(|edit| {
            assert_eq!(edit.after.id, update.id);
            assert!(edit.after.pinned);
            assert_eq!(edit.after.content.len(), MESSAGE_BYTES);
            edit.after.content.len() + edit.after.author.name.len()
        });
        (elapsed, payload)
    });
}

#[test]
#[ignore = "manual release-mode cache measurement"]
fn member_cache_fill_and_churn() {
    let measurement = Measurement::new("member");
    let handle = MemberCacheHandle::new();
    let runtime = MemberCacheRuntime::prepare(
        measurement.capacity.try_into().unwrap(),
        &Events::new(),
        Intents::GUILDS | Intents::GUILD_MEMBERS,
        Some(handle.clone()),
        None,
    )
    .unwrap()
    .unwrap();
    let route = runtime.shard(ShardId::ONE);
    let template: MemberAdd = serde_json::from_value(json!({
        "guild_id":"1", "user":{"id":"1", "username":"member", "discriminator":"0"},
        "deaf":false, "mute":false, "flags":0, "roles":["1", "2", "3"], "nick":"fixture nickname",
        "joined_at":"2026-01-01T00:00:00+00:00"
    }))
    .unwrap();
    let total = measurement.fill::<Member>(|id| {
        let mut event = template.clone();
        event.guild_id = member_guild(id);
        event.member.user.id = Id::new(id);
        let event = Event::MemberAdd(Box::new(event));
        let started = Instant::now();
        let notification = black_box(route.observe(&event));
        let elapsed = started.elapsed();
        assert!(notification.is_none());
        elapsed
    });
    measurement.verify::<Member>(total, "get", |id| {
        let started = Instant::now();
        let value = black_box(handle.get(member_guild(id), Id::new(id)));
        let elapsed = started.elapsed();
        let payload = value.as_ref().map(|member| {
            assert_eq!(member.user.id, Id::new(id));
            assert_eq!(member.nick, template.member.nick);
            assert_eq!(member.roles, template.member.roles);
            member.user.name.len() + member.nick.as_ref().map_or(0, String::len)
        });
        (elapsed, payload)
    });
}

fn member_guild(user_id: u64) -> Id<GuildMarker> {
    // サーバー単位のマップも、サーバー全体が古くなれば入れ替わる。
    Id::new((user_id - 1) / 128 + 1)
}

struct Measurement {
    cache: &'static str,
    capacity: u64,
    pause: Duration,
}

impl Measurement {
    fn new(cache: &'static str) -> Self {
        let capacity = option("AKARI_CACHE_MEASURE_CAPACITY", DEFAULT_CAPACITY, 250_000);
        assert!(capacity > 0, "cache capacity must be positive");
        Self {
            cache,
            capacity,
            pause: Duration::from_millis(option("AKARI_CACHE_MEASURE_PAUSE_MS", 0, 60_000)),
        }
    }

    fn fill<T>(&self, mut insert: impl FnMut(u64) -> Duration) -> u64 {
        self.checkpoint("empty", 0, 0);
        let mut total = 0;
        for round in 0..=CHURN_ROUNDS {
            let start = total + 1;
            total += self.capacity;
            let elapsed = (start..=total).map(&mut insert).sum();
            self.time(
                if round == 0 {
                    "fill_insert"
                } else {
                    "churn_insert"
                },
                round,
                self.capacity,
                elapsed,
            );
            self.checkpoint(if round == 0 { "full" } else { "churn" }, round, total);
        }
        println!(
            "perf cache={} model_inline_bytes_per_entry={}",
            self.cache,
            size_of::<T>()
        );
        total
    }

    fn verify<T>(
        &self,
        total: u64,
        metric: &str,
        mut access: impl FnMut(u64) -> (Duration, Option<usize>),
    ) {
        let mut hit_time = Duration::ZERO;
        let mut miss_time = Duration::ZERO;
        let mut payload_bytes = 0;
        let mut hits = 0;
        for id in 1..=total {
            let (elapsed, payload) = access(id);
            if id > total - self.capacity {
                hit_time += elapsed;
                hits += 1;
                payload_bytes += payload.expect("latest key still cached");
            } else {
                miss_time += elapsed;
                assert!(payload.is_none(), "evicted key {id} still cached");
            }
        }
        println!(
            "perf cache={} offered={total} retained={hits} evicted={} retained_model_inline_bytes={} retained_string_bytes={payload_bytes}",
            self.cache,
            total - hits,
            size_of::<T>() * usize::try_from(hits).unwrap(),
        );
        self.time(&format!("{metric}_hit"), CHURN_ROUNDS, hits, hit_time);
        self.time(
            &format!("{metric}_miss"),
            CHURN_ROUNDS,
            total - hits,
            miss_time,
        );
    }

    fn checkpoint(&self, stage: &str, round: u64, offered: u64) {
        println!(
            "perf cache={} stage={stage} round={round} pid={} capacity={} offered={offered} expected_retained={} debug_assertions={}",
            self.cache,
            std::process::id(),
            self.capacity,
            offered.min(self.capacity),
            cfg!(debug_assertions),
        );
        if !self.pause.is_zero() {
            std::thread::sleep(self.pause);
        }
    }

    fn time(&self, metric: &str, round: u64, count: u64, elapsed: Duration) {
        println!(
            "perf cache={} metric={metric} round={round} operations={count} mean_ns={:.1}",
            self.cache,
            elapsed.as_secs_f64() * 1_000_000_000.0 / count as f64,
        );
    }
}

fn option(key: &str, default: u64, maximum: u64) -> u64 {
    env::var(key).map_or(default, |value| {
        let value = value
            .parse()
            .expect("measurement setting must be an integer");
        assert!(value <= maximum, "{key} must be at most {maximum}");
        value
    })
}
