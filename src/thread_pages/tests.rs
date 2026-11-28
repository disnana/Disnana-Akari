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

async fn reply(listener: &TcpListener, path: &str, body: Value) {
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = BufReader::new(socket);
    let mut line = String::new();
    socket.read_line(&mut line).await.unwrap();
    assert_eq!(line.trim_end(), format!("GET {path} HTTP/1.1"));
    while !line.trim_end().is_empty() {
        line.clear();
        assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
    }
    let body = body.to_string();
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

fn thread(id: u64, kind: u8, day: u8) -> Value {
    json!({
        "id": id.to_string(),
        "type": kind,
        "parent_id": "2",
        "thread_metadata": {
            "archived": true,
            "auto_archive_duration": 60,
            "archive_timestamp": format!("2026-01-{day:02}T00:00:00.000000+00:00"),
            "locked": false
        }
    })
}

fn listing(threads: &[Value], members: &[u64], has_more: bool) -> Value {
    let members: Vec<_> = members
        .iter()
        .map(|id| {
            json!({
                "id": id.to_string(),
                "user_id": "1",
                "flags": 0,
                "join_timestamp": "2026-01-01T00:00:00.000000+00:00"
            })
        })
        .collect();
    json!({ "threads": threads, "members": members, "has_more": has_more })
}

fn ids(page: ThreadsListing) -> Vec<u64> {
    page.threads.iter().map(|thread| thread.id.get()).collect()
}

#[tokio::test]
async fn public_wire_cursor_and_total_limit_keep_matching_members() {
    let (listener, http) = local().await;
    let mut pages = ThreadPages::public(
        &http,
        Id::new(2),
        Some(Timestamp::parse("2026-01-04T00:00:00.123456+00:00").unwrap()),
    )
    .page_size(NonZeroU16::new(2).unwrap())
    .limit(3);
    let server = tokio::spawn(async move {
        reply(
            &listener,
            "/api/v10/channels/2/threads/archived/public?before=2026-01-04T00:00:00.123456Z&limit=2",
            listing(&[thread(8, 10, 2), thread(7, 11, 2)], &[8, 7], true),
        )
        .await;
        reply(
            &listener,
            "/api/v10/channels/2/threads/archived/public?before=2026-01-02T00:00:00.000000Z&limit=2",
            listing(&[thread(6, 11, 1), thread(5, 11, 1)], &[6, 5], true),
        )
        .await;
    });
    timeout(Duration::from_secs(3), async {
        assert_eq!(ids(pages.next_page().await.unwrap().unwrap()), [8, 7]);
        let last = pages.next_page().await.unwrap().unwrap();
        assert_eq!(
            last.members
                .iter()
                .filter_map(|member| member.id)
                .collect::<Vec<_>>(),
            [Id::new(6)]
        );
        assert_eq!(last.has_more, Some(true));
        assert_eq!(ids(last), [6]);
        assert!(pages.next_page().await.unwrap().is_none());
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn private_short_page_with_has_more_fetches_again() {
    let (listener, http) = local().await;
    let mut pages =
        ThreadPages::private(&http, Id::new(2), None).page_size(NonZeroU16::new(u16::MAX).unwrap());
    let server = tokio::spawn(async move {
        reply(
            &listener,
            "/api/v10/channels/2/threads/archived/private?limit=100",
            listing(&[thread(7, 12, 6)], &[], true),
        )
        .await;
        reply(
            &listener,
            "/api/v10/channels/2/threads/archived/private?before=2026-01-06T00:00:00.000000Z&limit=100",
            listing(&[], &[], false),
        )
        .await;
    });
    timeout(Duration::from_secs(3), async {
        assert_eq!(ids(pages.next_page().await.unwrap().unwrap()), [7]);
        assert!(pages.next_page().await.unwrap().is_none());
        assert!(pages.next_page().await.unwrap().is_none());
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn joined_private_rejects_nonprogress_then_continues_by_id() {
    let (listener, http) = local().await;
    let mut pages = ThreadPages::joined_private(&http, Id::new(2), Some(Id::new(10)))
        .page_size(NonZeroU16::new(1).unwrap());
    let server = tokio::spawn(async move {
        let path = "/api/v10/channels/2/users/@me/threads/archived/private?before=10&limit=2";
        for invalid in [
            listing(&[thread(10, 12, 2)], &[], true),
            listing(&[], &[], true),
            json!({ "threads": [], "members": [] }),
            listing(
                &[thread(9, 12, 1), thread(7, 12, 2), thread(6, 12, 3)],
                &[],
                true,
            ),
        ] {
            reply(&listener, path, invalid).await;
        }
        reply(
            &listener,
            path,
            listing(&[thread(9, 12, 1), thread(7, 12, 2)], &[], true),
        )
        .await;
        reply(
            &listener,
            "/api/v10/channels/2/users/@me/threads/archived/private?before=7&limit=2",
            listing(&[thread(6, 12, 3)], &[], false),
        )
        .await;
    });
    timeout(Duration::from_secs(3), async {
        for _ in 0..4 {
            assert!(matches!(
                pages.next_page().await,
                Err(ThreadPagesError::InvalidPage)
            ));
        }
        assert_eq!(ids(pages.next_page().await.unwrap().unwrap()), [9, 7]);
        assert_eq!(ids(pages.next_page().await.unwrap().unwrap()), [6]);
        assert!(pages.next_page().await.unwrap().is_none());
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn zero_total_does_not_contact_discord() {
    let (listener, http) = local().await;
    let mut pages = ThreadPages::private(&http, Id::new(2), None).limit(0);
    drop(listener);
    assert!(pages.next_page().await.unwrap().is_none());
}
