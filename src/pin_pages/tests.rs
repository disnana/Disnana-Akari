use std::time::Duration;

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::timeout,
};

use super::*;

const CHANNEL: Id<ChannelMarker> = Id::new(2);
const EARLY: &str = "2026-01-01T00:00:00.000001Z";
const MIDDLE: &str = "2026-01-01T00:00:00.000002Z";
const LATE: &str = "2026-01-01T00:00:00.000003Z";

fn pin(id: u64, channel: Id<ChannelMarker>, time: &str) -> Value {
    json!({
        "pinned_at": time.replace('Z', "+00:00"),
        "message": {
            "id": id.to_string(), "channel_id": channel.to_string(), "type": 0,
            "author": {"id": "4", "username": "fixture", "discriminator": "0"},
            "content": "", "timestamp": "2025-12-01T00:00:00+00:00",
            "attachments": [], "embeds": [], "mention_everyone": false, "mention_roles": [],
            "mentions": [], "pinned": true, "tts": false
        }
    })
}

async fn reply(listener: &TcpListener, query: &str, items: &[Value], has_more: bool) {
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = BufReader::new(socket);
    let mut line = String::new();
    socket.read_line(&mut line).await.unwrap();
    assert_eq!(
        line.trim_end(),
        format!("GET /api/v10/channels/2/messages/pins?{query} HTTP/1.1")
    );
    while !line.trim_end().is_empty() {
        line.clear();
        assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
    }
    let body = json!({"items": items, "has_more": has_more}).to_string();
    let reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    );
    socket.get_mut().write_all(reply.as_bytes()).await.unwrap();
}

#[tokio::test]
async fn short_page_with_more_keeps_going_and_retries_invalid_page() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = Client::builder()
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .build();
    let mut pages = PinPages::new(&http, CHANNEL)
        .limit(3)
        .page_size(NonZeroU16::new(2).unwrap());
    let server = async {
        reply(&listener, "limit=2", &[pin(8, CHANNEL, LATE)], true).await;
        let next = format!("before={LATE}&limit=2");
        reply(&listener, &next, &[pin(6, CHANNEL, LATE)], true).await;
        reply(&listener, &next, &[pin(6, CHANNEL, MIDDLE)], true).await;
        reply(
            &listener,
            &format!("before={MIDDLE}&limit=1"),
            &[pin(9, CHANNEL, EARLY)],
            false,
        )
        .await;
    };
    let consume = async {
        let items = pages.next_page().await.unwrap().unwrap();
        assert_eq!(items[0].message.id.get(), 8);
        assert_eq!(
            items[0].pinned_at,
            Timestamp::parse("2026-01-01T00:00:00.000003+00:00").unwrap()
        );
        assert!(matches!(
            pages.next_page().await,
            Err(PinPagesError::InvalidPage)
        ));
        assert_eq!(
            pages.next_page().await.unwrap().unwrap()[0]
                .message
                .id
                .get(),
            6
        );
        assert_eq!(
            pages.next_page().await.unwrap().unwrap()[0]
                .message
                .id
                .get(),
            9
        );
        assert!(pages.next_page().await.unwrap().is_none());
    };
    timeout(Duration::from_secs(5), async {
        tokio::join!(server, consume);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn zero_limit_finishes_without_network_and_wrong_channel_is_rejected() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = Client::builder()
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .build();
    assert!(
        PinPages::new(&http, CHANNEL)
            .limit(0)
            .next_page()
            .await
            .unwrap()
            .is_none()
    );

    let mut pages = PinPages::new(&http, CHANNEL).page_size(NonZeroU16::MAX);
    let wrong_channel = [pin(8, Id::new(10), EARLY)];
    let server = reply(&listener, "limit=50", &wrong_channel, false);
    let consume = async {
        assert!(matches!(
            pages.next_page().await,
            Err(PinPagesError::InvalidPage)
        ));
    };
    timeout(Duration::from_secs(5), async {
        tokio::join!(server, consume);
    })
    .await
    .unwrap();
}
