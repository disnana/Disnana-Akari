use std::{future::Future, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::timeout,
};

use super::*;

const APP: Id<ApplicationMarker> = Id::new(2);
const USER: Id<UserMarker> = Id::new(3);
const GUILD: Id<GuildMarker> = Id::new(4);
const SKUS: [Id<SkuMarker>; 2] = [Id::new(5), Id::new(6)];

async fn local() -> (TcpListener, Client) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = Client::builder()
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .build();
    (listener, http)
}

fn raw(id: u64) -> Value {
    json!({
        "application_id": "2", "id": id.to_string(), "sku_id": "5", "user_id": "3",
        "guild_id": "4", "type": 8, "deleted": false, "consumed": false
    })
}

fn list(ids: &[u64]) -> Vec<Value> {
    ids.iter().copied().map(raw).collect()
}

async fn reply(listener: &TcpListener, expected_query: &[&str], body: Vec<Value>) {
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = BufReader::new(socket);
    let mut line = String::new();
    socket.read_line(&mut line).await.unwrap();
    let request = line
        .trim_end()
        .strip_prefix("GET /api/v10/applications/2/entitlements?")
        .and_then(|path| path.strip_suffix(" HTTP/1.1"))
        .unwrap();
    let mut actual: Vec<_> = request.split('&').filter(|item| !item.is_empty()).collect();
    actual.sort_unstable();
    assert_eq!(actual, expected_query);
    while !line.trim_end().is_empty() {
        line.clear();
        assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
    }
    let body = serde_json::to_string(&body).unwrap();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
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

async fn ids(pages: &mut EntitlementPages<'_>) -> Vec<u64> {
    pages
        .next_page()
        .await
        .unwrap()
        .unwrap()
        .iter()
        .map(|entitlement| entitlement.id.get())
        .collect()
}

#[tokio::test]
async fn default_scan_is_lazy_clamped_and_ends_after_short_page() {
    let (listener, http) = local().await;
    let mut empty = EntitlementPages::new(&http, APP).limit(0);
    assert!(empty.next_page().await.unwrap().is_none());
    let mut pages = EntitlementPages::new(&http, APP).page_size(NonZeroU16::MAX);
    assert!(
        timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );

    exchange(
        reply(
            &listener,
            &["exclude_deleted=true", "exclude_ended=false", "limit=100"],
            list(&[9, 7]),
        ),
        async {
            assert_eq!(ids(&mut pages).await, [7, 9]);
            assert!(pages.next_page().await.unwrap().is_none());
        },
    )
    .await;
}

#[tokio::test]
async fn after_carries_filters_independently_and_respects_total() {
    let (listener, http) = local().await;
    let mut pages = EntitlementPages::new(&http, APP)
        .before(Id::new(60))
        .after(Id::new(10))
        .user_id(USER)
        .guild_id(GUILD)
        .sku_ids(&SKUS)
        .exclude_ended(true)
        .exclude_deleted(false)
        .page_size(NonZeroU16::new(2).unwrap())
        .limit(3);
    let serve = async {
        let mut deleted = raw(12);
        deleted["deleted"] = json!(true);
        deleted["sku_id"] = json!("6");
        reply(
            &listener,
            &[
                "after=10",
                "exclude_deleted=false",
                "exclude_ended=true",
                "guild_id=4",
                "limit=2",
                "sku_ids=5,6",
                "user_id=3",
            ],
            vec![deleted, raw(11)],
        )
        .await;
        reply(
            &listener,
            &[
                "after=12",
                "exclude_deleted=false",
                "exclude_ended=true",
                "guild_id=4",
                "limit=1",
                "sku_ids=5,6",
                "user_id=3",
            ],
            list(&[13]),
        )
        .await;
    };
    exchange(serve, async {
        assert_eq!(ids(&mut pages).await, [11, 12]);
        assert_eq!(ids(&mut pages).await, [13]);
        assert!(pages.next_page().await.unwrap().is_none());
    })
    .await;
}

#[tokio::test]
async fn invalid_pages_keep_cursor_and_budget_and_before_uses_lowest_id() {
    let (listener, http) = local().await;
    let mut pages = EntitlementPages::new(&http, APP)
        .before(Id::new(20))
        .user_id(USER)
        .guild_id(GUILD)
        .sku_ids(&SKUS)
        .page_size(NonZeroU16::new(2).unwrap())
        .limit(3);
    let expected = [
        "before=20",
        "exclude_deleted=true",
        "exclude_ended=false",
        "guild_id=4",
        "limit=2",
        "sku_ids=5,6",
        "user_id=3",
    ];
    let serve = async {
        for (key, invalid) in [
            ("application_id", json!("9")),
            ("user_id", Value::Null),
            ("guild_id", json!("9")),
            ("sku_id", json!("9")),
            ("deleted", json!(true)),
        ] {
            let mut entitlement = raw(19);
            entitlement[key] = invalid;
            reply(&listener, &expected, vec![entitlement]).await;
        }
        for invalid in [&[20][..], &[19, 19][..], &[19, 18, 17][..]] {
            reply(&listener, &expected, list(invalid)).await;
        }
        reply(&listener, &expected, vec![json!({"id":"19"})]).await;
        reply(&listener, &expected, list(&[19, 18])).await;
        reply(
            &listener,
            &[
                "before=18",
                "exclude_deleted=true",
                "exclude_ended=false",
                "guild_id=4",
                "limit=1",
                "sku_ids=5,6",
                "user_id=3",
            ],
            list(&[17]),
        )
        .await;
    };
    exchange(serve, async {
        for _ in 0..8 {
            assert!(matches!(
                pages.next_page().await,
                Err(EntitlementPagesError::InvalidPage)
            ));
        }
        assert!(matches!(
            pages.next_page().await,
            Err(EntitlementPagesError::HttpBody(_))
        ));
        assert_eq!(ids(&mut pages).await, [18, 19]);
        assert_eq!(ids(&mut pages).await, [17]);
        assert!(pages.next_page().await.unwrap().is_none());
    })
    .await;
}
