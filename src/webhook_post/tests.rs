use std::time::Duration;

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::timeout,
};

use super::*;
use crate::PollAnswerInput;

const TOKEN: &str = "webhook-secret_123";
const WEBHOOK: Id<WebhookMarker> = Id::new(12);

#[test]
fn validates_path_inputs_and_post_before_building_a_request() {
    let post = || WebhookPost::new().content("更新");
    for token in ["", "wrong/path", "key?query", "%2f", "key\n", "空白"] {
        assert!(matches!(
            post().request(WEBHOOK, token),
            Err(WebhookSendError::InvalidToken)
        ));
    }
    assert!(matches!(
        WebhookPost::new().request(WEBHOOK, TOKEN),
        Err(WebhookSendError::InvalidPost(_))
    ));
    assert!(matches!(
        post()
            .new_thread("更新", &[Id::new(1); 2])
            .request(WEBHOOK, TOKEN),
        Err(WebhookSendError::InvalidPost(_))
    ));
    assert!(matches!(
        post().new_thread("　", &[]).request(WEBHOOK, TOKEN),
        Err(WebhookSendError::InvalidPost(_))
    ));
    assert!(matches!(
        post().content(&"猫".repeat(2_001)).request(WEBHOOK, TOKEN),
        Err(WebhookSendError::InvalidPost(_))
    ));
    let files = [Attachment::from_bytes("a\"b.txt".into(), vec![0], 0)];
    assert!(matches!(
        post().attachments(&files).request(WEBHOOK, TOKEN),
        Err(WebhookSendError::InvalidPost(_))
    ));
    let files = [Attachment::from_bytes("a.txt".into(), vec![0; 2], 0)];
    let limits = AttachmentLimits::default().max_file_bytes(Some(1));
    assert!(matches!(
        post()
            .attachments(&files)
            .attachment_limits(limits)
            .request(WEBHOOK, TOKEN),
        Err(WebhookSendError::InvalidPost(_))
    ));
}

async fn exchange(listener: &TcpListener, status: &str, response: Value) -> (String, String) {
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = BufReader::new(socket);
    let mut line = String::new();
    socket.read_line(&mut line).await.unwrap();
    let route = line.split_whitespace().nth(1).unwrap().to_owned();
    assert!(line.starts_with("POST /api/v10/webhooks/12/"));
    assert!(route.contains(TOKEN));
    assert!(route.contains("wait=true"));
    let mut length = None;
    loop {
        line.clear();
        assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
        if line == "\r\n" {
            break;
        }
        let (name, value) = line.split_once(':').unwrap();
        assert!(!name.eq_ignore_ascii_case("authorization"));
        if name.eq_ignore_ascii_case("content-length") {
            length = Some(value.trim().parse().unwrap());
        }
    }
    let mut body = vec![0; length.unwrap()];
    socket.read_exact(&mut body).await.unwrap();
    let response = response.to_string();
    let sent = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
        response.len()
    );
    socket.get_mut().write_all(sent.as_bytes()).await.unwrap();
    (route, String::from_utf8(body).unwrap())
}

fn response(channel: &str) -> Value {
    json!({
        "id":"9","channel_id":channel,"type":0,"webhook_id":"12",
        "author":{"id":"12","username":"webhook","discriminator":"0","bot":true},
        "content":"更新","timestamp":"2026-01-01T00:00:00+00:00",
        "attachments":[],"embeds":[],"mention_everyone":false,"mention_roles":[],
        "mentions":[],"pinned":false,"tts":false
    })
}

struct RequestSpans(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl tracing::Subscriber for RequestSpans {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, attrs: &tracing::span::Attributes<'_>) -> tracing::Id {
        let metadata = attrs.metadata();
        if metadata.name() == "webhook_test_probe"
            || (metadata.name() == "request" && metadata.target().starts_with("twilight_http"))
        {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        tracing::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::Id, _: &tracing::Id) {}
    fn event(&self, _: &tracing::Event<'_>) {}
    fn enter(&self, _: &tracing::Id) {}
    fn exit(&self, _: &tracing::Id) {}
}

#[tokio::test]
async fn sends_thread_posts_and_redacts_discord_errors() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = Client::builder()
        .token("bot-secret".into())
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .build();
    let post = WebhookPost::new().content("更新").in_thread(Id::new(32));
    let files = [Attachment::from_bytes(
        "report.txt".into(),
        b"report data".to_vec(),
        0,
    )];
    let tags = [Id::new(33)];
    let answers = [PollAnswerInput::new("はい"), PollAnswerInput::new("いいえ")];
    let draft = PollDraft::new("進めますか？", &answers).unwrap();
    let new_thread = WebhookPost::new()
        .content("@everyone")
        .in_thread(Id::new(32))
        .new_thread("進捗", &tags)
        .attachments(&files)
        .poll(draft);
    let span_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let consume = async {
        let _probe = tracing::info_span!("webhook_test_probe");
        assert_eq!(
            post.send(&http, WEBHOOK, TOKEN)
                .await
                .unwrap()
                .channel_id
                .get(),
            32
        );
        assert_eq!(
            new_thread
                .send(&http, WEBHOOK, TOKEN)
                .await
                .unwrap()
                .channel_id
                .get(),
            45
        );
        assert!(matches!(
            post.send(&http, WEBHOOK, TOKEN).await,
            Err(WebhookSendError::InvalidResponse)
        ));
        let error = post.send(&http, WEBHOOK, TOKEN).await.unwrap_err();
        assert!(matches!(
            error,
            WebhookSendError::Rejected {
                status: 403,
                code: Some(50013)
            }
        ));
        assert!(!format!("{error:?} {error}").contains(TOKEN));
    }
    .with_subscriber(RequestSpans(span_count.clone()));
    let serve = async {
        let (route, body) = exchange(&listener, "200 OK", response("32")).await;
        assert!(route.contains("thread_id=32"));
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap(),
            json!({"content":"更新","allowed_mentions":{"parse":[]}})
        );

        let mut poll_response = response("45");
        poll_response["poll"] = json!({
            "question":{"text":"進めますか？"},"answers":[],"expiry":null,
            "allow_multiselect":false,"layout_type":1
        });
        let (route, body) = exchange(&listener, "200 OK", poll_response).await;
        assert!(!route.contains("thread_id"));
        assert!(body.contains("name=\"files[0]\"; filename=\"report.txt\""));
        assert!(body.contains("report data"));
        let payload = body
            .split_once("Content-Type: application/json\r\n\r\n")
            .unwrap()
            .1
            .lines()
            .next()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(payload).unwrap(),
            json!({
                "content":"@everyone","allowed_mentions":{"parse":[]},
                "thread_name":"進捗","applied_tags":["33"],
                "attachments":[{"id":0,"filename":"report.txt"}],
                "poll":{
                    "question":{"text":"進めますか？"},
                    "answers":[{"poll_media":{"text":"はい"}},{"poll_media":{"text":"いいえ"}}],
                    "allow_multiselect":false,"layout_type":1,"duration":24
                }
            })
        );
        exchange(&listener, "200 OK", response("42")).await;
        exchange(
            &listener,
            "403 Forbidden",
            json!({"code":50013,"message":TOKEN}),
        )
        .await;
    };
    timeout(Duration::from_secs(6), async {
        tokio::join!(serve, consume);
    })
    .await
    .unwrap();
    assert_eq!(span_count.load(std::sync::atomic::Ordering::Relaxed), 1);
}
