use std::sync::Mutex;

use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::*;
use crate::{
    Client, NoMessageHandler, dispatch_events,
    hooks::Hooks,
    message_events::{GatewayDecodeOptions, IncomingEvent, decode_text_with_options},
    permissions::PermissionResolver,
    route_gateway_event,
};

const INTENTS: Intents = Intents::GUILDS.union(Intents::GUILD_MEMBERS);

fn subscribed() -> Events {
    let mut events = Events::new();
    events.on_member_update(async |_: &CachedMemberUpdate| -> Result<()> { Ok(()) });
    events.on_member_remove(async |_: &CachedMemberRemove| -> Result<()> { Ok(()) });
    events
}

#[test]
fn requires_capacity_and_both_explicit_intents_only_for_cached_handlers() {
    let mut events = Events::new();
    events.on_gateway(async |_: &MemberUpdate| -> Result<()> { Ok(()) });
    assert!(
        MemberCacheRuntime::prepare(10, &events, Intents::empty(), None, None)
            .unwrap()
            .is_none()
    );
    let events = subscribed();
    assert!(matches!(
        MemberCacheRuntime::prepare(0, &events, INTENTS, None, None),
        Err(Error::MemberCacheRequired)
    ));
    for (configured, missing) in [
        (Intents::GUILDS, Intents::GUILD_MEMBERS),
        (Intents::GUILD_MEMBERS, Intents::GUILDS),
    ] {
        assert!(
            matches!(MemberCacheRuntime::prepare(2, &events, configured, None, None), Err(Error::MissingEventIntents { any_of }) if any_of == missing)
        );
    }
    let runtime = MemberCacheRuntime::prepare(2, &events, INTENTS, None, None)
        .unwrap()
        .unwrap();
    assert!(runtime.event_types().contains(
        EventTypeFlags::GUILD_CREATE
            | EventTypeFlags::MEMBER_UPDATE
            | EventTypeFlags::MEMBER_REMOVE
    ));
    assert!(!runtime.event_types().contains(EventTypeFlags::MEMBER_CHUNK));
}

#[test]
fn shared_lookup_works_without_event_listeners_and_hides_replayed_changes_until_resume() {
    let handle = MemberCacheHandle::new();
    let events = Events::new();
    let guild = Id::new(2);
    let user_id = Id::new(6);
    let other_guild = Id::new(4_194_304);
    assert!(handle.get(guild, user_id).is_none());
    assert!(matches!(
        MemberCacheRuntime::prepare(0, &events, INTENTS, Some(handle.clone()), None),
        Err(Error::MemberCacheRequired)
    ));
    assert!(
        matches!(MemberCacheRuntime::prepare(2, &events, Intents::GUILDS, Some(handle.clone()), None), Err(Error::MissingEventIntents { any_of }) if any_of == Intents::GUILD_MEMBERS)
    );
    let runtime = MemberCacheRuntime::prepare(2, &events, INTENTS, Some(handle.clone()), None)
        .unwrap()
        .unwrap();
    let first = runtime.shard(ShardId::new(0, 2));
    let second = runtime.shard(ShardId::new(1, 2));
    add(first, "2", "6");
    add(second, "4194304", "6");

    let reader = handle.clone();
    let before = std::thread::spawn(move || reader.get(guild, user_id).unwrap())
        .join()
        .unwrap();
    assert_eq!(before.nick.as_deref(), Some("before"));
    assert_eq!(before.roles, [Id::new(10)]);
    assert!(Arc::ptr_eq(&before, &handle.guild_members(guild)[0]));

    runtime.suspend_shard(ShardId::new(0, 2));
    assert!(handle.get(guild, user_id).is_none());
    assert!(handle.guild_members(guild).is_empty());
    assert!(handle.get(other_guild, user_id).is_some());
    assert!(
        update(
            first,
            json!({"guild_id":"2", "user":user("6"), "roles": [], "nick":"replayed"})
        )
        .is_none()
    );
    assert!(handle.get(guild, user_id).is_none());
    first.observe(&Event::Resumed);
    let after = handle.get(guild, user_id).unwrap();
    assert_eq!(after.nick.as_deref(), Some("replayed"));
    assert!(after.roles.is_empty());
    assert_eq!(before.nick.as_deref(), Some("before"));

    observe(first, "GUILD_DELETE", json!({"id":"2", "unavailable":true}));
    assert!(handle.get(guild, user_id).is_none());
    assert!(handle.get(other_guild, user_id).is_some());
    drop(runtime);
    assert!(handle.get(other_guild, user_id).is_none());
}

#[test]
fn shared_snapshot_is_bounded_and_the_replacement_session_does_not_publish_old_members() {
    let handle = MemberCacheHandle::new();
    let runtime =
        MemberCacheRuntime::prepare(2, &Events::new(), INTENTS, Some(handle.clone()), None)
            .unwrap()
            .unwrap();
    let guild = Id::new(2);
    let route = runtime.shard(ShardId::ONE);
    for id in ["6", "7", "8"] {
        add(route, "2", id);
    }
    assert!(handle.get(guild, Id::new(6)).is_none());
    assert_eq!(handle.guild_members(guild).len(), 2);

    runtime.suspend_shard(ShardId::ONE);
    let IncomingEvent::Standard(ready) = raw(
        "READY",
        json!({
            "application": {"id":"1", "flags":0}, "guilds": [],
            "resume_gateway_url":"wss://gateway.discord.gg", "session_id":"replacement", "v":10,
            "user":{"id":"1","username":"bot","discriminator":"0","bot":true,"mfa_enabled":false}
        }),
        EventTypeFlags::READY,
        false,
    ) else {
        panic!("expected ready");
    };
    route.observe(&ready);
    assert!(handle.guild_members(guild).is_empty());
    add(route, "2", "9");
    assert!(handle.get(guild, Id::new(9)).is_some());
    runtime.suspend_all();
    assert!(handle.guild_members(guild).is_empty());
}

#[tokio::test]
async fn public_handle_clears_before_shutdown_drains_event_handlers() {
    use futures_util::{SinkExt, StreamExt};
    use std::time::Duration;
    use tokio::{net::TcpListener, sync::Notify, time::timeout};
    use tokio_websockets::{Message as Frame, ServerBuilder};
    use twilight_model::gateway::payload::incoming::MemberAdd;

    let listen = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listen.local_addr().unwrap();
    let (enter, mut entered) = mpsc::channel(1);
    let wait_for_release = Arc::new(Notify::new());
    let release = Arc::clone(&wait_for_release);
    let mut events = Events::new();
    events.on_gateway(async move |_: &MemberAdd| -> Result<()> {
        enter.send(()).await.unwrap();
        wait_for_release.notified().await;
        Ok(())
    });
    let (bot, shutdown) = crate::Bot::new("local-member-cache-fixture")
        .unwrap()
        .with_events(events)
        .add_intents(INTENTS)
        .with_shutdown();
    let (mut bot, handle) = bot.with_member_cache(2);
    bot.gateway_test_port = Some(address.port());
    let guild = Id::new(2);
    let user_id = Id::new(6);
    assert!(handle.get(guild, user_id).is_none());

    let mock = async {
        let (socket, _) = listen.accept().await.unwrap();
        let (_, mut websocket) = ServerBuilder::new().accept(socket).await.unwrap();
        websocket
            .send(Frame::text(r#"{"op":10,"d":{"heartbeat_interval":60000}}"#))
            .await
            .unwrap();
        loop {
            let frame = websocket.next().await.unwrap().unwrap();
            let data: Value = serde_json::from_str(frame.as_text().unwrap()).unwrap();
            if data["op"] == 2 {
                break;
            }
            assert_eq!(data["op"], 1);
            websocket
                .send(Frame::text(r#"{"op":11,"d":null}"#))
                .await
                .unwrap();
        }
        let ready = json!({"op":0,"s":1,"t":"READY","d":{
            "application": {"id":"1","flags":0}, "guilds":[], "resume_gateway_url":format!("ws://{address}"),
            "session_id":"cache-fixture", "v":10,
            "user":{"id":"1","username":"bot","discriminator":"0","bot":true,"mfa_enabled":false}
        }});
        websocket
            .send(Frame::text(ready.to_string()))
            .await
            .unwrap();
        let joined = json!({"op":0,"s":2,"t":"GUILD_MEMBER_ADD","d":{
            "guild_id":"2","user": user("6"),"deaf":false,"mute":false,"roles":[],"flags":0
        }});
        websocket
            .send(Frame::text(joined.to_string()))
            .await
            .unwrap();
        entered.recv().await.unwrap();
        assert!(handle.get(guild, user_id).is_some());
        shutdown.request();
        loop {
            let frame = websocket.next().await.unwrap().unwrap();
            if let Some((code, _)) = frame.as_close() {
                assert_eq!(u16::from(code), 1000);
                websocket.flush().await.unwrap();
                break;
            }
            let data: Value = serde_json::from_str(frame.as_text().unwrap()).unwrap();
            assert_eq!(data["op"], 1);
            websocket
                .send(Frame::text(r#"{"op":11,"d":null}"#))
                .await
                .unwrap();
        }
        assert!(handle.get(guild, user_id).is_none());
        drop(websocket);
        release.notify_one();
    };

    let (result, ()) = timeout(Duration::from_secs(10), async {
        tokio::join!(bot.start(), mock)
    })
    .await
    .expect("loopback Gateway fixture completes");
    result.unwrap();
    assert!(handle.guild_members(guild).is_empty());
}

fn raw(
    event: &str,
    payload: Value,
    types: EventTypeFlags,
    capture_member_fields: bool,
) -> IncomingEvent {
    let envelope = json!({ "op": 0, "s": 2, "t": event, "d": payload });
    decode_text_with_options(
        envelope.to_string(),
        types,
        GatewayDecodeOptions {
            capture_member_fields,
            ..GatewayDecodeOptions::default()
        },
    )
    .unwrap()
    .unwrap()
}

fn observe(route: MemberCacheRoute<'_>, event: &str, payload: Value) -> Option<CachedMemberEvent> {
    let IncomingEvent::Standard(event) = raw(event, payload, route.runtime.event_types(), false)
    else {
        panic!("expected standard event");
    };
    route.observe(&event)
}

fn user(id: &str) -> Value {
    json!({"id": id, "username": "member", "discriminator": "0"})
}

fn add(route: MemberCacheRoute<'_>, guild: &str, id: &str) {
    assert!(
        observe(
            route,
            "GUILD_MEMBER_ADD",
            json!({
                "guild_id": guild, "user": user(id), "deaf": false, "mute": true,
                "flags": 0, "roles": ["10"], "nick": "before", "pending": true,
                "joined_at": "2026-01-01T00:00:00+00:00",
                "communication_disabled_until": "2026-02-01T00:00:00+00:00",
                "premium_since": "2026-01-01T00:00:00+00:00"
            })
        )
        .is_none()
    );
}

fn update(route: MemberCacheRoute<'_>, payload: Value) -> Option<CachedMemberUpdate> {
    let IncomingEvent::MemberUpdate(event, fields) = raw(
        "GUILD_MEMBER_UPDATE",
        payload,
        route.runtime.event_types(),
        true,
    ) else {
        panic!("expected member update and patch");
    };
    match route.update(&event, fields)? {
        CachedMemberEvent::Update(event) => Some(event),
        CachedMemberEvent::Remove(_) => panic!("expected cached update"),
    }
}

#[test]
fn partial_updates_keep_omitted_fields_and_clear_explicit_null_or_false() {
    let runtime = MemberCacheRuntime::prepare(2, &subscribed(), INTENTS, None, None)
        .unwrap()
        .unwrap();
    let route = runtime.shard(ShardId::ONE);
    assert!(
        update(
            route,
            json!({"guild_id":"7", "user": user("6"), "roles": []})
        )
        .is_none()
    );
    add(route, "7", "6");
    let event = update(
        route,
        json!({"guild_id":"7", "user": user("6"), "roles": ["11"], "pending":false,
        "nick":null, "communication_disabled_until":null, "deaf":true}),
    )
    .unwrap();
    assert_eq!(event.before.nick.as_deref(), Some("before"));
    assert!(event.before.pending);
    assert_eq!(event.before.roles, [Id::new(10)]);
    assert!(event.before.communication_disabled_until.is_some());
    assert!(event.after.nick.is_none());
    assert!(event.after.communication_disabled_until.is_none());
    assert!(!event.after.pending);
    assert!(event.after.deaf);
    assert!(event.after.mute);
    assert_eq!(event.after.roles, [Id::new(11)]);
    assert_eq!(event.before.premium_since, event.after.premium_since);
    assert_eq!(event.before.joined_at, event.after.joined_at);

    let newest = update(
        route,
        json!({"guild_id":"7", "user":user("6"), "roles": ["12"], "nick":"latest"}),
    )
    .unwrap();
    assert!(Arc::ptr_eq(&event.after, &newest.before));
    assert_eq!(event.after.roles, [Id::new(11)]);
    assert_eq!(newest.after.nick.as_deref(), Some("latest"));
}

#[test]
fn guild_create_seeds_no_more_than_the_configured_capacity() {
    let runtime = MemberCacheRuntime::prepare(1, &subscribed(), INTENTS, None, None)
        .unwrap()
        .unwrap();
    let route = runtime.shard(ShardId::ONE);
    let member = |id| json!({"user": user(id), "deaf":false, "mute":false, "flags":0, "roles":[]});
    let guild = json!({
        "id":"7", "owner_id":"1", "name":"seed", "afk_timeout":60,
        "default_message_notifications":0, "explicit_content_filter":0, "features":[],
        "mfa_level":0, "nsfw_level":0, "preferred_locale":"ja", "roles":[],
        "system_channel_flags":0, "premium_progress_bar_enabled":false, "verification_level":0,
        "members":[member("6"),member("8")]
    });
    assert!(observe(route, "GUILD_CREATE", guild).is_none());
    assert!(update(route, json!({"guild_id":"7", "user":user("6"), "roles":[]})).is_some());
    assert!(update(route, json!({"guild_id":"7", "user":user("8"), "roles":[]})).is_none());
}

#[test]
fn bounds_members_and_discards_only_matching_guild_and_shard() {
    let runtime = MemberCacheRuntime::prepare(2, &subscribed(), INTENTS, None, None)
        .unwrap()
        .unwrap();
    let first = runtime.shard(ShardId::new(0, 2));
    let second = runtime.shard(ShardId::new(1, 2));
    add(first, "2", "6");
    add(second, "4194304", "6");
    add(first, "3", "7");
    assert!(
        observe(
            first,
            "GUILD_MEMBER_REMOVE",
            json!({"guild_id":"2", "user":user("6")})
        )
        .is_none()
    );
    assert!(
        observe(
            first,
            "GUILD_MEMBER_REMOVE",
            json!({"guild_id":"4194304", "user":user("6")})
        )
        .is_none()
    );
    assert!(first.observe(&Event::Resumed).is_none());
    observe(
        first,
        "GUILD_ROLE_DELETE",
        json!({"guild_id":"3", "role_id":"10"}),
    );
    let removed = observe(
        first,
        "GUILD_MEMBER_REMOVE",
        json!({"guild_id":"3", "user":user("7")}),
    );
    let Some(CachedMemberEvent::Remove(removed)) = removed else {
        panic!("expected cached removal after resume");
    };
    assert_eq!(removed.member.user.id, Id::new(7));
    assert!(removed.member.roles.is_empty());
    add(first, "4", "8");
    observe(
        first,
        "GUILD_DELETE",
        json!({"id":"4", "unavailable": true}),
    );
    assert!(
        observe(
            first,
            "GUILD_MEMBER_REMOVE",
            json!({"guild_id":"4", "user":user("8")})
        )
        .is_none()
    );
    assert!(matches!(
        observe(
            second,
            "GUILD_MEMBER_REMOVE",
            json!({"guild_id":"4194304", "user":user("6")})
        ),
        Some(CachedMemberEvent::Remove(_))
    ));
}

#[test]
fn replacement_ready_and_missing_updates_reset_only_the_affected_shard() {
    let runtime = MemberCacheRuntime::prepare(3, &subscribed(), INTENTS, None, None)
        .unwrap()
        .unwrap();
    let first = runtime.shard(ShardId::new(0, 2));
    let second = runtime.shard(ShardId::new(1, 2));
    let edit = |guild, id| json!({"guild_id": guild, "user":user(id), "roles": []});
    add(first, "2", "6");
    add(second, "4194304", "7");
    let invalid_update = r#"{"op":0,"s":4,"t":"GUILD_MEMBER_UPDATE","d":{"guild_id":"2","user":{"id":"6","username":"member","discriminator":"0"}}}"#;
    let Some(Err(error)) = decode_text_with_options(
        invalid_update.to_owned(),
        EventTypeFlags::MEMBER_UPDATE,
        GatewayDecodeOptions {
            capture_member_fields: true,
            ..GatewayDecodeOptions::default()
        },
    ) else {
        panic!("expected invalid update");
    };
    first.invalidate_on_decode_error(&error);
    assert!(update(first, edit("2", "6")).is_none());
    assert!(update(second, edit("4194304", "7")).is_some());

    add(first, "2", "6");
    let IncomingEvent::Standard(ready) = raw(
        "READY",
        json!({
            "application": {"id":"1", "flags":0}, "guilds": [],
            "resume_gateway_url": "wss://gateway.discord.gg",
            "session_id": "replacement", "shard": [1, 2], "v": 10,
            "user": { "id":"1", "username":"bot", "discriminator":"0", "bot":true, "mfa_enabled":false }
        }),
        EventTypeFlags::READY,
        false,
    ) else {
        panic!("expected ready");
    };
    second.observe(&ready);
    assert!(update(second, edit("4194304", "7")).is_none());
    assert!(update(first, edit("2", "6")).is_some());
}

#[tokio::test]
async fn dispatches_raw_then_cached_in_fifo_and_keeps_raw_cache_misses() {
    let received = Arc::new(Mutex::new(Vec::new()));
    let raw_updates = Arc::clone(&received);
    let cached_updates = Arc::clone(&received);
    let mut events = Events::new();
    events.on_gateway(async move |update: &MemberUpdate| -> Result<()> {
        raw_updates
            .lock()
            .unwrap()
            .push(("raw", update.user.id.get()));
        Ok(())
    });
    events.on_member_update(async move |update: &CachedMemberUpdate| -> Result<()> {
        assert_eq!(update.before.nick.as_deref(), Some("before"));
        assert_eq!(update.after.nick.as_deref(), Some("after"));
        cached_updates
            .lock()
            .unwrap()
            .push(("cached", update.after.user.id.get()));
        Ok(())
    });
    let runtime = MemberCacheRuntime::prepare(1, &events, INTENTS, None, None)
        .unwrap()
        .unwrap();
    let route = runtime.shard(ShardId::ONE);
    let (tx, rx) = mpsc::channel(3);
    let hooks = Hooks::default();
    let permissions = PermissionResolver::new(None);
    for (kind, payload) in [
        (
            "GUILD_MEMBER_ADD",
            json!({"guild_id":"7", "user":user("6"), "deaf":false, "mute":false, "flags":0, "roles":[], "nick":"before"}),
        ),
        (
            "GUILD_MEMBER_UPDATE",
            json!({"guild_id":"7", "user":user("6"), "roles":[], "nick":"after"}),
        ),
        (
            "GUILD_MEMBER_UPDATE",
            json!({"guild_id":"7", "user":user("8"), "roles":[]}),
        ),
    ] {
        let event = raw(kind, payload, runtime.event_types(), true);
        route_gateway_event(
            Ok(event),
            &events,
            &permissions,
            &tx,
            &hooks.reporter,
            ShardId::ONE,
            (None, Some(route)),
        );
    }
    drop(tx);
    dispatch_events(
        rx,
        None::<NoMessageHandler>,
        &events,
        &Client::new("test".into()),
        &permissions,
        &hooks,
    )
    .await;
    assert_eq!(
        *received.lock().unwrap(),
        [("raw", 6), ("cached", 6), ("raw", 8)]
    );
}
