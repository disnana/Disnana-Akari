use std::time::Duration;

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::timeout,
};

use super::*;

async fn local() -> (TcpListener, Client) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = Client::builder()
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .build();
    (listener, http)
}

async fn reply(listener: &TcpListener, query: &str, status: &str, body: &str) {
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = BufReader::new(socket);
    let mut line = String::new();
    socket.read_line(&mut line).await.unwrap();
    assert_eq!(
        line.trim_end(),
        format!("GET /api/v10/skus/2/subscriptions?{query} HTTP/1.1")
    );
    while !line.trim_end().is_empty() {
        line.clear();
        assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
    }
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    socket
        .get_mut()
        .write_all(response.as_bytes())
        .await
        .unwrap();
}

fn subscription(id: u64, user: u64, sku: u64) -> Value {
    json!({
        "id": id.to_string(),
        "user_id": user.to_string(),
        "sku_ids": [sku.to_string()],
        "entitlement_ids": [],
        "renewal_sku_ids": null,
        "current_period_start": "2024-08-27T19:48:44.406602+00:00",
        "current_period_end": "2024-09-27T19:48:44.406602+00:00",
        "status": 0,
        "canceled_at": null
    })
}

async fn page(listener: &TcpListener, query: &str, subscriptions: &[Value]) {
    reply(listener, query, "200 OK", &json!(subscriptions).to_string()).await;
}

fn ids(subscriptions: Vec<Subscription>) -> Vec<u64> {
    subscriptions
        .into_iter()
        .map(|subscription| subscription.id.get())
        .collect()
}

#[tokio::test]
async fn after_advances_and_requests_exact_remaining_total() {
    let (listener, http) = local().await;
    let mut pages = SubscriptionPages::new(&http, Id::new(2), Id::new(3))
        .before(Id::new(99))
        .after(Id::new(5))
        .page_size(NonZeroU16::new(2).unwrap())
        .limit(3);
    let server = tokio::spawn(async move {
        page(
            &listener,
            "after=5&limit=2&user_id=3",
            &[subscription(7, 3, 2), subscription(6, 3, 2)],
        )
        .await;
        page(
            &listener,
            "after=7&limit=1&user_id=3",
            &[subscription(8, 3, 2)],
        )
        .await;
    });
    timeout(Duration::from_secs(5), async {
        assert_eq!(ids(pages.next_page().await.unwrap().unwrap()), [6, 7]);
        assert_eq!(ids(pages.next_page().await.unwrap().unwrap()), [8]);
        assert!(pages.next_page().await.unwrap().is_none());
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn before_advances_only_after_a_valid_page() {
    let (listener, http) = local().await;
    let mut pages = SubscriptionPages::new(&http, Id::new(2), Id::new(3))
        .before(Id::new(10))
        .page_size(NonZeroU16::new(2).unwrap())
        .limit(3);
    let server = tokio::spawn(async move {
        let query = "before=10&limit=2&user_id=3";
        reply(
            &listener,
            query,
            "403 Forbidden",
            r#"{"code":50001,"message":"Missing Access"}"#,
        )
        .await;
        reply(&listener, query, "200 OK", "{}").await;
        for malformed in [
            vec![subscription(8, 4, 2)],
            vec![subscription(8, 3, 4)],
            vec![subscription(10, 3, 2)],
            vec![subscription(8, 3, 2), subscription(8, 3, 2)],
            vec![
                subscription(6, 3, 2),
                subscription(7, 3, 2),
                subscription(8, 3, 2),
            ],
        ] {
            page(&listener, query, &malformed).await;
        }
        page(
            &listener,
            query,
            &[subscription(8, 3, 2), subscription(9, 3, 2)],
        )
        .await;
        page(
            &listener,
            "before=8&limit=1&user_id=3",
            &[subscription(7, 3, 2)],
        )
        .await;
    });
    timeout(Duration::from_secs(5), async {
        assert!(matches!(
            pages.next_page().await,
            Err(SubscriptionPagesError::Http(_))
        ));
        assert!(matches!(
            pages.next_page().await,
            Err(SubscriptionPagesError::HttpBody(_))
        ));
        for _ in 0..5 {
            assert!(matches!(
                pages.next_page().await,
                Err(SubscriptionPagesError::InvalidPage)
            ));
        }
        assert_eq!(ids(pages.next_page().await.unwrap().unwrap()), [8, 9]);
        assert_eq!(ids(pages.next_page().await.unwrap().unwrap()), [7]);
        assert!(pages.next_page().await.unwrap().is_none());
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn page_size_is_capped_and_short_page_ends_iteration() {
    let (listener, http) = local().await;
    let mut pages = SubscriptionPages::new(&http, Id::new(2), Id::new(3))
        .page_size(NonZeroU16::new(u16::MAX).unwrap());
    let server = tokio::spawn(async move {
        page(&listener, "limit=100&user_id=3", &[subscription(5, 3, 2)]).await;
    });
    timeout(Duration::from_secs(5), async {
        assert_eq!(ids(pages.next_page().await.unwrap().unwrap()), [5]);
        assert!(pages.next_page().await.unwrap().is_none());
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn zero_limit_skips_request() {
    let (listener, http) = local().await;
    let mut pages = SubscriptionPages::new(&http, Id::new(2), Id::new(3)).limit(0);
    drop(listener);
    assert!(pages.next_page().await.unwrap().is_none());
}
