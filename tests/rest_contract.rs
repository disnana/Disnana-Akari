//! 公開している Twilight クライアントを使い、REST 操作のガイドの前提を検証します。

#![allow(
    clippy::unwrap_used,
    reason = "ローカル HTTP のテスト用入力と応答を検証するため"
)]

use std::time::{Duration, Instant};

use disnana_akari::{
    Error, Result, ThreadMemberLookupError, fetch_thread_member,
    http::{Client, api_error::ApiError, error::ErrorType},
    model::{
        channel::Channel,
        id::{Id, marker::ChannelMarker},
    },
    parse_reaction_emoji,
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

async fn respond(
    listener: &TcpListener,
    request: &str,
    status: &str,
    extra_headers: &str,
    body: &str,
) -> Instant {
    let (stream, _) = listener.accept().await.unwrap();
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    stream.read_line(&mut line).await.unwrap();
    assert_eq!(line.trim_end(), request);
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
    let response = if status == "204 No Content" {
        format!("HTTP/1.1 {status}\r\nConnection: close\r\n\r\n")
    } else {
        format!(
            "HTTP/1.1 {status}\r\n{extra_headers}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len(),
        )
    };
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
            "GET /api/v10/channels/123 HTTP/1.1",
            "404 Not Found",
            "",
            r#"{"code":10003,"message":"Unknown Channel"}"#,
        )
        .await;
        let started = respond(
            &listener,
            "GET /api/v10/channels/123 HTTP/1.1",
            "429 Too Many Requests",
            "X-RateLimit-Scope: user\r\nX-RateLimit-Bucket: test-channel\r\nX-RateLimit-Limit: 1\r\nX-RateLimit-Remaining: 0\r\nX-RateLimit-Reset-After: 0.05\r\nRetry-After: 0.05\r\n",
            r#"{"message":"You are being rate limited.","retry_after":0.05,"global":false}"#,
        )
        .await;
        respond(
            &listener,
            "GET /api/v10/channels/123 HTTP/1.1",
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

#[tokio::test]
async fn consumes_only_through_the_discord_endpoint_and_deletes_a_test_grant() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::builder()
        .token("fixture-rest-token".to_owned())
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .build();

    let operations = async {
        client
            .consume_entitlement(Id::new(11), Id::new(12))
            .await
            .unwrap();
        let refused = client
            .consume_entitlement(Id::new(11), Id::new(13))
            .await
            .unwrap_err();
        assert!(matches!(
            refused.kind(),
            ErrorType::Response { status, error: ApiError::General(error), .. }
                if status.get() == 400 && error.code == 40018
        ));
        client
            .delete_test_entitlement(Id::new(11), Id::new(14))
            .await
            .unwrap();
    };
    let server = async {
        respond(
            &listener,
            "POST /api/v10/applications/11/entitlements/12/consume HTTP/1.1",
            "204 No Content",
            "",
            "",
        )
        .await;
        respond(
            &listener,
            "POST /api/v10/applications/11/entitlements/13/consume HTTP/1.1",
            "400 Bad Request",
            "",
            r#"{"code":40018,"message":"Only consumable SKUs can be consumed"}"#,
        )
        .await;
        respond(
            &listener,
            "DELETE /api/v10/applications/11/entitlements/14 HTTP/1.1",
            "204 No Content",
            "",
            "",
        )
        .await;
    };
    timeout(Duration::from_secs(5), async {
        tokio::join!(operations, server)
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn fetch_thread_member_preserves_membership_and_checks_identity() {
    const QUERY: &str = "GET /api/v10/channels/2/thread-members/8?with_member=true HTTP/1.1";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::builder()
        .token("fixture-rest-token".to_owned())
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .build();
    let requests = async {
        let (thread, user) = (Id::new(2), Id::new(8));
        let result = fetch_thread_member(&client, thread, user)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.user_id, Some(user));
        assert_eq!(result.member.unwrap().nick.as_deref(), Some("fixture"));
        assert!(matches!(
            fetch_thread_member(&client, thread, user).await,
            Err(ThreadMemberLookupError::MismatchedResponse)
        ));
        assert!(
            fetch_thread_member(&client, thread, user)
                .await
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            fetch_thread_member(&client, thread, user).await,
            Err(ThreadMemberLookupError::Http(error)) if matches!(
                error.kind(), ErrorType::Response { status, .. } if status.get() == 403
            )
        ));
    };
    let server = async {
        respond(
            &listener,
            QUERY,
            "200 OK",
            "",
            r#"{
            "id":"2","user_id":"8","join_timestamp":"2026-01-01T00:00:00+00:00","flags":0,
            "member":{"nick":"fixture","mute":false,"deaf":false,"flags":0,"roles":[],
                      "user":{"id":"8","username":"fixture","discriminator":"0"}}
        }"#,
        )
        .await;
        respond(
            &listener,
            QUERY,
            "200 OK",
            "",
            r#"{
            "id":"3","user_id":"8","join_timestamp":"2026-01-01T00:00:00+00:00","flags":0
        }"#,
        )
        .await;
        respond(
            &listener,
            QUERY,
            "404 Not Found",
            "",
            r#"{"code":10007,"message":"Unknown Member"}"#,
        )
        .await;
        respond(
            &listener,
            QUERY,
            "403 Forbidden",
            "",
            r#"{"code":50001,"message":"Missing Access"}"#,
        )
        .await;
    };
    timeout(Duration::from_secs(5), async {
        tokio::join!(requests, server)
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn reaction_routes_use_encoded_unicode_and_distinct_actor_targets() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::builder()
        .token("fixture-rest-token".to_owned())
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .build();
    let unicode = parse_reaction_emoji("❤️").unwrap();
    let custom = parse_reaction_emoji("<a:wave_1:42>").unwrap();
    let (channel, message) = (Id::new(2), Id::new(6));
    let operations = async {
        client
            .create_reaction(channel, message, &unicode)
            .await
            .unwrap();
        client
            .delete_current_user_reaction(channel, message, &unicode)
            .await
            .unwrap();
        client
            .delete_reaction(channel, message, &custom, Id::new(8))
            .await
            .unwrap();
        client
            .delete_all_reaction(channel, message, &custom)
            .await
            .unwrap();
    };
    let server = async {
        for request in [
            "PUT /api/v10/channels/2/messages/6/reactions/%E2%9D%A4%EF%B8%8F/@me HTTP/1.1",
            "DELETE /api/v10/channels/2/messages/6/reactions/%E2%9D%A4%EF%B8%8F/@me HTTP/1.1",
            "DELETE /api/v10/channels/2/messages/6/reactions/wave_1:42/8 HTTP/1.1",
            "DELETE /api/v10/channels/2/messages/6/reactions/wave_1:42 HTTP/1.1",
        ] {
            respond(&listener, request, "204 No Content", "", "").await;
        }
    };
    timeout(Duration::from_secs(5), async {
        tokio::join!(operations, server)
    })
    .await
    .unwrap();
}
