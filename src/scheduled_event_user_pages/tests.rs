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
        format!("GET /api/v10/guilds/2/scheduled-events/3/users?{query} HTTP/1.1")
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

fn user(id: u64) -> Value {
    json!({ "id": id.to_string(), "username": "subscriber", "discriminator": "0" })
}

fn subscriber(event: u64, id: u64, member_id: Option<u64>) -> Value {
    let member = member_id.map(
        |id| json!({ "user": user(id), "deaf": false, "mute": false, "flags": 0, "roles": [] }),
    );
    json!({ "guild_scheduled_event_id": event.to_string(), "user": user(id), "member": member })
}

async fn page(listener: &TcpListener, query: &str, users: &[Value]) {
    reply(listener, query, "200 OK", &json!(users).to_string()).await;
}

fn ids(users: Vec<GuildScheduledEventUser>) -> Vec<u64> {
    users.into_iter().map(|entry| entry.user.id.get()).collect()
}

#[tokio::test]
async fn after_uses_maximum_id_and_requests_exact_remaining_count() {
    let (listener, http) = local().await;
    let mut pages = ScheduledEventUserPages::new(&http, Id::new(2), Id::new(3))
        .before(Id::new(99))
        .after(Id::new(4))
        .with_member(true)
        .page_size(NonZeroU16::new(2).unwrap())
        .limit(3);
    let server = tokio::spawn(async move {
        page(
            &listener,
            "after=4&limit=2&with_member=true",
            &[subscriber(3, 6, None), subscriber(3, 5, Some(5))],
        )
        .await;
        page(
            &listener,
            "after=6&limit=1&with_member=true",
            &[subscriber(3, 9, None)],
        )
        .await;
    });
    timeout(Duration::from_secs(5), async {
        let first = pages.next_page().await.unwrap().unwrap();
        let member = first.first().and_then(|entry| entry.member.as_ref());
        assert_eq!(member.map(|member| member.user.id.get()), Some(5));
        assert_eq!(ids(first), [5, 6]);
        assert_eq!(ids(pages.next_page().await.unwrap().unwrap()), [9]);
        assert!(pages.next_page().await.unwrap().is_none());
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn before_retries_same_cursor_then_uses_minimum_id() {
    let (listener, http) = local().await;
    let mut pages = ScheduledEventUserPages::new(&http, Id::new(2), Id::new(3))
        .before(Id::new(20))
        .page_size(NonZeroU16::new(2).unwrap())
        .limit(3);
    let server = tokio::spawn(async move {
        let query = "before=20&limit=2";
        reply(
            &listener,
            query,
            "403 Forbidden",
            r#"{"code":50001,"message":"Missing Access"}"#,
        )
        .await;
        reply(&listener, query, "200 OK", "{}").await;
        for bad in [
            vec![subscriber(4, 18, None)],
            vec![subscriber(3, 18, Some(19))],
            vec![subscriber(3, 18, None), subscriber(3, 18, None)],
            vec![subscriber(3, 20, None)],
            vec![
                subscriber(3, 17, None),
                subscriber(3, 18, None),
                subscriber(3, 19, None),
            ],
        ] {
            page(&listener, query, &bad).await;
        }
        page(
            &listener,
            query,
            &[subscriber(3, 17, None), subscriber(3, 18, None)],
        )
        .await;
        page(&listener, "before=17&limit=1", &[subscriber(3, 16, None)]).await;
    });
    timeout(Duration::from_secs(5), async {
        assert!(matches!(
            pages.next_page().await,
            Err(ScheduledEventUserPagesError::Http(_))
        ));
        assert!(matches!(
            pages.next_page().await,
            Err(ScheduledEventUserPagesError::HttpBody(_))
        ));
        for _ in 0..5 {
            assert!(matches!(
                pages.next_page().await,
                Err(ScheduledEventUserPagesError::InvalidPage)
            ));
        }
        assert_eq!(ids(pages.next_page().await.unwrap().unwrap()), [17, 18]);
        assert_eq!(ids(pages.next_page().await.unwrap().unwrap()), [16]);
        assert!(pages.next_page().await.unwrap().is_none());
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn page_size_is_capped_and_short_page_ends_iteration() {
    let (listener, http) = local().await;
    let mut pages = ScheduledEventUserPages::new(&http, Id::new(2), Id::new(3))
        .page_size(NonZeroU16::new(u16::MAX).unwrap());
    let server = tokio::spawn(async move {
        page(&listener, "limit=100", &[subscriber(3, 5, None)]).await;
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
async fn zero_total_skips_request() {
    let (listener, http) = local().await;
    let mut pages = ScheduledEventUserPages::new(&http, Id::new(2), Id::new(3)).limit(0);
    drop(listener);
    assert!(pages.next_page().await.unwrap().is_none());
}
