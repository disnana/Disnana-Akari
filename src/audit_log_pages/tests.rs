use std::{future::Future, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::timeout,
};

use super::*;

const GUILD: Id<GuildMarker> = Id::new(2);
const ACTOR: Id<UserMarker> = Id::new(7);

async fn local() -> (TcpListener, Client) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = Client::builder()
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .build();
    (listener, http)
}

fn raw_entry(id: u64, actor: u64, action: AuditLogEventType) -> Value {
    json!({
        "id": id.to_string(), "target_id": "88", "user_id": actor.to_string(),
        "action_type": u16::from(action), "reason": "fixture"
    })
}

fn entries(ids: &[u64]) -> Vec<Value> {
    ids.iter()
        .map(|&id| raw_entry(id, ACTOR.get(), AuditLogEventType::MemberBanAdd))
        .collect()
}

fn null_action(id: u64) -> Value {
    let mut entry = raw_entry(id, ACTOR.get(), AuditLogEventType::MemberBanAdd);
    entry["action_type"] = Value::Null;
    entry
}

async fn reply(listener: &TcpListener, expected_query: &[&str], entries: Vec<Value>) {
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = BufReader::new(socket);
    let mut line = String::new();
    socket.read_line(&mut line).await.unwrap();
    let request = line
        .trim_end()
        .strip_prefix("GET /api/v10/guilds/2/audit-logs?")
        .and_then(|path| path.strip_suffix(" HTTP/1.1"))
        .unwrap();
    let mut actual: Vec<_> = request.split('&').collect();
    actual.sort_unstable();
    assert_eq!(actual, expected_query);
    while !line.trim_end().is_empty() {
        line.clear();
        assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
    }
    let body = json!({
        "audit_log_entries": entries,
        "application_commands": [], "auto_moderation_rules": [], "guild_scheduled_events": [],
        "integrations": [], "webhooks": [],
        "threads": [{"id": "12", "type": 11, "guild_id": "2", "name": "thread"}],
        "users": [{"id":"7","username":"moderator","discriminator":"0"}]
    })
    .to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    );
    socket
        .get_mut()
        .write_all(response.as_bytes())
        .await
        .unwrap();
}

async fn exchange(serve: impl Future<Output = ()>, consume: impl Future<Output = ()>) {
    timeout(Duration::from_secs(5), async {
        tokio::join!(serve, consume);
    })
    .await
    .unwrap();
}

fn ids(page: &AuditLog) -> Vec<u64> {
    page.entries.iter().map(|entry| entry.id.get()).collect()
}

#[tokio::test]
async fn latest_is_lazy_and_retains_side_tables_while_paging_back() {
    let (listener, http) = local().await;
    let mut empty = AuditLogPages::new(&http, GUILD).limit(0);
    assert!(
        timeout(Duration::from_secs(1), empty.next_page())
            .await
            .unwrap()
            .unwrap()
            .is_none()
    );
    let mut pages = AuditLogPages::new(&http, GUILD)
        .page_size(NonZeroU16::new(2).unwrap())
        .limit(3);
    assert!(
        timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );

    let serve = async {
        reply(&listener, &["limit=2"], entries(&[16, 17])).await;
        reply(&listener, &["before=16", "limit=1"], entries(&[15])).await;
    };
    let consume = async {
        let page = pages.next_page().await.unwrap().unwrap();
        assert_eq!(ids(&page), [17, 16]);
        assert_eq!(page.users.first().unwrap().id, ACTOR);
        assert_eq!(page.threads.first().unwrap().id, Id::new(12));
        assert_eq!(ids(&pages.next_page().await.unwrap().unwrap()), [15]);
        assert!(pages.next_page().await.unwrap().is_none());
    };
    exchange(serve, consume).await;
}

#[tokio::test]
async fn after_carries_filters_and_advances_even_when_a_page_is_short() {
    let (listener, http) = local().await;
    let mut pages = AuditLogPages::new(&http, GUILD)
        .before(Id::new(100))
        .after(Id::new(10))
        .actor(ACTOR)
        .action(AuditLogEventType::MemberBanAdd)
        .limit(3);
    let serve = async {
        reply(
            &listener,
            &["action_type=22", "after=10", "limit=3", "user_id=7"],
            entries(&[13, 12]),
        )
        .await;
        reply(
            &listener,
            &["action_type=22", "after=13", "limit=1", "user_id=7"],
            entries(&[14]),
        )
        .await;
    };
    let consume = async {
        assert_eq!(ids(&pages.next_page().await.unwrap().unwrap()), [12, 13]);
        assert_eq!(ids(&pages.next_page().await.unwrap().unwrap()), [14]);
        assert!(pages.next_page().await.unwrap().is_none());
    };
    exchange(serve, consume).await;
}

#[tokio::test]
async fn oldest_first_uses_the_wire_zero_sentinel() {
    let (listener, http) = local().await;
    let mut pages = AuditLogPages::new(&http, GUILD)
        .before(Id::new(10))
        .oldest_first()
        .page_size(NonZeroU16::new(200).unwrap());
    exchange(
        reply(&listener, &["after=0", "limit=100"], Vec::new()),
        async {
            assert!(pages.next_page().await.unwrap().is_none());
            assert!(pages.next_page().await.unwrap().is_none());
        },
    )
    .await;
}

#[tokio::test]
async fn invalid_filtered_pages_can_retry_with_the_same_cursor_and_budget() {
    let (listener, http) = local().await;
    let mut pages = AuditLogPages::new(&http, GUILD)
        .before(Id::new(40))
        .actor(ACTOR)
        .action(AuditLogEventType::MemberBanAdd)
        .limit(2);
    let query = ["action_type=22", "before=40", "limit=2", "user_id=7"];
    let serve = async {
        reply(
            &listener,
            &query,
            vec![raw_entry(39, 6, AuditLogEventType::MemberBanAdd)],
        )
        .await;
        reply(
            &listener,
            &query,
            vec![raw_entry(39, 7, AuditLogEventType::GuildUpdate)],
        )
        .await;
        reply(&listener, &query, entries(&[40, 39])).await;
        reply(&listener, &query, entries(&[39, 38])).await;
    };
    let consume = async {
        for _ in 0..3 {
            assert!(matches!(
                pages.next_page().await,
                Err(AuditLogPagesError::InvalidPage)
            ));
        }
        assert_eq!(ids(&pages.next_page().await.unwrap().unwrap()), [39, 38]);
        assert!(pages.next_page().await.unwrap().is_none());
    };
    exchange(serve, consume).await;
}

#[tokio::test]
async fn null_actions_advance_the_wire_cursor_without_counting_toward_the_total() {
    let (listener, http) = local().await;
    let mut pages = AuditLogPages::new(&http, GUILD)
        .after(Id::new(10))
        .page_size(NonZeroU16::new(2).unwrap())
        .limit(3);
    let serve = async {
        reply(
            &listener,
            &["after=10", "limit=2"],
            vec![
                null_action(12),
                raw_entry(11, 7, AuditLogEventType::MemberBanAdd),
            ],
        )
        .await;
        reply(
            &listener,
            &["after=12", "limit=2"],
            vec![null_action(14), null_action(13)],
        )
        .await;
        reply(&listener, &["after=14", "limit=2"], entries(&[16, 15])).await;
    };
    let consume = async {
        let mixed = pages.next_page().await.unwrap().unwrap();
        assert_eq!(ids(&mixed), [11]);
        assert_eq!(mixed.users.first().unwrap().id, ACTOR);
        let skipped = pages.next_page().await.unwrap().unwrap();
        assert!(skipped.entries.is_empty());
        assert_eq!(skipped.threads.first().unwrap().id, Id::new(12));
        assert_eq!(ids(&pages.next_page().await.unwrap().unwrap()), [15, 16]);
        assert!(pages.next_page().await.unwrap().is_none());
    };
    exchange(serve, consume).await;
}

#[tokio::test]
async fn fallback_rejects_malformed_entries_and_duplicate_raw_ids_without_advancing() {
    let (listener, http) = local().await;
    let mut pages = AuditLogPages::new(&http, GUILD)
        .before(Id::new(20))
        .limit(2);
    let serve = async {
        let query = ["before=20", "limit=2"];
        let mut malformed = raw_entry(19, 7, AuditLogEventType::MemberBanAdd);
        malformed["user_id"] = json!("invalid-snowflake");
        reply(&listener, &query, vec![null_action(18), malformed]).await;
        reply(
            &listener,
            &query,
            vec![
                null_action(18),
                raw_entry(18, 7, AuditLogEventType::MemberBanAdd),
            ],
        )
        .await;
        reply(
            &listener,
            &query,
            vec![
                null_action(18),
                raw_entry(19, 7, AuditLogEventType::MemberBanAdd),
            ],
        )
        .await;
        reply(&listener, &["before=18", "limit=1"], entries(&[17])).await;
    };
    let consume = async {
        assert!(matches!(
            pages.next_page().await,
            Err(AuditLogPagesError::Decode(_))
        ));
        assert!(matches!(
            pages.next_page().await,
            Err(AuditLogPagesError::InvalidPage)
        ));
        assert_eq!(ids(&pages.next_page().await.unwrap().unwrap()), [19]);
        assert_eq!(ids(&pages.next_page().await.unwrap().unwrap()), [17]);
        assert!(pages.next_page().await.unwrap().is_none());
    };
    exchange(serve, consume).await;
}

#[test]
fn validates_duplicate_and_overfull_pages() {
    let entry = |id| {
        serde_json::from_value::<AuditLogEntry>(raw_entry(id, 7, AuditLogEventType::MemberBanAdd))
            .unwrap()
    };
    let after = Cursor::After(Some(Id::new(10)));
    assert!(matches!(
        after.advance(&mut [entry(11), entry(11)], 2, |entry| entry.id),
        Err(AuditLogPagesError::InvalidPage)
    ));
    assert!(matches!(
        after.advance(&mut [entry(11), entry(12)], 1, |entry| entry.id),
        Err(AuditLogPagesError::InvalidPage)
    ));
}
