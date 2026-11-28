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

pub(crate) async fn reject_callback(
    listener: &TcpListener,
    status: &str,
    code: u64,
) -> io::Result<()> {
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
    let body = format!(r#"{{"code":{code},"message":"private-response-fixture"}}"#);
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
        permissions: &crate::permissions::PermissionResolver::default(),
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
        reject_callback(&listener, "404 Not Found", 10062).await?;
        reject_callback(&listener, "404 Not Found", 10062).await
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

#[tokio::test]
async fn dispatch_does_not_repeat_an_expired_or_acknowledged_callback() -> Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::hooks::Reporter;

    let mut commands = tests::active(Scope::Global);
    commands.handlers.get_mut(&Id::new(5)).unwrap().root.command =
        Some(Box::new(async |ctx: &mut SlashContext<'_>| -> Result<()> {
            ctx.defer_ephemeral().await
        }));
    let interaction = command_interaction(None, None, 5);
    for (status, code) in [("404 Not Found", 10062), ("400 Bad Request", 40060)] {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(Error::handler)?;
        let client = Client::builder()
            .proxy(
                listener.local_addr().map_err(Error::handler)?.to_string(),
                true,
            )
            .build();
        let reports = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&reports);
        let hooks = Hooks {
            once_checks: Vec::new(),
            reporter: Reporter::new(move |report| {
                assert_eq!(report.source, ErrorSource::SlashCommand);
                observed.fetch_add(1, Ordering::Relaxed);
            }),
            ..Hooks::default()
        };
        let (_, timing) = Pending::new((), None).begin();
        let ((), rejected) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(
                commands.dispatch(&client, &interaction, timing, &hooks),
                reject_callback(&listener, status, code),
            )
        })
        .await
        .map_err(Error::handler)?;
        rejected.map_err(Error::handler)?;
        assert_eq!(reports.load(Ordering::Relaxed), 1);
    }
    Ok(())
}

#[tokio::test]
async fn autocomplete_does_not_send_empty_choices_after_a_definitive_rejection() -> Result<()> {
    let mut interaction = command_interaction(None, None, 5);
    interaction.kind = InteractionType::ApplicationCommandAutocomplete;
    let Some(InteractionData::ApplicationCommand(data)) = &interaction.data else {
        return Err(Error::msg(
            "試験用の Interaction にコマンドデータがありません",
        ));
    };
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(Error::handler)?;
    let client = Client::builder()
        .proxy(
            listener.local_addr().map_err(Error::handler)?.to_string(),
            true,
        )
        .build();
    let (_, timing) = Pending::new((), None).begin();
    let info = InfoReporter::default();
    let mut ctx = AutocompleteContext {
        initial: InitialResponder {
            http: &client,
            interaction: &interaction,
            timing,
            info: &info,
            permissions: &crate::permissions::PermissionResolver::default(),
        },
        data,
        response_finished: false,
    };
    let request = async {
        let choices = vec![SlashCommands::string_choice("例", "value")];
        let rejected = ctx.respond(choices).await;
        assert!(matches!(rejected, Err(error) if callback_unavailable(&error)));
        ctx.respond_empty_if_needed().await
    };
    let (result, mock) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(request, reject_callback(&listener, "404 Not Found", 10062))
    })
    .await
    .map_err(Error::handler)?;
    mock.map_err(Error::handler)?;
    result
}
