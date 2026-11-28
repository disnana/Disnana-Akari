use std::{future::Future, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::timeout,
};

use super::*;

const CHANNEL: Id<ChannelMarker> = Id::new(2);

async fn local() -> (TcpListener, Client) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = Client::builder()
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .build();
    (listener, http)
}

fn raw_message(id: u64, channel: u64) -> Value {
    json!({
        "id": id.to_string(), "channel_id": channel.to_string(), "type": 0,
        "author": {"id": "3", "username": "member", "discriminator": "0"},
        "content": "", "timestamp": "2026-01-01T00:00:00.000000+00:00",
        "attachments": [], "embeds": [], "mention_everyone": false, "mention_roles": [],
        "mentions": [], "pinned": false, "tts": false
    })
}

async fn reply(listener: &TcpListener, expected_query: &[&str], ids: &[u64]) {
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = BufReader::new(socket);
    let mut line = String::new();
    socket.read_line(&mut line).await.unwrap();
    let request = line
        .trim_end()
        .strip_prefix("GET /api/v10/channels/2/messages?")
        .and_then(|path| path.strip_suffix(" HTTP/1.1"))
        .unwrap();
    let mut actual: Vec<_> = request.split('&').collect();
    actual.sort_unstable();
    assert_eq!(actual, expected_query);
    while !line.trim_end().is_empty() {
        line.clear();
        assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
    }
    let messages: Vec<_> = ids
        .iter()
        .map(|&id| raw_message(id, CHANNEL.get()))
        .collect();
    let body = serde_json::to_string(&messages).unwrap();
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

async fn page_ids(history: &mut MessageHistory<'_>) -> Vec<u64> {
    history
        .next_page()
        .await
        .unwrap()
        .unwrap()
        .into_iter()
        .map(|message| message.id.get())
        .collect()
}

#[tokio::test]
async fn latest_is_lazy_and_pages_back_with_an_exclusive_cursor_and_total_limit() {
    let (listener, http) = local().await;
    let mut history = MessageHistory::new(&http, CHANNEL, HistoryCursor::Latest, 102).unwrap();
    assert!(
        timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
    let range: Vec<_> = (5..=104).rev().collect();
    let serve = async {
        reply(&listener, &["limit=100"], &range).await;
        reply(&listener, &["before=5", "limit=2"], &[4, 3]).await;
    };
    let consume = async {
        assert_eq!(page_ids(&mut history).await, range);
        assert_eq!(page_ids(&mut history).await, [4, 3]);
        assert!(history.next_page().await.unwrap().is_none());
    };
    exchange(serve, consume).await;
}

#[tokio::test]
async fn after_follows_the_highest_id_even_when_pages_are_short() {
    let (listener, http) = local().await;
    let mut history =
        MessageHistory::new(&http, CHANNEL, HistoryCursor::After(Id::new(100)), 4).unwrap();
    let serve = async {
        reply(&listener, &["after=100", "limit=4"], &[102, 101]).await;
        reply(&listener, &["after=102", "limit=2"], &[103]).await;
        reply(&listener, &["after=103", "limit=1"], &[]).await;
    };
    let consume = async {
        assert_eq!(page_ids(&mut history).await, [101, 102]);
        assert_eq!(page_ids(&mut history).await, [103]);
        assert!(history.next_page().await.unwrap().is_none());
        assert!(history.next_page().await.unwrap().is_none());
    };
    exchange(serve, consume).await;
}

#[tokio::test]
async fn around_is_one_request_while_respecting_the_requested_total() {
    let (listener, http) = local().await;
    let origin = HistoryCursor::Around(Id::new(200));
    assert!(matches!(
        MessageHistory::new(&http, CHANNEL, origin, 101),
        Err(MessageHistoryError::AroundLimit)
    ));
    let mut empty = MessageHistory::new(&http, CHANNEL, origin, 0).unwrap();
    assert!(empty.next_page().await.unwrap().is_none());

    let mut history = MessageHistory::new(&http, CHANNEL, origin, 2).unwrap();
    exchange(
        reply(&listener, &["around=200", "limit=2"], &[201, 200, 199]),
        async {
            assert_eq!(page_ids(&mut history).await, [201, 200]);
            assert!(history.next_page().await.unwrap().is_none());
        },
    )
    .await;
}

#[tokio::test]
async fn a_malformed_page_preserves_the_cursor_for_a_retry() {
    let (listener, http) = local().await;
    let mut history =
        MessageHistory::new(&http, CHANNEL, HistoryCursor::Before(Id::new(10)), 2).unwrap();
    let serve = async {
        reply(&listener, &["before=10", "limit=2"], &[10]).await;
        reply(&listener, &["before=10", "limit=2"], &[8, 9]).await;
    };
    let consume = async {
        assert!(matches!(
            history.next_page().await,
            Err(MessageHistoryError::InvalidPage)
        ));
        assert_eq!(page_ids(&mut history).await, [9, 8]);
        assert!(history.next_page().await.unwrap().is_none());
    };
    exchange(serve, consume).await;
}

#[test]
fn invalid_pages_reject_duplicates_other_channels_and_overlong_non_around_data() {
    let deserialize =
        |id, channel| serde_json::from_value::<Message>(raw_message(id, channel)).unwrap();
    let good = || deserialize(10, CHANNEL.get());
    let another_channel = deserialize(11, 3);
    let latest = HistoryCursor::Latest;
    assert!(matches!(
        validate_page(&mut [good(), good()], CHANNEL, latest, 2),
        Err(MessageHistoryError::InvalidPage)
    ));
    assert!(matches!(
        validate_page(&mut [good(), another_channel], CHANNEL, latest, 2),
        Err(MessageHistoryError::InvalidPage)
    ));
    assert!(matches!(
        validate_page(
            &mut [good(), deserialize(11, CHANNEL.get())],
            CHANNEL,
            latest,
            1
        ),
        Err(MessageHistoryError::InvalidPage)
    ));
}
