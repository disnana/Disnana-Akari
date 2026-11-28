use std::time::Duration;

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::{Instant, timeout},
};

use super::*;

async fn setup() -> (TcpListener, Client) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = Client::builder()
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .build();
    (listener, http)
}

async fn reply(listener: &TcpListener, accepted: bool) {
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = BufReader::new(socket);
    let mut line = String::new();
    socket.read_line(&mut line).await.unwrap();
    assert_eq!(line.trim_end(), "POST /api/v10/channels/2/typing HTTP/1.1");
    while !line.trim_end().is_empty() {
        line.clear();
        assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
    }
    let response = if accepted {
        "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n"
    } else {
        "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: 36\r\nConnection: close\r\n\r\n{\"code\":50013,\"message\":\"Forbidden\"}"
    };
    socket
        .get_mut()
        .write_all(response.as_bytes())
        .await
        .unwrap();
}

#[tokio::test]
async fn refreshes_until_work_completes_and_makes_no_extra_request() {
    let (listener, http) = setup().await;
    let typing = TypingIndicator::new(&http, Id::new(2));
    let (done, work) = tokio::sync::oneshot::channel();
    let serve = async {
        reply(&listener, true).await;
        let first = Instant::now();
        reply(&listener, true).await;
        assert!(first.elapsed() >= REFRESH_INTERVAL);
        done.send(42).unwrap();
    };
    let (_, result) = timeout(Duration::from_secs(30), async {
        tokio::join!(serve, typing.while_running(work))
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap(), 42);
    assert!(
        timeout(
            REFRESH_INTERVAL + Duration::from_millis(100),
            listener.accept()
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn a_failed_indicator_keeps_work_running() {
    let (listener, http) = setup().await;
    let typing = TypingIndicator::new(&http, Id::new(2));
    let (done, work) = tokio::sync::oneshot::channel();
    let serve = async {
        reply(&listener, false).await;
        assert!(
            timeout(
                REFRESH_INTERVAL + Duration::from_millis(100),
                listener.accept()
            )
            .await
            .is_err()
        );
        done.send("done").unwrap();
    };
    let (_, result) = timeout(Duration::from_secs(30), async {
        tokio::join!(serve, typing.while_running(work))
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap(), "done");
}
