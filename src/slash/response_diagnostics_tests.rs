use std::{
    io,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};
use twilight_model::http::attachment::Attachment;

use super::{file_response::FileCallback, tests::command_interaction, *};
use crate::timing::Pending;

async fn reject_callback(listener: &TcpListener) -> io::Result<()> {
    let (stream, _) = listener.accept().await?;
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    stream.read_line(&mut line).await?;
    assert!(line.starts_with("POST /api/v10/interactions/"));
    let mut bytes = 0;
    loop {
        line.clear();
        if stream.read_line(&mut line).await? == 0 {
            return Err(io::Error::other("HTTP ヘッダーが途切れました"));
        }
        if line == "\r\n" {
            break;
        }
        if let Some((header, value)) = line.split_once(':')
            && header.eq_ignore_ascii_case("content-length")
        {
            bytes = value.trim().parse().map_err(io::Error::other)?;
        }
    }
    if bytes > 4096 {
        return Err(io::Error::other("テストのリクエストが上限を超えています"));
    }
    stream.read_exact(&mut vec![0; bytes]).await?;
    let body = r#"{"code":10062,"message":"private-response-fixture"}"#;
    let response = format!(
        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.get_mut().write_all(response.as_bytes()).await
}

#[tokio::test]
async fn initial_callback_failures_report_matching_ids_numbers_and_timing() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(Error::handler)?;
    let client = Client::builder()
        .proxy(
            listener.local_addr().map_err(Error::handler)?.to_string(),
            true,
        )
        .build();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&seen);
    let info = InfoReporter::new(move |event| {
        if let Ok(mut events) = captured.lock() {
            events.push(*event);
        }
    });
    let mut interaction = command_interaction(None, None, 25);
    interaction.token = "private-token-fixture".into();
    let (_, timing) = Pending::new((), None).begin();
    let initial = InitialResponder {
        http: &client,
        interaction: &interaction,
        timing,
        info: &info,
    };

    let default_reply = async {
        let result = initial
            .send(&initial_response(InitialResponse::Deferred, true))
            .await;
        assert!(matches!(result, Err(Error::Http(_))));
        let files = [Attachment::from_bytes("note.txt".into(), vec![1], 0)];
        let result = initial
            .send_file_callback(
                FileCallback::Classic {
                    content: "",
                    ephemeral: true,
                },
                &files,
                Vec::new(),
            )
            .await;
        assert!(matches!(result, Err(Error::Http(_))));
    };
    let mock = async {
        reject_callback(&listener).await?;
        reject_callback(&listener).await
    };
    let ((), result) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(default_reply, mock)
    })
    .await
    .map_err(Error::handler)?;
    result.map_err(Error::handler)?;

    let events = seen
        .lock()
        .map_err(|_| Error::msg("診断イベントを取得できませんでした"))?;
    assert_eq!(events.len(), 2);
    for (event, expected) in events.iter().zip([
        InteractionResponseType::DeferredChannelMessageWithSource,
        InteractionResponseType::ChannelMessageWithSource,
    ]) {
        let InfoEvent::InteractionResponseFailed {
            interaction_id,
            command_id,
            response_type,
            elapsed,
            queue_wait,
            rest_elapsed,
            http_status,
            discord_code,
            ..
        } = event
        else {
            return Err(Error::msg("初回応答の失敗が通知されませんでした"));
        };
        assert_eq!(*interaction_id, interaction.id);
        assert_eq!(*command_id, Some(Id::new(25)));
        assert_eq!(*response_type, expected);
        assert_eq!((*http_status, *discord_code), (Some(404), Some(10062)));
        assert!(*elapsed >= *queue_wait + *rest_elapsed);
        assert!(!format!("{event:?}").contains("private-"));
    }
    Ok(())
}
