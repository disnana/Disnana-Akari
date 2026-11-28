use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::mpsc,
};
use twilight_gateway::ShardId;
use twilight_model::gateway::payload::incoming::{ChannelUpdate, RoleDelete};

use super::{
    Client, Event, EventTypeFlags, Events, Id, Intents, Message, NoMessageHandler,
    PermissionResolver, Permissions, Result, dispatch_events, gateway_event_types,
    hooks::Hooks,
    message_events::{IncomingEvent, decode_text},
    permission_cache_events, route_gateway_event, update_permission_cache,
};

#[test]
fn cache_events_require_the_caller_to_enable_guilds() {
    assert!(permission_cache_events(false, Intents::GUILDS).is_empty());
    assert_eq!(
        permission_cache_events(true, Intents::empty()),
        EventTypeFlags::RESUMED
    );
    let cache_events = permission_cache_events(true, Intents::GUILDS);
    assert!(gateway_event_types(true, true, cache_events).contains(
        EventTypeFlags::READY
            | EventTypeFlags::RESUMED
            | EventTypeFlags::MESSAGE_CREATE
            | EventTypeFlags::INTERACTION_CREATE,
    ));
    assert!(cache_events.contains(
        EventTypeFlags::GUILD_UPDATE
            | EventTypeFlags::GUILD_DELETE
            | EventTypeFlags::ROLE_CREATE
            | EventTypeFlags::ROLE_UPDATE
            | EventTypeFlags::ROLE_DELETE
            | EventTypeFlags::CHANNEL_UPDATE
            | EventTypeFlags::CHANNEL_DELETE
            | EventTypeFlags::THREAD_UPDATE
            | EventTypeFlags::THREAD_DELETE
            | EventTypeFlags::RESUMED,
    ));
}

#[tokio::test]
async fn gateway_updates_refresh_only_the_needed_rest_entries() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = Arc::new(
        Client::builder()
            .proxy(listener.local_addr().unwrap().to_string(), true)
            .build(),
    );
    let resolver = Arc::new(PermissionResolver::new(Some(Duration::from_secs(60))));
    let allowed = Permissions::MANAGE_MESSAGES | Permissions::BAN_MEMBERS;

    let server = async {
        reply(&listener, "/channels/12/messages/100", MESSAGE).await;
        reply(&listener, "/channels/11", &parent(true)).await;
        // Exact request order catches duplicate fetches and accidental invalidation of unaffected keys.
        for (path, body) in [
            ("/guilds/10", guild(allowed)),
            ("/channels/12", thread()),
            ("/channels/11", parent(false)),
            ("/channels/11", parent(true)),
            ("/guilds/10", guild(Permissions::empty())),
            ("/guilds/10", guild(Permissions::empty())),
            ("/channels/12", thread()),
            ("/channels/11", parent(true)),
        ] {
            reply(&listener, path, &body).await;
        }
    };
    let commands = async {
        let message = Arc::new(message(&http).await);
        let modified_parent = http
            .channel(Id::new(11))
            .await
            .unwrap()
            .model()
            .await
            .unwrap();
        let (first, second) = tokio::join!(
            resolver.resolve(&http, &message),
            resolver.resolve(&http, &message),
        );
        assert!(first.unwrap().contains(allowed));
        assert!(second.unwrap().contains(allowed));

        update_permission_cache(&resolver, &role_deleted(20), twilight_gateway::ShardId::ONE);
        assert!(
            resolver
                .resolve(&http, &message)
                .await
                .unwrap()
                .contains(allowed)
        );

        let parent_update = Event::ChannelUpdate(Box::new(ChannelUpdate(modified_parent)));
        update_permission_cache(&resolver, &parent_update, twilight_gateway::ShardId::ONE);
        let held = resolver.resolve(&http, &message).await.unwrap();
        assert!(!held.contains(Permissions::MANAGE_MESSAGES));
        assert!(held.contains(Permissions::BAN_MEMBERS));

        let checked = Arc::new(AtomicBool::new(false));
        let handler_checked = Arc::clone(&checked);
        let handler_http = Arc::clone(&http);
        let handler_resolver = Arc::clone(&resolver);
        let handler_message = Arc::clone(&message);
        let mut events = Events::new();
        events.on_gateway(async move |role: &RoleDelete| -> Result<()> {
            assert_eq!(role.guild_id, Id::new(10));
            let held = handler_resolver
                .resolve(&handler_http, &handler_message)
                .await;
            handler_checked.store(permissions_unavailable(held), Ordering::Relaxed);
            Ok(())
        });
        let hooks = Hooks::default();
        let (tx, rx) = mpsc::channel(1);
        let incoming = IncomingEvent::Standard(role_deleted(10));
        assert!(
            route_gateway_event(
                Ok(incoming),
                &events,
                &resolver,
                &tx,
                &hooks.reporter,
                twilight_gateway::ShardId::ONE,
                (None, None, None),
            )
            .is_none()
        );
        drop(tx);
        dispatch_events(
            rx,
            None::<NoMessageHandler>,
            &events,
            &http,
            &resolver,
            &hooks,
        )
        .await;
        assert!(checked.load(Ordering::Relaxed));
        assert!(permissions_unavailable(
            resolver.resolve(&http, &message).await
        ));

        update_permission_cache(&resolver, &Event::Resumed, twilight_gateway::ShardId::ONE);
        assert!(permissions_unavailable(
            resolver.resolve(&http, &message).await
        ));
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(commands, server);
    })
    .await
    .expect("permission checks and mock HTTP requests must complete");
}

#[tokio::test]
async fn role_delete_during_rest_fetch_prevents_using_the_old_permissions() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = Client::builder()
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .build();
    let resolver = PermissionResolver::new(Some(Duration::from_secs(60)));
    let server = async {
        reply(&listener, "/channels/12/messages/100", MESSAGE).await;
        let request = read_request(&listener, "/guilds/10").await;
        update_permission_cache(&resolver, &role_deleted(10), twilight_gateway::ShardId::ONE);
        write_response(request, &guild(Permissions::MANAGE_MESSAGES)).await;
        reply(&listener, "/channels/11", &parent(false)).await;
        reply(&listener, "/guilds/10", &guild(Permissions::empty())).await;
    };
    let commands = async {
        let mut message = message(&http).await;
        message.channel_id = Id::new(11);
        assert!(permissions_unavailable(
            resolver.resolve(&http, &message).await
        ));
        assert!(permissions_unavailable(
            resolver.resolve(&http, &message).await
        ));
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(commands, server);
    })
    .await
    .expect("the retry and cache hit must complete");
}

#[tokio::test]
async fn malformed_role_update_invalidates_only_its_shard() {
    const SECOND_GUILD: u64 = (1 << 22) + 10;
    const SECOND_CHANNEL: u64 = 21;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = Client::builder()
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .build();
    let resolver = PermissionResolver::new(Some(Duration::from_secs(60)));
    let allowed = Permissions::BAN_MEMBERS;
    let server = async {
        reply(&listener, "/guilds/10", &guild(allowed)).await;
        reply(&listener, "/channels/11", &parent(false)).await;
        reply(
            &listener,
            &format!("/guilds/{SECOND_GUILD}"),
            &guild_for(SECOND_GUILD, allowed),
        )
        .await;
        reply(
            &listener,
            &format!("/channels/{SECOND_CHANNEL}"),
            &format!(r#"{{"id":"{SECOND_CHANNEL}","type":0,"guild_id":"{SECOND_GUILD}"}}"#),
        )
        .await;
        reply(&listener, "/guilds/10", &guild(Permissions::empty())).await;
        reply(&listener, "/channels/11", &parent(false)).await;
    };
    let checks = async {
        let mut first: Message = serde_json::from_str(MESSAGE).unwrap();
        first.channel_id = Id::new(11);
        let mut second = first.clone();
        second.guild_id = Some(Id::new(SECOND_GUILD));
        second.channel_id = Id::new(SECOND_CHANNEL);
        for message in [&first, &second] {
            assert!(
                resolver
                    .resolve(&http, message)
                    .await
                    .unwrap()
                    .contains(allowed)
            );
        }

        let events = Events::new();
        let hooks = Hooks::default();
        let (tx, _) = mpsc::channel(1);
        let route_invalid = |name, kind| {
            let body = format!(r#"{{"op":0,"t":"{name}","s":1,"d":{{}}}}"#);
            let event = decode_text(body, kind, false).unwrap();
            assert!(event.is_err());
            assert!(
                route_gateway_event(
                    event,
                    &events,
                    &resolver,
                    &tx,
                    &hooks.reporter,
                    ShardId::new(0, 2),
                    (None, None, None)
                )
                .is_none()
            );
        };
        route_invalid("MESSAGE_CREATE", EventTypeFlags::MESSAGE_CREATE);
        assert!(
            resolver
                .resolve(&http, &first)
                .await
                .unwrap()
                .contains(allowed)
        );
        route_invalid("GUILD_ROLE_UPDATE", EventTypeFlags::ROLE_UPDATE);
        assert!(
            resolver
                .resolve(&http, &second)
                .await
                .unwrap()
                .contains(allowed)
        );
        assert!(permissions_unavailable(
            resolver.resolve(&http, &first).await
        ));
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(checks, server);
    })
    .await
    .unwrap();
}

fn permissions_unavailable(held: Result<Permissions>) -> bool {
    matches!(
        held,
        Err(crate::Error::Check(
            crate::CheckFailure::PermissionsUnavailable
        ))
    )
}

fn role_deleted(guild: u64) -> Event {
    Event::RoleDelete(RoleDelete {
        guild_id: Id::new(guild),
        role_id: Id::new(60),
    })
}

const MESSAGE: &str = r#"{
        "id": "100", "guild_id": "10", "channel_id": "12", "type": 0,
        "author": {"id": "2", "username": "member", "discriminator": "0"},
        "member": {"roles": ["60"], "deaf": false, "mute": false, "flags": 0},
        "content": "!probe", "timestamp": "2026-01-01T00:00:00.000000+00:00", "attachments": [],
        "embeds": [], "mention_everyone": false, "mention_roles": [], "mentions": [],
        "pinned": false, "tts": false
    }"#;

async fn message(http: &Client) -> Message {
    http.message(Id::new(12), Id::new(100))
        .await
        .unwrap()
        .model()
        .await
        .unwrap()
}

fn guild(permissions: Permissions) -> String {
    guild_for(10, permissions)
}

fn guild_for(id: u64, permissions: Permissions) -> String {
    let everyone = Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES;
    let base_role = role(id, everyone);
    let roles = if permissions.is_empty() {
        base_role
    } else {
        format!("{base_role},{}", role(60, permissions))
    };
    format!(
        r#"{{
        "id": "{id}", "owner_id": "1", "name": "test", "afk_timeout": 60,
        "default_message_notifications": 0, "explicit_content_filter": 0, "features": [],
        "mfa_level": 0, "nsfw_level": 0, "preferred_locale": "ja", "verification_level": 0,
        "premium_progress_bar_enabled": false, "system_channel_flags": 0,
        "roles": [{roles}]
    }}"#
    )
}

fn role(id: u64, permissions: Permissions) -> String {
    format!(
        r#"{{
        "id": "{id}", "name": "test role", "color": 0, "colors": {{"primary_color": 0}},
        "hoist": false, "managed": false, "mentionable": false, "position": 1, "flags": 0,
        "permissions": "{}"
    }}"#,
        permissions.bits()
    )
}

fn thread() -> String {
    r#"{"id": "12", "type": 11, "guild_id": "10", "parent_id": "11"}"#.to_owned()
}

fn parent(deny_manage: bool) -> String {
    let deny = if deny_manage {
        Permissions::MANAGE_MESSAGES
    } else {
        Permissions::empty()
    };
    format!(
        r#"{{
        "id": "11", "type": 0, "guild_id": "10",
        "permission_overwrites": [{{"id": "60", "type": 0, "allow": "0", "deny": "{}"}}]
    }}"#,
        deny.bits()
    )
}

async fn reply(listener: &TcpListener, expected_path: &str, body: &str) {
    let stream = read_request(listener, expected_path).await;
    write_response(stream, body).await;
}

async fn read_request(listener: &TcpListener, expected_path: &str) -> BufReader<TcpStream> {
    let (stream, _) = listener.accept().await.unwrap();
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    stream.read_line(&mut line).await.unwrap();
    let mut parts = line.split_whitespace();
    assert_eq!(parts.next(), Some("GET"));
    let path = parts.next().unwrap().split('?').next().unwrap();
    assert!(
        path.ends_with(expected_path),
        "expected {expected_path}, received {path}"
    );
    loop {
        line.clear();
        if stream.read_line(&mut line).await.unwrap() == 0 || line == "\r\n" {
            break;
        }
    }
    stream
}

async fn write_response(mut stream: BufReader<TcpStream>, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .get_mut()
        .write_all(response.as_bytes())
        .await
        .unwrap();
}
