use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    time::timeout,
};
use tokio_websockets::{Message as Frame, ServerBuilder, WebSocketStream};
use twilight_gateway::{Intents, ShardId, ShardState};
use twilight_model::{
    gateway::{
        payload::incoming::{GuildCreate, GuildDelete, VoiceServerUpdate},
        presence::Status,
    },
    id::Id,
};

use crate::{
    Bot, CachedMessageEdit, Context, Error, Events, GatewayShardState, GatewayStatusHandle,
    GuildLifecycleEvent, GuildLifecycleKind, MessageUpdate, Presence, PresenceHandle, Ready,
    ShutdownHandle, VoiceGateway, VoiceGatewayError, VoiceStateChange,
};

use super::lost_gateway_session;

mod permissions_on_reconnect;
mod voice_handshake;

const FIXTURE_TOKEN: &str = "local-gateway-fixture";
const HELLO: &str = r#"{"op":10,"d":{"heartbeat_interval":60000}}"#;
const ACK: &str = r#"{"op":11,"d":null}"#;
const MESSAGE: &str = r#"{"op":0,"s":2,"t":"MESSAGE_CREATE","d":{
    "id":"10","channel_id":"5","guild_id":"4","type":0,
    "author":{"id":"6","username":"member","discriminator":"0"},
    "content":"verify heartbeat RTT","timestamp":"2026-01-01T00:00:00.000000+00:00",
    "attachments":[],"embeds":[],"mention_everyone":false,"mention_roles":[],"mentions":[],
    "pinned":false,"tts":false
}}"#;
const PARTIAL_EDIT: &str =
    r#"{"op":0,"s":3,"t":"MESSAGE_UPDATE","d":{"id":"10","channel_id":"5","embeds":[]}}"#;

enum Observation {
    Ready(u64),
    GatewayRtt(Option<Duration>),
    PartialEdit { id: u64, has_content: bool },
    CachedEdit(u64, String, String),
}

enum ShardObservation {
    Ready(Option<u32>),
    Message(u64, Option<Duration>),
    VoiceServer(u64),
}

#[test]
fn permission_invalidation_follows_session_loss_not_reconnect_attempts() {
    let disconnected = ShardState::Disconnected {
        reconnect_attempts: 0,
    };
    let retrying = ShardState::Disconnected {
        reconnect_attempts: 1,
    };

    assert!(lost_gateway_session(ShardState::Active, disconnected));
    assert!(lost_gateway_session(
        ShardState::Active,
        ShardState::Resuming,
    ));
    assert!(lost_gateway_session(ShardState::Resuming, disconnected));
    assert!(!lost_gateway_session(disconnected, retrying));
    assert!(!lost_gateway_session(retrying, ShardState::Identifying));
    assert!(!lost_gateway_session(
        ShardState::Resuming,
        ShardState::Active,
    ));
}

#[tokio::test]
async fn loopback_gateway_identifies_resumes_and_closes_cleanly() {
    let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = server.local_addr().unwrap();
    let url = format!("ws://{address}");
    let (observations, received) = mpsc::unbounded_channel();
    let ready = observations.clone();
    let raw_edit = observations.clone();
    let cached_edit = observations.clone();
    let mut events = Events::new();
    events.on_ready(async move |payload: &Ready| {
        ready.send(Observation::Ready(payload.user.id.get()))
    });
    events.on_raw_message_edit(async move |payload: &MessageUpdate| {
        raw_edit.send(Observation::PartialEdit {
            id: payload.id.get(),
            has_content: payload.content.is_some(),
        })
    });
    events.on_message_edit(async move |edit: &CachedMessageEdit| {
        cached_edit.send(Observation::CachedEdit(
            edit.after.id.get(),
            edit.before.content.clone(),
            edit.after.content.clone(),
        ))
    });
    let intents = Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT;
    let presence = PresenceHandle::new(Presence::new(Status::Idle));
    let (mut bot, shutdown) = Bot::new(FIXTURE_TOKEN)
        .unwrap()
        .add_intents(intents)
        .with_events(events)
        .cache_messages(2)
        .with_presence(presence)
        .with_shutdown();
    bot.gateway_test_port = Some(address.port());

    let client = bot.run(async move |ctx: Context<'_>| {
        observations.send(Observation::GatewayRtt(ctx.timing().gateway_rtt()))
    });
    let gateway = mock_gateway(server, &url, intents, received, shutdown);
    let (result, ()) = timeout(Duration::from_secs(10), async {
        tokio::join!(client, gateway)
    })
    .await
    .expect("gateway fixture completed");
    assert!(result.is_ok(), "{result:?}");
}

#[tokio::test]
async fn guild_lifecycle_requires_explicit_guilds_before_connecting() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut events = Events::new();
    events.on_guild_lifecycle(async |_: &GuildLifecycleEvent| Ok::<(), Error>(()));
    let mut bot = Bot::new(FIXTURE_TOKEN)
        .unwrap()
        .with_intents(Intents::empty())
        .with_events(events);
    bot.gateway_test_port = Some(listener.local_addr().unwrap().port());

    let result = timeout(Duration::from_secs(1), bot.start())
        .await
        .expect("Intent validation completed before connecting");
    assert!(matches!(
        result,
        Err(Error::MissingEventIntents { any_of }) if any_of == Intents::GUILDS
    ));
    assert!(
        timeout(Duration::from_millis(25), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn loopback_gateway_dispatches_guild_lifecycle_and_raw_events() {
    use GuildLifecycleKind::{Available, InitialAvailable, Joined, Removed, Unavailable};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (derived, mut lifecycle) = mpsc::unbounded_channel();
    let (raw, mut raw_events) = mpsc::unbounded_channel();
    let raw_create = raw.clone();
    let mut events = Events::new();
    events.on_guild_lifecycle(async move |event: &GuildLifecycleEvent| {
        derived.send((event.kind(), event.guild_id().get()))
    });
    events
        .on_gateway(async move |event: &GuildCreate| raw_create.send(("create", event.id().get())));
    events.on_gateway(async move |event: &GuildDelete| raw.send(("delete", event.id.get())));
    let (mut bot, shutdown) = Bot::new(FIXTURE_TOKEN)
        .unwrap()
        .with_intents(Intents::GUILDS)
        .with_events(events)
        .with_shutdown();
    bot.gateway_test_port = Some(address.port());
    let gateway = async {
        let mut socket = accept(&listener).await;
        let identify = receive_opcode(&mut socket, 2).await;
        assert_eq!(identify["d"]["intents"], Intents::GUILDS.bits());
        send_ready_with_guilds(
            &mut socket,
            &format!("ws://{address}"),
            "guild-lifecycle",
            [0, 1],
            json!([{"id":"4", "unavailable":true}]),
        )
        .await;
        for (sequence, (event, payload, raw_kind, kind, id)) in [
            (
                "GUILD_CREATE",
                available_guild_payload(4),
                "create",
                InitialAvailable,
                4,
            ),
            ("GUILD_CREATE", guild_payload(5), "create", Joined, 5),
            (
                "GUILD_DELETE",
                json!({"id":"5", "unavailable":true}),
                "delete",
                Unavailable,
                5,
            ),
            (
                "GUILD_CREATE",
                available_guild_payload(5),
                "create",
                Available,
                5,
            ),
            ("GUILD_DELETE", json!({"id":"5"}), "delete", Removed, 5),
        ]
        .into_iter()
        .enumerate()
        {
            send_dispatch(&mut socket, sequence + 2, event, payload).await;
            assert_eq!(lifecycle.recv().await, Some((kind, id)));
            assert_eq!(raw_events.recv().await, Some((raw_kind, id)));
        }

        // 型に変換できない所属イベントの後は、次の Ready まで推測した通知を出さない。
        send_dispatch(&mut socket, 7, "GUILD_CREATE", json!({"id":"6"})).await;
        send_dispatch(&mut socket, 8, "GUILD_CREATE", guild_payload(7)).await;
        assert_eq!(raw_events.recv().await, Some(("create", 7)));
        // 後続の raw イベントまで処理されると、先行する同じキューの通知も処理済み。
        send_dispatch(&mut socket, 9, "GUILD_DELETE", json!({"id":"7"})).await;
        assert_eq!(raw_events.recv().await, Some(("delete", 7)));
        assert!(lifecycle.try_recv().is_err());
        assert!(raw_events.try_recv().is_err());
        shutdown.request();
        expect_close(&mut socket, 1000).await;
    };
    let (result, ()) = timeout(Duration::from_secs(10), async {
        tokio::join!(bot.start(), gateway)
    })
    .await
    .expect("guild lifecycle gateway fixture completed");
    assert!(result.is_ok(), "{result:?}");
}

fn guild_payload(id: u64) -> Value {
    json!({
        "id":id.to_string(), "owner_id":"2", "name":"fixture", "afk_timeout":60,
        "default_message_notifications":0, "explicit_content_filter":0, "features":[],
        "mfa_level":0, "nsfw_level":0, "preferred_locale":"ja", "roles":[],
        "system_channel_flags":0, "premium_progress_bar_enabled":false, "verification_level":0
    })
}

fn available_guild_payload(id: u64) -> Value {
    let mut payload = guild_payload(id);
    payload["unavailable"] = false.into();
    payload
}

#[tokio::test]
async fn loopback_gateway_fulfills_event_waits_and_cancels_on_shutdown() {
    use crate::{WaitError, WaitLimits, model::channel::Message};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (ready, mut ready_rx) = mpsc::unbounded_channel();
    let (ordinary, mut ordinary_rx) = mpsc::unbounded_channel();
    let (from_handler, mut from_handler_rx) = mpsc::unbounded_channel();
    let intents =
        Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT | Intents::GUILD_MESSAGE_REACTIONS;
    let (bot, waiters) = Bot::new(FIXTURE_TOKEN)
        .unwrap()
        .with_intents(intents)
        .with_waiters(WaitLimits::default())
        .unwrap();
    let channel = Id::new(5);
    let user = Id::new(6);
    let deadline = Duration::from_secs(3);
    let handler_waiters = waiters.clone();
    let mut events = Events::new();
    events.on_ready(async move |_: &Ready| -> crate::Result<()> {
        // on_ready が通常のイベントキューを処理中でも、待機は Gateway から解決される。
        let pending = handler_waiters
            .wait_for_message(channel, user, deadline)
            .map_err(Error::handler)?;
        ready.send(()).map_err(Error::handler)?;
        from_handler
            .send(pending.await.map_err(Error::handler)?.id)
            .map_err(Error::handler)
    });
    events.on_message(async move |message: &Message| ordinary.send(message.id));
    let (mut bot, shutdown) = bot.with_events(events).with_shutdown();
    bot.gateway_test_port = Some(address.port());
    assert!(matches!(
        waiters.wait_for_message(channel, user, deadline),
        Err(WaitError::NotEnabled)
    ));
    let gateway = async {
        let mut socket = accept(&listener).await;
        let identify = receive_opcode(&mut socket, 2).await;
        assert_eq!(identify["d"]["intents"], intents.bits());
        send_ready(
            &mut socket,
            &format!("ws://{address}"),
            "event-wait",
            [0, 1],
        )
        .await;
        assert_eq!(ready_rx.recv().await, Some(()));

        let message = waiters
            .wait_for_message_where(channel, user, deadline, |message| {
                message.content == "verify heartbeat RTT"
            })
            .unwrap();
        let reaction = waiters
            .wait_for_reaction_add(channel, user, deadline)
            .unwrap();
        socket.send(Frame::text(MESSAGE)).await.unwrap();
        send_dispatch(
            &mut socket,
            3,
            "MESSAGE_REACTION_ADD",
            json!({
                "channel_id":"5", "message_id":"10", "guild_id":"4", "user_id":"6",
                "emoji":{"id":null, "name":"👍"}, "burst":false, "type":0
            }),
        )
        .await;
        assert_eq!(message.await.unwrap().id, Id::new(10));
        assert_eq!(reaction.await.unwrap().message_id, Id::new(10));
        assert_eq!(from_handler_rx.recv().await, Some(Id::new(10)));
        assert_eq!(ordinary_rx.recv().await, Some(Id::new(10)));
        let pending = waiters.wait_for_message(channel, user, deadline).unwrap();
        shutdown.request();
        assert!(matches!(pending.await, Err(WaitError::Closed)));
        expect_close(&mut socket, 1000).await;
    };
    let (result, ()) = timeout(Duration::from_secs(10), async {
        tokio::join!(bot.start(), gateway)
    })
    .await
    .unwrap();
    assert!(result.is_ok(), "{result:?}");
}

async fn mock_gateway(
    listener: TcpListener,
    url: &str,
    intents: Intents,
    mut observations: mpsc::UnboundedReceiver<Observation>,
    shutdown: ShutdownHandle,
) {
    let mut socket = accept(&listener).await;
    let identify = receive_opcode(&mut socket, 2).await;
    assert_eq!(identify["d"]["shard"], json!([0, 1]));
    assert_eq!(identify["d"]["intents"], intents.bits());
    assert_eq!(identify["d"]["presence"]["status"], "idle");

    send_ready(&mut socket, url, "fixture-session", [0, 1]).await;
    assert!(matches!(
        observations.recv().await,
        Some(Observation::Ready(2))
    ));

    socket
        .send(Frame::text(r#"{"op":1,"d":null}"#))
        .await
        .unwrap();
    receive_opcode(&mut socket, 1).await;
    socket.send(Frame::text(ACK)).await.unwrap();
    socket.send(Frame::text(MESSAGE)).await.unwrap();
    assert!(matches!(
        observations.recv().await,
        Some(Observation::GatewayRtt(Some(_)))
    ));
    socket.send(Frame::text(PARTIAL_EDIT)).await.unwrap();
    assert!(matches!(
        observations.recv().await,
        Some(Observation::PartialEdit {
            id: 10,
            has_content: false
        })
    ));
    assert!(matches!(
        observations.recv().await,
        Some(Observation::CachedEdit(10, before, after))
            if before == "verify heartbeat RTT" && after == before
    ));

    let mut own: Value = serde_json::from_str(MESSAGE).unwrap();
    own["s"] = 4.into();
    own["d"]["id"] = "11".into();
    own["d"]["content"] = "owned before".into();
    own["d"]["author"]["id"] = "2".into();
    own["d"]["author"]["bot"] = true.into();
    socket.send(Frame::text(own.to_string())).await.unwrap();

    socket
        .send(Frame::text(r#"{"op":7,"d":null}"#))
        .await
        .unwrap();
    expect_close(&mut socket, 4000).await;
    drop(socket);

    let mut socket = accept(&listener).await;
    let resume = receive_opcode(&mut socket, 6).await;
    assert_eq!(resume["d"]["session_id"], "fixture-session");
    assert_eq!(resume["d"]["seq"], 4);
    socket
        .send(Frame::text(r#"{"op":0,"s":5,"t":"MESSAGE_UPDATE","d":{"id":"11","channel_id":"5","content":"owned after"}}"#))
        .await
        .unwrap();
    assert!(matches!(
        observations.recv().await,
        Some(Observation::PartialEdit {
            id: 11,
            has_content: true
        })
    ));
    assert!(matches!(
        observations.recv().await,
        Some(Observation::CachedEdit(11, before, after))
            if before == "owned before" && after == "owned after"
    ));
    socket
        .send(Frame::text(r#"{"op":0,"s":6,"t":"RESUMED","d":{}}"#))
        .await
        .unwrap();
    let presence = receive_opcode(&mut socket, 3).await;
    assert_eq!(presence["d"]["status"], "idle");

    shutdown.request();
    expect_close(&mut socket, 1000).await;
    assert!(observations.recv().await.is_none());
}

#[tokio::test]
async fn loopback_gateway_routes_member_queries_and_matches_nonce() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (ready, mut ready_rx) = mpsc::channel(1);
    let mut events = Events::new();
    events.on_ready(async move |_: &Ready| ready.send(()).await);
    let (bot, query) = Bot::new(FIXTURE_TOKEN)
        .unwrap()
        .with_events(events)
        .with_member_query();
    let (mut bot, shutdown) = bot.with_shutdown();
    bot.gateway_test_port = Some(address.port());
    let gateway = async {
        let mut socket = accept(&listener).await;
        let identify = receive_opcode(&mut socket, 2).await;
        assert_eq!(identify["d"]["intents"], 0);
        send_ready(
            &mut socket,
            &format!("ws://{address}"),
            "member-query",
            [0, 1],
        )
        .await;
        let prefix = receive_opcode(&mut socket, 8).await;
        assert_eq!(prefix["d"]["guild_id"], "4");
        assert_eq!(prefix["d"]["query"], "mem");
        assert_eq!(prefix["d"]["limit"], 2);
        send_dispatch(
            &mut socket,
            2,
            "GUILD_MEMBERS_CHUNK",
            member_query_chunk("unrelated", "8", &[]),
        )
        .await;
        send_dispatch(
            &mut socket,
            3,
            "GUILD_MEMBERS_CHUNK",
            member_query_chunk(prefix["d"]["nonce"].as_str().unwrap(), "6", &[]),
        )
        .await;

        let ids = receive_opcode(&mut socket, 8).await;
        assert_eq!(ids["d"]["guild_id"], "4");
        assert_eq!(ids["d"]["user_ids"], json!(["6", "7"]));
        assert_ne!(prefix["d"]["nonce"], ids["d"]["nonce"]);
        send_dispatch(
            &mut socket,
            4,
            "GUILD_MEMBERS_CHUNK",
            member_query_chunk(ids["d"]["nonce"].as_str().unwrap(), "6", &["7"]),
        )
        .await;
        expect_close(&mut socket, 1000).await;
    };
    let request = async {
        assert_eq!(ready_rx.recv().await, Some(()));
        let deadline = Duration::from_secs(4);
        let guild = Id::new(4);
        let found = query.query_prefix(guild, "mem", 2, deadline).await.unwrap();
        assert_eq!(
            found
                .members()
                .iter()
                .map(|member| member.user.id)
                .collect::<Vec<_>>(),
            [Id::new(6)]
        );
        let found = query
            .query_ids(guild, &[Id::new(6), Id::new(7)], deadline)
            .await
            .unwrap();
        assert_eq!(
            found
                .members()
                .iter()
                .map(|member| member.user.id)
                .collect::<Vec<_>>(),
            [Id::new(6)]
        );
        assert_eq!(found.not_found(), &[Id::new(7)]);
        shutdown.request();
    };
    let (result, (), ()) = timeout(Duration::from_secs(10), async {
        tokio::join!(bot.start(), gateway, request)
    })
    .await
    .unwrap();
    assert!(result.is_ok(), "{result:?}");
}

#[tokio::test]
async fn loopback_member_query_bounds_transport_before_next_inbound_frame() {
    use crate::MemberQueryError;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (ready, mut ready_rx) = mpsc::channel(1);
    let (registered, mut registered_rx) = mpsc::channel(1);
    let mut events = Events::new();
    events.on_ready(async move |_: &Ready| ready.send(()).await);
    let (bot, query) = Bot::new(FIXTURE_TOKEN)
        .unwrap()
        .with_events(events)
        .with_member_query();
    let (mut bot, shutdown) = bot.with_shutdown();
    bot.gateway_test_port = Some(address.port());
    let server = async {
        let mut socket = accept(&listener).await;
        receive_opcode(&mut socket, 2).await;
        send_ready(
            &mut socket,
            &format!("ws://{address}"),
            "member-query-transport",
            [0, 1],
        )
        .await;
        for _ in 0..super::MEMBER_QUERY_TRANSPORT_LIMIT {
            receive_opcode(&mut socket, 8).await;
        }
        assert_eq!(registered_rx.recv().await, Some(()));
        assert!(
            timeout(Duration::from_millis(75), socket.next())
                .await
                .is_err()
        );
        socket.send(Frame::text(ACK)).await.unwrap();
        let next = receive_opcode(&mut socket, 8).await;
        let nonce = next["d"]["nonce"].as_str().unwrap();
        send_dispatch(
            &mut socket,
            2,
            "GUILD_MEMBERS_CHUNK",
            member_query_chunk(nonce, "6", &[]),
        )
        .await;
        expect_close(&mut socket, 1000).await;
    };
    let request = async {
        assert_eq!(ready_rx.recv().await, Some(()));
        let guild = Id::new(4);
        let short = Duration::from_millis(500);
        let attempts = futures_util::future::join_all(
            (0..super::MEMBER_QUERY_TRANSPORT_LIMIT)
                .map(|_| query.query_prefix(guild, "mem", 1, short)),
        )
        .await;
        assert!(
            attempts
                .iter()
                .all(|result| matches!(result, Err(MemberQueryError::TimedOut)))
        );
        let next = query.query_prefix(guild, "mem", 1, Duration::from_secs(3));
        tokio::pin!(next);
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(next.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        registered.send(()).await.unwrap();
        assert_eq!(next.await.unwrap().members().len(), 1);
        shutdown.request();
    };
    let (result, (), ()) = timeout(Duration::from_secs(10), async {
        tokio::join!(bot.start(), server, request)
    })
    .await
    .unwrap();
    assert!(result.is_ok(), "{result:?}");
}

fn member_query_chunk(nonce: &str, user: &str, not_found: &[&str]) -> Value {
    json!({
        "guild_id":"4", "chunk_index":0, "chunk_count":1, "nonce":nonce,
        "members":[{
            "user":{"id":user, "username":"member", "discriminator":"0"},
            "roles":[], "joined_at":"2026-01-01T00:00:00+00:00",
            "deaf":false, "mute":false, "flags":0
        }],
        "not_found":not_found
    })
}

#[tokio::test]
async fn recommended_shards_launch_in_waves_and_resume_independently() {
    let gateway = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_address = gateway.local_addr().unwrap();
    let url = format!("ws://{gateway_address}");
    let (observations, received) = mpsc::unbounded_channel();
    let ready = observations.clone();
    let mut events = Events::new();
    events.on_ready(async move |payload: &Ready| {
        ready.send(ShardObservation::Ready(payload.shard.map(ShardId::number)))
    });
    let voice_events = observations.clone();
    events.on_gateway(async move |update: &VoiceServerUpdate| {
        voice_events.send(ShardObservation::VoiceServer(update.guild_id.get()))
    });
    let (bot, shutdown) = Bot::new(FIXTURE_TOKEN)
        .unwrap()
        .with_recommended_shards()
        .add_intents(
            Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT | Intents::GUILD_VOICE_STATES,
        )
        .with_events(events)
        .with_presence(PresenceHandle::new(Presence::new(Status::Idle)))
        .with_shutdown();
    let (bot, status) = bot.with_gateway_status();
    let (mut bot, voice) = bot.with_voice_gateway();
    bot.gateway_test_port = Some(gateway_address.port());
    bot.http_test_port = Some(http.local_addr().unwrap().port());

    let client = bot.run(async move |ctx: Context<'_>| {
        observations.send(ShardObservation::Message(
            ctx.message().id.get(),
            ctx.timing().gateway_rtt(),
        ))
    });
    let (result, (), ()) = timeout(Duration::from_secs(15), async {
        tokio::join!(
            client,
            respond_to_discovery(&http, 1),
            mock_shards(gateway, &url, received, shutdown, &status, &voice)
        )
    })
    .await
    .expect("two-shard fixture completed");
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        voice
            .set_state(Id::new(4_194_304), VoiceStateChange::leave())
            .await,
        Err(VoiceGatewayError::Closed)
    );
    assert_eq!(
        status
            .shards()
            .iter()
            .map(|shard| shard.state)
            .collect::<Vec<_>>(),
        [GatewayShardState::Closed, GatewayShardState::Closed]
    );
}

async fn respond_to_discovery(listener: &TcpListener, max_concurrency: u16) {
    let (socket, _) = listener.accept().await.unwrap();
    let mut request = BufReader::new(socket);
    let mut line = String::new();
    request.read_line(&mut line).await.unwrap();
    assert!(line.starts_with("GET ") && line.contains("/gateway/bot HTTP/"));
    loop {
        line.clear();
        assert_ne!(request.read_line(&mut line).await.unwrap(), 0);
        if line == "\r\n" {
            break;
        }
    }
    let body = json!({
        "url":"wss://gateway.discord.gg", "shards":2,
        "session_start_limit":{"total":10, "remaining":10, "reset_after":60000, "max_concurrency":max_concurrency}
    }).to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    request
        .into_inner()
        .write_all(response.as_bytes())
        .await
        .unwrap();
}

async fn mock_shards(
    gateway: TcpListener,
    url: &str,
    mut observations: mpsc::UnboundedReceiver<ShardObservation>,
    shutdown: ShutdownHandle,
    status: &GatewayStatusHandle,
    voice: &VoiceGateway,
) {
    let mut first = accept(&gateway).await;
    assert_eq!(
        receive_opcode(&mut first, 2).await["d"]["shard"],
        json!([0, 2])
    );
    assert!(
        timeout(Duration::from_millis(25), gateway.accept())
            .await
            .is_err()
    );
    assert_eq!(status.shards()[0].state, GatewayShardState::Connecting);
    assert_eq!(status.shards()[1].state, GatewayShardState::Queued);
    assert_eq!(
        voice
            .set_state(Id::new(4_194_304), VoiceStateChange::leave())
            .await,
        Err(VoiceGatewayError::NotReady)
    );
    send_ready(&mut first, url, "session-zero", [0, 2]).await;
    assert!(matches!(
        observations.recv().await,
        Some(ShardObservation::Ready(Some(0)))
    ));

    let mut second = accept(&gateway).await;
    assert!(
        timeout(Duration::from_millis(25), receive_opcode(&mut second, 2))
            .await
            .is_err()
    );
    // 同じ Identify バケットの 5 秒制限を実時間の待ち時間にしない。
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(5)).await;
    tokio::time::resume();
    assert_eq!(
        receive_opcode(&mut second, 2).await["d"]["shard"],
        json!([1, 2])
    );
    send_ready(&mut second, url, "session-one", [1, 2]).await;
    assert!(matches!(
        observations.recv().await,
        Some(ShardObservation::Ready(Some(1)))
    ));
    observe_voice_request(
        voice,
        &mut second,
        VoiceStateChange::join(Id::new(42)).self_mute(true),
        json!("42"),
        true,
    )
    .await;
    second
        .send(Frame::text(r#"{"op":0,"s":2,"t":"VOICE_SERVER_UPDATE","d":{"guild_id":"4194304","endpoint":"fixture.local","token":"voice-fixture"}}"#))
        .await
        .unwrap();
    assert!(matches!(
        observations.recv().await,
        Some(ShardObservation::VoiceServer(4_194_304))
    ));
    second
        .send(Frame::text(r#"{"op":1,"d":null}"#))
        .await
        .unwrap();
    receive_opcode(&mut second, 1).await;
    second.send(Frame::text(ACK)).await.unwrap();
    send_probe(&mut second, 20, 4_194_304, 3).await;
    assert!(matches!(
        observations.recv().await,
        Some(ShardObservation::Message(20, Some(_)))
    ));
    assert!(status.shards()[1].gateway_rtt.is_some());

    first
        .send(Frame::text(r#"{"op":1,"d":null}"#))
        .await
        .unwrap();
    receive_opcode(&mut first, 1).await;
    first.send(Frame::text(ACK)).await.unwrap();
    second
        .send(Frame::text(r#"{"op":7,"d":null}"#))
        .await
        .unwrap();
    expect_close(&mut second, 4000).await;
    // 相手が接続を閉じる前に再接続中と判定し、以前の RTT を消す。
    let second_status = status.shards()[1];
    assert_eq!(second_status.state, GatewayShardState::Reconnecting);
    assert_eq!(second_status.gateway_rtt, None);
    assert_eq!(
        voice
            .set_state(Id::new(4_194_304), VoiceStateChange::leave())
            .await,
        Err(VoiceGatewayError::NotReady)
    );
    drop(second);

    // シャード 1 の再接続を進めずに、シャード 0 のイベントを処理できる。
    send_probe(&mut first, 30, 8_388_608, 2).await;
    assert!(matches!(
        observations.recv().await,
        Some(ShardObservation::Message(30, Some(_)))
    ));

    let mut resumed = accept(&gateway).await;
    let resume = receive_opcode(&mut resumed, 6).await;
    let shards = status.shards();
    assert_eq!(shards[0].state, GatewayShardState::Ready);
    assert!(shards[0].gateway_rtt.is_some());
    assert_eq!(shards[1].state, GatewayShardState::Reconnecting);
    assert_eq!(shards[1].gateway_rtt, None);
    assert_eq!(resume["d"]["session_id"], "session-one");
    assert_eq!(resume["d"]["seq"], 3);
    resumed
        .send(Frame::text(r#"{"op":0,"s":4,"t":"RESUMED","d":{}}"#))
        .await
        .unwrap();
    assert_eq!(receive_opcode(&mut resumed, 3).await["d"]["status"], "idle");
    send_probe(&mut resumed, 40, 4_194_304, 5).await;
    assert!(matches!(
        observations.recv().await,
        Some(ShardObservation::Message(40, _))
    ));
    assert_eq!(status.shards()[1].state, GatewayShardState::Ready);
    observe_voice_request(
        voice,
        &mut resumed,
        VoiceStateChange::leave(),
        Value::Null,
        false,
    )
    .await;

    shutdown.request();
    tokio::join!(
        expect_close(&mut first, 1000),
        expect_close(&mut resumed, 1000)
    );
    assert!(observations.recv().await.is_none());
}

async fn observe_voice_request(
    voice: &VoiceGateway,
    socket: &mut WebSocketStream<TcpStream>,
    change: VoiceStateChange,
    channel: Value,
    muted: bool,
) {
    let (outcome, command) = tokio::join!(
        voice.set_state(Id::new(4_194_304), change),
        receive_opcode(socket, 4)
    );
    assert_eq!(outcome, Ok(()));
    assert_eq!(
        command["d"],
        json!({
            "channel_id": channel, "guild_id": "4194304", "self_deaf": false, "self_mute": muted
        })
    );
}

async fn send_ready(
    socket: &mut WebSocketStream<TcpStream>,
    url: &str,
    session_id: &str,
    shard: [u32; 2],
) {
    send_ready_with_guilds(socket, url, session_id, shard, json!([])).await;
}

async fn send_ready_with_guilds(
    socket: &mut WebSocketStream<TcpStream>,
    url: &str,
    session_id: &str,
    shard: [u32; 2],
    guilds: Value,
) {
    let payload = json!({
        "application": {"id": "1", "flags": 0},
        "guilds": guilds,
        "resume_gateway_url": url,
        "session_id": session_id,
        "shard": shard,
        "user": {"id": "2", "username": "fixture", "discriminator": "0", "mfa_enabled": false, "bot": true},
        "v": 10,
    });
    send_dispatch(socket, 1, "READY", payload).await;
}

async fn send_dispatch(
    socket: &mut WebSocketStream<TcpStream>,
    sequence: usize,
    kind: &str,
    payload: Value,
) {
    socket
        .send(Frame::text(format!(
            r#"{{"op":0,"s":{sequence},"t":"{kind}","d":{payload}}}"#
        )))
        .await
        .unwrap();
}

async fn send_probe(socket: &mut WebSocketStream<TcpStream>, id: u64, guild: u64, sequence: u64) {
    let mut payload: Value = serde_json::from_str(MESSAGE).unwrap();
    payload["s"] = sequence.into();
    payload["d"]["id"] = id.to_string().into();
    payload["d"]["guild_id"] = guild.to_string().into();
    socket.send(Frame::text(payload.to_string())).await.unwrap();
}

async fn accept(listener: &TcpListener) -> WebSocketStream<TcpStream> {
    let (socket, _) = listener.accept().await.unwrap();
    let (request, mut socket) = ServerBuilder::new().accept(socket).await.unwrap();
    assert!(
        request
            .uri()
            .query()
            .unwrap_or_default()
            .contains("encoding=json")
    );
    socket.send(Frame::text(HELLO)).await.unwrap();
    socket
}

async fn receive_opcode(socket: &mut WebSocketStream<TcpStream>, expected: u8) -> Value {
    loop {
        let frame = socket.next().await.unwrap().unwrap();
        let payload: Value = serde_json::from_str(frame.as_text().unwrap()).unwrap();
        if expected != 1 && payload["op"] == 1 {
            socket.send(Frame::text(ACK)).await.unwrap();
            continue;
        }
        assert_eq!(payload["op"], expected);
        return payload;
    }
}

async fn expect_close(socket: &mut WebSocketStream<TcpStream>, expected: u16) {
    loop {
        let frame = socket.next().await.unwrap().unwrap();
        if let Some((code, _)) = frame.as_close() {
            assert_eq!(u16::from(code), expected);
            // tokio-websockets が受け取った Close の確認応答をキューに入れる。
            socket.flush().await.unwrap();
            return;
        }
        let payload: Value = serde_json::from_str(frame.as_text().unwrap()).unwrap();
        assert_eq!(payload["op"], 1);
        socket.send(Frame::text(ACK)).await.unwrap();
    }
}
