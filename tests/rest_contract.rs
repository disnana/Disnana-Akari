//! 公開している Twilight クライアントを使い、REST 操作のガイドの前提を検証します。

#![allow(
    clippy::unwrap_used,
    reason = "ローカル HTTP のテスト用入力と応答を検証するため"
)]

use std::time::{Duration, Instant};

use disnana_akari::{
    Error, Result,
    http::{Client, error::ErrorType},
    model::{
        channel::Channel,
        id::{Id, marker::ChannelMarker},
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::timeout,
};

async fn channel_if_present(http: &Client, id: Id<ChannelMarker>) -> Result<Option<Channel>> {
    match http.channel(id).await {
        Ok(response) => Ok(Some(response.model().await?)),
        Err(error) => match error.kind() {
            ErrorType::Response { status, .. } if status.get() == 404 => Ok(None),
            _ => Err(error.into()),
        },
    }
}

async fn respond(listener: &TcpListener, status: &str, extra_headers: &str, body: &str) -> Instant {
    let (stream, _) = listener.accept().await.unwrap();
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    stream.read_line(&mut line).await.unwrap();
    assert!(line.starts_with("GET /api/v10/channels/123 "));
    let mut token_present = false;
    loop {
        line.clear();
        assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("authorization")
        {
            token_present = value.trim() == "Bot fixture-rest-token";
        }
    }
    assert!(token_present);
    let response = format!(
        "HTTP/1.1 {status}\r\n{extra_headers}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    );
    let sent = Instant::now();
    stream
        .get_mut()
        .write_all(response.as_bytes())
        .await
        .unwrap();
    sent
}

#[tokio::test]
async fn shared_rest_client_retries_429_and_keeps_403_distinct_from_404() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::builder()
        .token("fixture-rest-token".to_owned())
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .build();
    let requests = async {
        assert!(
            channel_if_present(&client, Id::new(123))
                .await
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            channel_if_present(&client, Id::new(123)).await,
            Err(Error::Http(error)) if matches!(
                error.kind(), ErrorType::Response { status, .. } if status.get() == 403
            )
        ));
    };
    let server = async {
        respond(
            &listener,
            "404 Not Found",
            "",
            r#"{"code":10003,"message":"Unknown Channel"}"#,
        )
        .await;
        let started = respond(
            &listener,
            "429 Too Many Requests",
            "X-RateLimit-Scope: user\r\nX-RateLimit-Bucket: test-channel\r\nX-RateLimit-Limit: 1\r\nX-RateLimit-Remaining: 0\r\nX-RateLimit-Reset-After: 0.05\r\nRetry-After: 0.05\r\n",
            r#"{"message":"You are being rate limited.","retry_after":0.05,"global":false}"#,
        )
        .await;
        respond(
            &listener,
            "403 Forbidden",
            "",
            r#"{"code":50001,"message":"Missing Access"}"#,
        )
        .await;
        assert!(started.elapsed() >= Duration::from_millis(50));
    };
    timeout(Duration::from_secs(5), async {
        tokio::join!(requests, server);
    })
    .await
    .unwrap();
}
