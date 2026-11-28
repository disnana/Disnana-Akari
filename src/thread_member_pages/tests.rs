use std::{future::Future, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::timeout,
};

use super::*;

const THREAD: Id<ChannelMarker> = Id::new(2);

async fn local() -> (TcpListener, Client) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = Client::builder()
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .build();
    (listener, http)
}

fn raw(user: u64) -> Value {
    json!({"id": "2", "user_id": user.to_string(), "join_timestamp": "2024-04-15T12:00:00+00:00", "flags": 0})
}

fn full(user: u64, guild_user: u64) -> Value {
    let mut raw = raw(user);
    raw["member"] = json!({
        "deaf": false, "mute": false, "flags": 0, "roles": [],
        "user": {"id": guild_user.to_string(), "username": "member", "discriminator": "0"}
    });
    raw
}

async fn reply(listener: &TcpListener, query: &str, includes: Vec<Value>) {
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = BufReader::new(socket);
    let mut line = String::new();
    socket.read_line(&mut line).await.unwrap();
    assert_eq!(
        line.trim_end(),
        format!("GET /api/v10/channels/2/thread-members?{query} HTTP/1.1")
    );
    while !line.trim_end().is_empty() {
        line.clear();
        assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
    }
    let body = serde_json::to_string(&includes).unwrap();
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

fn ids(page: &[ThreadMember]) -> Vec<u64> {
    page.iter()
        .map(|member| member.user_id.unwrap().get())
        .collect()
}

#[tokio::test]
async fn pagination_is_lazy_capped_and_accepts_missing_guild_member_data() {
    let (listener, http) = local().await;
    let mut empty = ThreadMemberPages::new(&http, THREAD).limit(0);
    assert!(empty.next_page().await.unwrap().is_none());
    let mut pages = ThreadMemberPages::new(&http, THREAD).page_size(NonZeroU16::MAX);
    assert!(
        timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
    let serve = async {
        reply(
            &listener,
            "limit=100&with_member=true",
            vec![full(12, 12), raw(11)],
        )
        .await;
        reply(&listener, "after=12&limit=100&with_member=true", Vec::new()).await;
    };
    exchange(serve, async {
        let page = pages.next_page().await.unwrap().unwrap();
        assert_eq!(ids(&page), [11, 12]);
        assert!(page[0].member.is_none());
        assert_eq!(page[1].member.as_ref().unwrap().user.id, Id::new(12));
        assert!(pages.next_page().await.unwrap().is_none());
        assert!(pages.next_page().await.unwrap().is_none());
    })
    .await;
}

#[tokio::test]
async fn invalid_pages_do_not_consume_total_budget_or_advance_after() {
    let (listener, http) = local().await;
    let mut pages = ThreadMemberPages::new(&http, THREAD)
        .after(Id::new(10))
        .page_size(NonZeroU16::new(2).unwrap())
        .limit(3);
    let serve = async {
        let query = "after=10&limit=2&with_member=true";
        let mut no_user = raw(11);
        no_user["user_id"] = Value::Null;
        let mut other_thread = raw(11);
        other_thread["id"] = json!("9");
        for invalid in [
            vec![no_user],
            vec![other_thread],
            vec![full(11, 12)],
            vec![raw(10)],
            vec![raw(11), raw(11)],
            vec![raw(11), raw(12), raw(13)],
        ] {
            reply(&listener, query, invalid).await;
        }
        reply(&listener, query, vec![json!({"id":"2", "user_id":"11"})]).await;
        reply(&listener, query, vec![raw(12), raw(11)]).await;
        reply(
            &listener,
            "after=12&limit=1&with_member=true",
            vec![raw(13)],
        )
        .await;
    };
    exchange(serve, async {
        for _ in 0..6 {
            assert!(matches!(
                pages.next_page().await,
                Err(ThreadMemberPagesError::InvalidPage)
            ));
        }
        assert!(matches!(
            pages.next_page().await,
            Err(ThreadMemberPagesError::HttpBody(_))
        ));
        assert_eq!(ids(&pages.next_page().await.unwrap().unwrap()), [11, 12]);
        assert_eq!(ids(&pages.next_page().await.unwrap().unwrap()), [13]);
        assert!(pages.next_page().await.unwrap().is_none());
    })
    .await;
}
