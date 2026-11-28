use std::{future::Future, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::timeout,
};
use twilight_model::channel::message::MentionType;

use super::*;

const SOURCE_CHANNEL: Id<ChannelMarker> = Id::new(2);
const SOURCE_MESSAGE: Id<MessageMarker> = Id::new(3);
const DESTINATION: Id<ChannelMarker> = Id::new(4);

async fn local() -> (TcpListener, Client) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = Client::builder()
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .token("local-test".into())
        .default_allowed_mentions(AllowedMentions {
            parse: vec![MentionType::Everyone],
            ..AllowedMentions::default()
        })
        .build();
    (listener, http)
}

fn raw_forward() -> Value {
    json!({
        "id":"5", "channel_id":"4", "type":0,
        "author":{"id":"6", "username":"bot", "discriminator":"0"},
        "content":"", "timestamp":"2026-01-01T00:00:00+00:00",
        "attachments":[], "embeds":[], "mention_everyone":false, "mention_roles":[],
        "mentions":[], "pinned":false, "tts":false,
        "message_reference":{"type":1, "channel_id":"2", "message_id":"3"},
        "message_snapshots":[{"message":{
            "type":0, "content":"snapshot", "attachments":[], "embeds":[],
            "timestamp":"2026-01-01T00:00:00+00:00"
        }}]
    })
}

async fn reply(listener: &TcpListener, status: &str, body: Value) {
    let (stream, _) = listener.accept().await.unwrap();
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    stream.read_line(&mut line).await.unwrap();
    assert_eq!(
        line.trim_end(),
        "POST /api/v10/channels/4/messages HTTP/1.1"
    );

    let mut authenticated = false;
    let mut length = None;
    loop {
        line.clear();
        assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
        let Some((name, value)) = line.trim_end().split_once(':') else {
            assert_eq!(line, "\r\n");
            break;
        };
        if name.eq_ignore_ascii_case("authorization") {
            assert_eq!(value.trim(), "Bot local-test");
            authenticated = true;
        } else if name.eq_ignore_ascii_case("content-length") {
            length = Some(value.trim().parse().unwrap());
        }
    }
    assert!(authenticated);
    let mut request = vec![0; length.unwrap()];
    stream.read_exact(&mut request).await.unwrap();
    let request: Value = serde_json::from_slice(&request).unwrap();
    assert_eq!(
        request,
        json!({
            "allowed_mentions":{"parse":[]},
            "message_reference":{"type":1, "channel_id":"2", "message_id":"3", "fail_if_not_exists":true}
        })
    );

    let body = body.to_string();
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
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

#[tokio::test]
async fn forwards_by_reference_with_no_mentions_and_keeps_the_snapshot() {
    let (listener, http) = local().await;
    exchange(reply(&listener, "200 OK", raw_forward()), async {
        let forwarded = forward_message(&http, SOURCE_CHANNEL, SOURCE_MESSAGE, DESTINATION)
            .await
            .unwrap();
        assert_eq!(forwarded.id, Id::new(5));
        assert_eq!(forwarded.channel_id, DESTINATION);
        assert_eq!(forwarded.message_snapshots[0].message.content, "snapshot");
    })
    .await;
}

#[tokio::test]
async fn rejects_unrelated_or_malformed_responses_and_surfaces_rest_failures() {
    let (listener, http) = local().await;
    let serve = async {
        for (key, mismatch) in [
            ("channel_id", json!("9")),
            ("id", json!("3")),
            ("message_reference", Value::Null),
            (
                "message_reference",
                json!({"type":0, "channel_id":"2", "message_id":"3"}),
            ),
            (
                "message_reference",
                json!({"type":1, "channel_id":"2", "message_id":"7"}),
            ),
        ] {
            let mut response = raw_forward();
            response[key] = mismatch;
            reply(&listener, "200 OK", response).await;
        }
        reply(&listener, "200 OK", json!({})).await;
        reply(
            &listener,
            "403 Forbidden",
            json!({"code":160014, "message":"Cannot forward unreadable content"}),
        )
        .await;
    };
    let consume = async {
        let send = || forward_message(&http, SOURCE_CHANNEL, SOURCE_MESSAGE, DESTINATION);
        for _ in 0..5 {
            assert!(matches!(
                send().await,
                Err(ForwardMessageError::InvalidResponse)
            ));
        }
        assert!(matches!(
            send().await,
            Err(ForwardMessageError::HttpBody(_))
        ));
        assert!(matches!(send().await, Err(ForwardMessageError::Http(_))));
    };
    exchange(serve, consume).await;
}
