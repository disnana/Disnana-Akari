use std::{future::Future, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::timeout,
};

use super::*;

const GUILD: Id<GuildMarker> = Id::new(2);

async fn local() -> (TcpListener, Client) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = Client::builder()
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .build();
    (listener, http)
}

fn raw_ban(id: u64) -> Value {
    json!({
        "reason": "fixture",
        "user": {"id": id.to_string(), "username": "member", "discriminator": "0"}
    })
}

async fn reply(listener: &TcpListener, expected_query: &[&str], ids: &[u64]) {
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = BufReader::new(socket);
    let mut line = String::new();
    socket.read_line(&mut line).await.unwrap();
    let request = line
        .trim_end()
        .strip_prefix("GET /api/v10/guilds/2/bans?")
        .and_then(|path| path.strip_suffix(" HTTP/1.1"))
        .unwrap();
    let mut actual: Vec<_> = request.split('&').collect();
    actual.sort_unstable();
    assert_eq!(actual, expected_query);
    while !line.trim_end().is_empty() {
        line.clear();
        assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
    }
    let body = ids.iter().copied().map(raw_ban).collect::<Vec<_>>();
    let body = serde_json::to_string(&body).unwrap();
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

async fn ids(pages: &mut BanPages<'_>) -> Vec<u64> {
    pages
        .next_page()
        .await
        .unwrap()
        .unwrap()
        .into_iter()
        .map(|ban| ban.user.id.get())
        .collect()
}

#[tokio::test]
async fn default_scan_is_lazy_clamps_page_size_and_stops_on_a_short_page() {
    let (listener, http) = local().await;
    let mut zero = BanPages::new(&http, GUILD).limit(0);
    assert!(zero.next_page().await.unwrap().is_none());
    let mut pages = BanPages::new(&http, GUILD).page_size(NonZeroU16::new(2_000).unwrap());
    assert!(
        timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );

    exchange(reply(&listener, &["limit=1000"], &[10, 5]), async {
        assert_eq!(ids(&mut pages).await, [5, 10]);
        assert!(pages.next_page().await.unwrap().is_none());
    })
    .await;
}

#[tokio::test]
async fn before_advances_by_the_lowest_id_but_preserves_ascending_pages() {
    let (listener, http) = local().await;
    let mut pages = BanPages::new(&http, GUILD)
        .after(Id::new(5))
        .before(Id::new(20))
        .page_size(NonZeroU16::new(2).unwrap())
        .limit(3);

    let serve = async {
        reply(&listener, &["before=20", "limit=2"], &[17, 19]).await;
        reply(&listener, &["before=17", "limit=1"], &[16]).await;
    };
    let consume = async {
        assert_eq!(ids(&mut pages).await, [17, 19]);
        assert_eq!(ids(&mut pages).await, [16]);
        assert!(pages.next_page().await.unwrap().is_none());
    };
    exchange(serve, consume).await;
}

#[tokio::test]
async fn after_retries_a_nonprogressing_page_without_changing_the_request() {
    let (listener, http) = local().await;
    let mut pages = BanPages::new(&http, GUILD)
        .before(Id::new(90))
        .after(Id::new(10))
        .page_size(NonZeroU16::new(2).unwrap());

    let serve = async {
        reply(&listener, &["after=10", "limit=2"], &[10, 11]).await;
        reply(&listener, &["after=10", "limit=2"], &[12, 11]).await;
        reply(&listener, &["after=12", "limit=2"], &[]).await;
    };
    let consume = async {
        assert!(matches!(
            pages.next_page().await,
            Err(BanPagesError::InvalidPage)
        ));
        assert_eq!(ids(&mut pages).await, [11, 12]);
        assert!(pages.next_page().await.unwrap().is_none());
    };
    exchange(serve, consume).await;
}

#[test]
fn malformed_pages_reject_duplicates_oversize_and_invalid_backward_progress() {
    let ban = |id| serde_json::from_value::<Ban>(raw_ban(id)).unwrap();
    let after = Cursor::After(None);
    assert!(matches!(
        after.advance(&mut [ban(2), ban(2)], 2),
        Err(BanPagesError::InvalidPage)
    ));
    assert!(matches!(
        after.advance(&mut [ban(2), ban(3)], 1),
        Err(BanPagesError::InvalidPage)
    ));
    let before = Cursor::Before(Id::new(10));
    assert!(matches!(
        before.advance(&mut [ban(9), ban(10)], 2),
        Err(BanPagesError::InvalidPage)
    ));
}
