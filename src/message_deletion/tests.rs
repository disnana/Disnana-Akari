use std::time::Duration;

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
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

async fn reply(
    listener: &TcpListener,
    request_line: &str,
    audit_reason: Option<&str>,
    response: Option<&str>,
) -> Vec<u8> {
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = BufReader::new(socket);
    let mut line = String::new();
    socket.read_line(&mut line).await.unwrap();
    assert_eq!(line.trim_end(), request_line);
    let mut headers = String::new();
    loop {
        line.clear();
        assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
        if line == "\r\n" {
            break;
        }
        headers.push_str(&line);
    }
    let header = |name: &str| {
        headers.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
    };
    assert_eq!(header("x-audit-log-reason"), audit_reason);
    let length = header("content-length").map_or(0, |value| value.parse::<usize>().unwrap());
    let mut body = vec![0; length];
    socket.read_exact(&mut body).await.unwrap();
    let outbound = if let Some(response) = response {
        format!(
            "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
            response.len()
        )
    } else {
        "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_owned()
    };
    socket
        .get_mut()
        .write_all(outbound.as_bytes())
        .await
        .unwrap();
    body
}

fn snowflake(created_ms: u64, sequence: u64) -> Id<MessageMarker> {
    Id::new(((created_ms - DISCORD_EPOCH_MS) << 22) | sequence)
}

#[test]
fn cutoff_is_exclusive_at_millisecond_precision() {
    const NOW_MS: u64 = 1_789_000_000_000;
    let now = UNIX_EPOCH + Duration::from_millis(NOW_MS);
    let boundary = NOW_MS - BULK_MAX_AGE_MS as u64;
    let just_before = snowflake(boundary - 1, (1 << 22) - 1);
    let exactly = snowflake(boundary, 0);
    let just_after = snowflake(boundary + 1, 0);
    validate_bulk(&[exactly, just_after], now).unwrap();
    let maximum: Vec<_> = (0..BULK_MAX_MESSAGES as u64)
        .map(|sequence| snowflake(boundary, sequence))
        .collect();
    validate_bulk(&maximum, now).unwrap();
    assert!(matches!(
        validate_bulk(&[exactly, just_before], now),
        Err(MessageDeletionError::TooOld { message }) if message == just_before
    ));
    assert!(matches!(
        validate_bulk(&[exactly, just_after], UNIX_EPOCH - Duration::from_secs(1)),
        Err(MessageDeletionError::Clock(_))
    ));
}

#[tokio::test]
async fn single_uses_delete_and_bulk_uses_one_post_with_audit_reason() {
    let (listener, http) = local().await;
    let recent = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let ids = [snowflake(recent, 1), snowflake(recent, 2)];
    let server = tokio::spawn(async move {
        let single = reply(
            &listener,
            "DELETE /api/v10/channels/2/messages/4 HTTP/1.1",
            Some("one"),
            None,
        )
        .await;
        assert!(single.is_empty());
        let bulk = reply(
            &listener,
            "POST /api/v10/channels/2/messages/bulk-delete HTTP/1.1",
            Some("bulk"),
            None,
        )
        .await;
        assert_eq!(
            serde_json::from_slice::<Value>(&bulk).unwrap(),
            json!({ "messages": ids.map(|message| message.to_string()) })
        );
    });
    timeout(Duration::from_secs(5), async {
        // An old message stays eligible for the single-message endpoint.
        delete_messages(&http, Id::new(2), &[Id::new(4)], Some("one"))
            .await
            .unwrap();
        delete_messages(&http, Id::new(2), &ids, Some("bulk"))
            .await
            .unwrap();
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn rejects_invalid_bulk_without_connection_and_empty_is_noop() {
    let (listener, http) = local().await;
    drop(listener);
    let channel = Id::new(2);
    delete_messages(&http, channel, &[], None).await.unwrap();
    let oversized = [Id::new(1); BULK_MAX_MESSAGES + 1];
    assert!(matches!(
        delete_messages(&http, channel, &oversized, None).await,
        Err(MessageDeletionError::TooMany { count: 101 })
    ));
    assert!(matches!(
        delete_messages(&http, channel, &[Id::new(1); 2], None).await,
        Err(MessageDeletionError::Duplicate { message }) if message == Id::new(1)
    ));
    assert!(matches!(
        delete_messages(&http, channel, &[Id::new(1), Id::new(2)], None).await,
        Err(MessageDeletionError::TooOld { message }) if message == Id::new(1)
    ));
}

#[tokio::test]
async fn bulk_http_error_does_not_fall_back_to_individual_delete() {
    let (listener, http) = local().await;
    let recent = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let ids = [snowflake(recent, 1), snowflake(recent, 2)];
    let server = tokio::spawn(async move {
        reply(
            &listener,
            "POST /api/v10/channels/2/messages/bulk-delete HTTP/1.1",
            None,
            Some(r#"{"code":50013,"message":"Missing Permissions"}"#),
        )
        .await;
        reply(
            &listener,
            "DELETE /api/v10/channels/2/messages/4 HTTP/1.1",
            None,
            None,
        )
        .await;
    });
    timeout(Duration::from_secs(5), async {
        assert!(matches!(
            delete_messages(&http, Id::new(2), &ids, None).await,
            Err(MessageDeletionError::Http(_))
        ));
        delete_messages(&http, Id::new(2), &[Id::new(4)], None)
            .await
            .unwrap();
        server.await.unwrap();
    })
    .await
    .unwrap();
}
