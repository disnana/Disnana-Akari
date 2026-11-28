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

fn post() -> Value {
    json!({
        "id": "9",
        "type": 11,
        "parent_id": "2",
        "message": {
            "id": "9", "channel_id": "9", "type": 0,
            "author": { "id": "3", "username": "bot", "discriminator": "0" },
            "content": "Hello", "timestamp": "2026-01-01T00:00:00.000000+00:00",
            "attachments": [], "embeds": [], "mention_everyone": false,
            "mention_roles": [], "mentions": [], "pinned": false, "tts": false
        }
    })
}

async fn reply(listener: &TcpListener, response: Value) -> (String, String, Vec<u8>) {
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = BufReader::new(socket);
    let mut line = String::new();
    socket.read_line(&mut line).await.unwrap();
    assert_eq!(line.trim_end(), "POST /api/v10/channels/2/threads HTTP/1.1");
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
    let content_type = header("content-type").unwrap().to_owned();
    let length = header("content-length").unwrap().parse().unwrap();
    let audit_reason = header("x-audit-log-reason").unwrap_or_default().to_owned();
    let mut body = vec![0; length];
    socket.read_exact(&mut body).await.unwrap();
    let response = response.to_string();
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.len()
    );
    socket
        .get_mut()
        .write_all(headers.as_bytes())
        .await
        .unwrap();
    socket
        .get_mut()
        .write_all(response.as_bytes())
        .await
        .unwrap();
    (content_type, audit_reason, body)
}

#[tokio::test]
async fn creates_thread_and_starter_in_one_json_request() {
    let (listener, http) = local().await;
    let server = tokio::spawn(async move {
        let (content_type, reason, body) = reply(&listener, post()).await;
        assert_eq!(content_type, "application/json");
        assert_eq!(reason, "reason");
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({
                "name": "案内",
                "auto_archive_duration": 1440,
                "rate_limit_per_user": 0,
                "applied_tags": ["4", "5"],
                "message": {"content": "Hello @everyone", "allowed_mentions": {"parse": []}}
            }),
        );
    });
    timeout(Duration::from_secs(5), async {
        let created = ForumPost::new("案内")
            .content("Hello @everyone")
            .auto_archive_duration(AutoArchiveDuration::Day)
            .slowmode(0)
            .applied_tags(&[Id::new(4), Id::new(5)])
            .reason("reason")
            .send(&http, Id::new(2))
            .await
            .unwrap();
        assert_eq!(created.channel.id, Id::new(9));
        assert_eq!(created.message.id, Id::new(9));
        assert_eq!(created.message.content, "Hello");
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn multipart_keeps_file_metadata_inside_the_starter() {
    let (listener, http) = local().await;
    let server = tokio::spawn(async move {
        let (content_type, _, body) = reply(&listener, post()).await;
        let boundary = content_type
            .strip_prefix("multipart/form-data; boundary=")
            .unwrap();
        let body = std::str::from_utf8(&body).unwrap();
        assert!(body.starts_with(&format!("--{boundary}\r\n")));
        assert!(body.ends_with(&format!("--{boundary}--")));
        let (_, parts) = body.split_once("\r\n\r\n").unwrap();
        let (payload, file) = parts.split_once(&format!("\r\n--{boundary}\r\n")).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(payload).unwrap(),
            json!({
                "name": "Media", "rate_limit_per_user": 21600,
                "message": {
                    "allowed_mentions": {"parse": []},
                    "attachments": [{"id": 0, "filename": "photo.png", "description": "alt text"}]
                }
            }),
        );
        assert!(
            file.contains("name=\"files[0]\"; filename=\"photo.png\"\r\n\r\nfixture-media\r\n")
        );
    });
    timeout(Duration::from_secs(5), async {
        let mut attachment =
            Attachment::from_bytes("photo.png".to_owned(), b"fixture-media".to_vec(), 0);
        attachment.description("alt text".to_owned());
        ForumPost::new("Media")
            .attachments(&[attachment])
            .slowmode(21600)
            .send(&http, Id::new(2))
            .await
            .unwrap();
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn rejects_mismatched_thread_response_without_retrying() {
    let (listener, http) = local().await;
    let server = tokio::spawn(async move {
        for field in ["type", "parent_id", "message_id", "message_channel_id"] {
            let mut response = post();
            match field {
                "type" => response["type"] = json!(12),
                "parent_id" => response["parent_id"] = json!("5"),
                "message_id" => response["message"]["id"] = json!("5"),
                _ => response["message"]["channel_id"] = json!("5"),
            }
            reply(&listener, response).await;
        }
    });
    timeout(Duration::from_secs(5), async {
        for _ in 0..4 {
            assert!(matches!(
                ForumPost::new("Post")
                    .content("Hello")
                    .send(&http, Id::new(2))
                    .await,
                Err(Error::InvalidUi(
                    "Discord の投稿作成応答が要求と一致しません。"
                ))
            ));
        }
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn validation_rejects_invalid_values_and_encoded_size_before_http() {
    let (listener, http) = local().await;
    drop(listener);
    let parent = Id::new(2);
    let long_title = "日".repeat(101);
    let long_body = "日".repeat(2001);
    let duplicate = [Id::new(3), Id::new(3)];
    let too_many_tags = [
        Id::new(1),
        Id::new(2),
        Id::new(3),
        Id::new(4),
        Id::new(5),
        Id::new(6),
    ];
    for invalid in [
        ForumPost::new(" ").content("Hello"),
        ForumPost::new(&long_title).content("Hello"),
        ForumPost::new("Post").content(" "),
        ForumPost::new("Post").content(&long_body),
        ForumPost::new("Post")
            .content("Hello")
            .applied_tags(&duplicate),
        ForumPost::new("Post")
            .content("Hello")
            .applied_tags(&too_many_tags),
        ForumPost::new("Post").content("Hello").slowmode(21601),
        ForumPost::new("Post")
            .content("Hello")
            .auto_archive_duration(AutoArchiveDuration::Unknown { value: 1 }),
    ] {
        assert!(matches!(
            invalid.send(&http, parent).await,
            Err(Error::InvalidUi(_) | Error::MessageValidation(_) | Error::ChannelValidation(_))
        ));
    }
    let attachment = Attachment::from_bytes(
        "near-limit.png".to_owned(),
        vec![0; validation::CREATE_MESSAGE_REQUEST_MAX_BYTES as usize - 1],
        0,
    );
    assert!(matches!(
        ForumPost::new("Post")
            .attachments(&[attachment])
            .send(&http, parent)
            .await,
        Err(Error::InvalidUi(
            "投稿作成リクエストが 25 MiB を超えています。"
        ))
    ));
}
