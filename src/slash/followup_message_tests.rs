use std::time::Duration;

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::timeout,
};
use twilight_model::application::interaction::InteractionData;

use super::*;
use crate::{
    ResponseStateError,
    hooks::Hooks,
    slash::{
        ResponseState,
        tests::{command_interaction, exchange_response, test_command_message},
    },
    timing::Pending,
    ui::FileDisplay,
};

const TOKEN: &str = "private-followup-fixture";
const FOLLOWUP: &str = "/webhooks/10/private-followup-fixture";
const MESSAGE: &str = "/webhooks/10/private-followup-fixture/messages/19";

async fn local_http() -> Result<(TcpListener, Client)> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(Error::handler)?;
    let http = Client::builder()
        .proxy(
            listener.local_addr().map_err(Error::handler)?.to_string(),
            true,
        )
        .build();
    Ok((listener, http))
}

async fn json_request(
    listener: &TcpListener,
    method: &str,
    path: &str,
    response: &Message,
) -> Result<Value> {
    let response = serde_json::to_string(response).map_err(Error::handler)?;
    let body = exchange_response(listener, method, Some(path), Some(&response))
        .await
        .map_err(Error::handler)?;
    serde_json::from_str(&body).map_err(Error::handler)
}

fn message(content: &str, flags: MessageFlags) -> Result<Message> {
    let mut message = test_command_message(content)?;
    message.id = Id::new(19);
    message.flags = Some(flags);
    Ok(message)
}

fn application_interaction() -> Interaction {
    let mut event = command_interaction(None, None, 5);
    event.token = TOKEN.into();
    event.attachment_size_limit = 32;
    event
}

fn initial<'a>(http: &'a Client, event: &'a Interaction, hooks: &'a Hooks) -> InitialResponder<'a> {
    let (_, timing) = Pending::new((), None).begin();
    InitialResponder {
        http,
        interaction: event,
        timing,
        info: &hooks.info,
        permissions: &hooks.permissions,
    }
}

#[tokio::test]
async fn tracked_classic_followup_uses_the_returned_id_and_refreshes_its_model() -> Result<()> {
    let (listener, http) = local_http().await?;
    let interaction = application_interaction();
    let Some(InteractionData::ApplicationCommand(data)) = &interaction.data else {
        return Err(Error::msg("コマンドの試験データがありません"));
    };
    let hooks = Hooks::default();
    let mut state = ResponseState::Pending;
    let ctx = SlashContext {
        initial: initial(&http, &interaction, &hooks),
        data,
        response_state: &mut state,
    };
    let client = async {
        assert!(matches!(
            ctx.followups(),
            Err(Error::ResponseState(
                ResponseStateError::FollowupBeforeResponse
            ))
        ));
        *ctx.response_state = ResponseState::Deferred { ephemeral: true };
        assert!(matches!(
            ctx.followups(),
            Err(Error::ResponseState(
                ResponseStateError::FollowupBeforeResponse
            ))
        ));
        *ctx.response_state = ResponseState::Replied;
        let followups = ctx.followups()?;
        assert!(!format!("{followups:?}").contains(TOKEN));
        let mut sent = followups.send("<@123>", true).await?;
        assert!(!format!("{sent:?}").contains(TOKEN));
        assert_eq!(sent.id(), Id::new(19));
        assert_eq!(sent.message().flags, Some(MessageFlags::EPHEMERAL));
        assert_eq!(sent.message().content, "<@123>");
        assert!(
            sent.edit_v2(&ComponentsV2::new().text("wrong mode"))
                .await
                .is_err()
        );
        sent.edit("edited").await?;
        assert_eq!(sent.message().content, "edited");
        sent.edit_ui(&UiMessage::new().content("panel")).await?;
        assert_eq!(sent.message().content, "panel");
        sent.delete().await
    };
    let server = async {
        let body = json_request(
            &listener,
            "POST",
            FOLLOWUP,
            &message("<@123>", MessageFlags::EPHEMERAL)?,
        )
        .await?;
        assert_eq!(
            body,
            json!({"content":"<@123>", "flags":64, "allowed_mentions":{"parse":[]}})
        );
        let body = json_request(
            &listener,
            "PATCH",
            MESSAGE,
            &message("edited", MessageFlags::EPHEMERAL)?,
        )
        .await?;
        assert_eq!(
            body,
            json!({"content":"edited", "allowed_mentions":{"parse":[]}})
        );
        let body = json_request(
            &listener,
            "PATCH",
            MESSAGE,
            &message("panel", MessageFlags::EPHEMERAL)?,
        )
        .await?;
        assert_eq!(
            body,
            json!({"content":"panel","embeds":[],"components":[],"allowed_mentions":{"parse":[]}})
        );
        exchange_response(&listener, "DELETE", Some(MESSAGE), None)
            .await
            .map_err(Error::handler)?;
        Ok::<_, Error>(())
    };
    let (client, server) = timeout(Duration::from_secs(5), async {
        tokio::join!(client, server)
    })
    .await
    .map_err(Error::handler)?;
    client?;
    server
}

async fn multipart(
    listener: &TcpListener,
    method: &str,
    path: &str,
    response: &Message,
) -> Result<Value> {
    let response = serde_json::to_string(response).map_err(Error::handler)?;
    let body = exchange_response(listener, method, Some(path), Some(&response))
        .await
        .map_err(Error::handler)?;
    assert!(body.contains("name=\"files[3]\"; filename=\"upload.txt\""));
    let payload = body
        .split_once("Content-Type: application/json\r\n\r\n")
        .and_then(|(_, parts)| parts.lines().next())
        .unwrap();
    serde_json::from_str(payload).map_err(Error::handler)
}

#[tokio::test]
async fn tracked_v2_followup_validates_urls_and_preserves_attachment_ids_on_edit() -> Result<()> {
    let (listener, http) = local_http().await?;
    let interaction = application_interaction();
    let hooks = Hooks::default();
    let followups = Followups::new(initial(&http, &interaction, &hooks));
    let upload = || Attachment::from_bytes("upload.txt".into(), b"file".to_vec(), 3);
    let client = async {
        assert!(
            followups
                .send_v2(
                    &ComponentsV2::new().file(FileDisplay::new("missing.txt")),
                    true
                )
                .await
                .is_err()
        );
        let mut sent = followups
            .send_v2(&ComponentsV2::new().text("first"), true)
            .await?;
        assert!(sent.edit("wrong mode").await.is_err());
        sent.edit_v2(
            &ComponentsV2::new()
                .file(FileDisplay::new("already.txt"))
                .file(FileDisplay::new("upload.txt"))
                .attachment(upload()),
        )
        .await?;
        assert_eq!(
            sent.message().flags,
            Some(MessageFlags::IS_COMPONENTS_V2 | MessageFlags::EPHEMERAL)
        );
        Ok::<_, Error>(())
    };
    let server = async {
        let flags = MessageFlags::IS_COMPONENTS_V2 | MessageFlags::EPHEMERAL;
        let reply = message("", flags)?;
        let body = json_request(&listener, "POST", FOLLOWUP, &reply).await?;
        assert_eq!(
            body,
            json!({"flags":32832,"components":[{"type":10,"content":"first"}],"allowed_mentions":{"parse":[]}})
        );
        let mut current = reply;
        current.attachments = serde_json::from_value(json!([{
            "id":"91","filename":"already.txt","size":4,"url":"https://example.com/already.txt",
            "proxy_url":"https://example.com/already.txt"
        }]))
        .map_err(Error::handler)?;
        let response = serde_json::to_string(&current).map_err(Error::handler)?;
        exchange_response(&listener, "GET", Some(MESSAGE), Some(&response))
            .await
            .map_err(Error::handler)?;
        let patch = multipart(&listener, "PATCH", MESSAGE, &current).await?;
        assert!(patch.get("flags").is_none());
        assert_eq!(
            patch["attachments"],
            json!([{"id":3,"filename":"upload.txt"},{"id":91}])
        );
        assert_eq!(
            patch["components"][0]["file"]["url"],
            "attachment://already.txt"
        );
        assert_eq!(
            patch["components"][1]["file"]["url"],
            "attachment://upload.txt"
        );
        assert_eq!(patch["allowed_mentions"]["parse"], json!([]));
        Ok::<_, Error>(())
    };
    let (client, server) = timeout(Duration::from_secs(5), async {
        tokio::join!(client, server)
    })
    .await
    .map_err(Error::handler)?;
    client?;
    server
}

async fn reject_request(listener: &TcpListener) -> std::io::Result<()> {
    let (stream, _) = listener.accept().await?;
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    stream.read_line(&mut line).await?;
    assert!(line.starts_with(&format!("POST /api/v10{FOLLOWUP}")));
    let mut length = 0;
    loop {
        line.clear();
        if stream.read_line(&mut line).await? == 0 {
            return Err(std::io::Error::other("incomplete fixture request"));
        }
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().map_err(std::io::Error::other)?;
        }
    }
    stream.read_exact(&mut vec![0; length]).await?;
    let body = json!({"code":10015,"message":TOKEN}).to_string();
    let response = format!(
        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.get_mut().write_all(response.as_bytes()).await
}

#[tokio::test]
async fn tracked_followup_errors_do_not_include_webhook_credentials_or_response_body() -> Result<()>
{
    let (listener, http) = local_http().await?;
    let interaction = application_interaction();
    let hooks = Hooks::default();
    let followups = Followups::new(initial(&http, &interaction, &hooks));
    let client = async {
        let error = followups.send("first", false).await.unwrap_err();
        assert!(error.to_string().contains("404"));
        assert!(error.to_string().contains("10015"));
        assert!(!format!("{error} {error:?}").contains(TOKEN));
        let error = followups.send("second", false).await.unwrap_err();
        assert!(!format!("{error} {error:?}").contains(TOKEN));
    };
    let server = async {
        reject_request(&listener).await?;
        exchange_response(&listener, "POST", Some(FOLLOWUP), Some(TOKEN)).await?;
        Ok::<_, std::io::Error>(())
    };
    let ((), server) = timeout(Duration::from_secs(5), async {
        tokio::join!(client, server)
    })
    .await
    .map_err(Error::handler)?;
    server.map_err(Error::handler)
}
