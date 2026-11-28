use std::time::Duration;

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::timeout,
};

use super::*;

const TARGET: WebhookMessageRef =
    WebhookMessageRef::new(Id::new(12), Id::new(9)).in_thread(Id::new(32));
const TOKEN: &str = "webhook-test_token";

#[test]
fn edit_omits_unchanged_fields_and_can_explicitly_clear_values() {
    let request = WebhookMessageEdit::new()
        .content(None)
        .embeds(&[])
        .attachments(&[], &[], AttachmentLimits::default())
        .request(TARGET, TOKEN)
        .unwrap();
    let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
    assert_eq!(
        body,
        json!({"content":null,"embeds":[],"attachments":[],"allowed_mentions":{"parse":[]}})
    );
    let request = WebhookMessageEdit::new()
        .content(Some("更新"))
        .request(TARGET, TOKEN)
        .unwrap();
    let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
    assert_eq!(
        body,
        json!({"content":"更新","allowed_mentions":{"parse":[]}})
    );
    assert!(matches!(
        WebhookMessageEdit::new().request(TARGET, TOKEN),
        Err(WebhookSendError::InvalidPost(_))
    ));
}

#[test]
fn rejects_duplicate_or_excess_attachments() {
    let uploads = [Attachment::from_bytes("new.txt".into(), vec![1], 7)];
    for retained in [
        vec![Id::new(7)],
        vec![Id::new(8); 2],
        (10..20).map(Id::new).collect(),
    ] {
        assert!(matches!(
            WebhookMessageEdit::new()
                .attachments(&retained, &uploads, AttachmentLimits::default())
                .request(TARGET, TOKEN),
            Err(WebhookSendError::InvalidPost(_))
        ));
    }
}

fn message(id: u64) -> Value {
    json!({
        "id":id.to_string(),"channel_id":"32","type":0,"webhook_id":"12",
        "author":{"id":"12","username":"webhook","discriminator":"0","bot":true},
        "content":"更新","timestamp":"2026-01-01T00:00:00+00:00",
        "attachments":[],"embeds":[],"mention_everyone":false,"mention_roles":[],
        "mentions":[],"pinned":false,"tts":false
    })
}

async fn exchange(listener: &TcpListener, method: &str, status: &str, body: &str) -> String {
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = BufReader::new(socket);
    let mut line = String::new();
    socket.read_line(&mut line).await.unwrap();
    assert_eq!(
        line.trim(),
        format!("{method} /api/v10/webhooks/12/{TOKEN}/messages/9?thread_id=32 HTTP/1.1")
    );
    let mut length = 0;
    loop {
        line.clear();
        assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
        if line == "\r\n" {
            break;
        }
        let (name, value) = line.split_once(':').unwrap();
        assert!(!name.eq_ignore_ascii_case("authorization"));
        if name.eq_ignore_ascii_case("content-length") {
            length = value.trim().parse().unwrap();
        }
    }
    let mut request = vec![0; length];
    socket.read_exact(&mut request).await.unwrap();
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    socket
        .get_mut()
        .write_all(response.as_bytes())
        .await
        .unwrap();
    String::from_utf8(request).unwrap()
}

#[tokio::test]
async fn thread_lifecycle_validates_response_and_sends_retained_and_new_attachments() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = Client::builder()
        .token("unused-bot-token".into())
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .build();
    let files = [Attachment::from_bytes(
        "new.txt".into(),
        b"new data".to_vec(),
        0,
    )];
    let consume = async {
        assert_eq!(TARGET.fetch(&http, TOKEN).await.unwrap().id.get(), 9);
        let ids = [Id::new(9007199254740993)];
        let edit = WebhookMessageEdit::new()
            .content(Some("@everyone"))
            .attachments(&ids, &files, AttachmentLimits::default());
        assert_eq!(TARGET.edit(&http, TOKEN, &edit).await.unwrap().id.get(), 9);
        assert!(matches!(
            TARGET.fetch(&http, TOKEN).await,
            Err(WebhookSendError::InvalidResponse)
        ));
        assert!(matches!(
            TARGET.delete(&http, "wrong/path").await,
            Err(WebhookSendError::InvalidToken)
        ));
        TARGET.delete(&http, TOKEN).await.unwrap();
        let error = TARGET.delete(&http, TOKEN).await.unwrap_err();
        assert!(matches!(
            error,
            WebhookSendError::Rejected {
                status: 404,
                code: Some(10008)
            }
        ));
        assert!(!format!("{error:?} {error}").contains(TOKEN));
    };
    let serve = async {
        exchange(&listener, "GET", "200 OK", &message(9).to_string()).await;
        let body = exchange(&listener, "PATCH", "200 OK", &message(9).to_string()).await;
        assert!(body.contains("name=\"files[0]\"; filename=\"new.txt\""));
        assert!(body.contains("new data"));
        let payload = body
            .split_once("Content-Type: application/json\r\n\r\n")
            .unwrap()
            .1
            .lines()
            .next()
            .unwrap();
        let payload: Value = serde_json::from_str(payload).unwrap();
        assert_eq!(payload["content"], "@everyone");
        assert_eq!(payload["allowed_mentions"], json!({"parse":[]}));
        assert_eq!(
            payload["attachments"],
            json!([
                {"id":"9007199254740993"}, {"id":0,"filename":"new.txt"}
            ])
        );
        exchange(&listener, "GET", "200 OK", &message(10).to_string()).await;
        exchange(&listener, "DELETE", "204 No Content", "").await;
        exchange(
            &listener,
            "DELETE",
            "404 Not Found",
            &json!({"code":10008,"message":TOKEN}).to_string(),
        )
        .await;
    };
    timeout(Duration::from_secs(6), async {
        tokio::join!(serve, consume);
    })
    .await
    .unwrap();
}
