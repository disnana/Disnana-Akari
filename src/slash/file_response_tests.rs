use serde_json::{Value, json};
use tokio::net::TcpListener;
use twilight_http::Client;
use twilight_model::application::interaction::InteractionData;

use super::*;
use crate::{
    Context, ResponseStateError,
    hooks::Hooks,
    permissions::PermissionResolver,
    slash::tests::{command_interaction, exchange_response, test_command_message},
    timing::Pending,
};

const CALLBACK: &str = "/interactions/1/token/callback";
const ORIGINAL: &str = "/webhooks/10/token/messages/@original";
const FOLLOWUP: &str = "/webhooks/10/token";

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

async fn multipart(
    listener: &TcpListener,
    method: &str,
    path: &str,
    callback: bool,
) -> Result<Value> {
    let body = exchange_response(listener, method, Some(path), None)
        .await
        .map_err(Error::handler)?;
    assert!(body.contains("name=\"payload_json\""));
    assert!(body.contains("name=\"files[3]\"; filename=\"report.txt\""));
    assert!(body.contains("\r\n\r\nxyz\r\n--"));
    let payload = body
        .split_once("Content-Type: application/json\r\n\r\n")
        .and_then(|(_, contents)| contents.lines().next())
        .unwrap();
    let payload: Value = serde_json::from_str(payload).map_err(Error::handler)?;
    let message = if callback { &payload["data"] } else { &payload };
    assert_eq!(message["allowed_mentions"]["parse"], json!([]));
    assert_eq!(
        message["attachments"],
        json!([{"id": 3, "filename": "report.txt"}])
    );
    assert!(message.get("components").is_none());
    Ok(payload)
}

fn files() -> [Attachment; 1] {
    [Attachment::from_bytes(
        "report.txt".into(),
        b"xyz".to_vec(),
        3,
    )]
}

#[tokio::test]
async fn classic_files_send_channel_public_and_private_attachment_only_replies() -> Result<()> {
    let (listener, http) = local_http().await?;
    let uploads = files();
    let message = test_command_message("!file")?;
    let (_, timing) = Pending::new((), None).begin();
    let permissions = PermissionResolver::new(None);
    let channel = Context {
        http: &http,
        message: &message,
        timing,
        permissions: &permissions,
    };
    let mut interaction = command_interaction(None, None, 5);
    interaction.token = "token".into();
    interaction.attachment_size_limit = 3;
    let Some(InteractionData::ApplicationCommand(data)) = &interaction.data else {
        return Err(Error::msg(
            "試験用の Interaction にコマンドデータがありません",
        ));
    };
    let hooks = Hooks::default();
    let initial = InitialResponder {
        http: &http,
        interaction: &interaction,
        timing,
        info: &hooks.info,
        permissions: &hooks.permissions,
    };
    let mut public = ResponseState::Pending;
    let mut private = ResponseState::Pending;
    let request = async {
        let limits = AttachmentLimits::default();
        channel.reply_files("", &uploads, limits).await?;
        SlashContext {
            initial,
            data,
            response_state: &mut public,
        }
        .reply_files("<@123>", &uploads, limits)
        .await?;
        SlashContext {
            initial,
            data,
            response_state: &mut private,
        }
        .reply_files_ephemeral("", &uploads, limits)
        .await
    };
    let server = async {
        let channel = multipart(&listener, "POST", "/channels/2/messages", false).await?;
        assert_eq!(channel["message_reference"]["message_id"], "1");
        let public = multipart(&listener, "POST", CALLBACK, true).await?;
        assert_eq!(public["type"], 4);
        assert_eq!(public["data"]["content"], "<@123>");
        assert!(public["data"].get("flags").is_none());
        let private = multipart(&listener, "POST", CALLBACK, true).await?;
        assert_eq!(private["type"], 4);
        assert_eq!(private["data"]["flags"], 64);
        assert!(private["data"].get("content").is_none());
        Ok::<_, Error>(())
    };
    let (request, server) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(request, server)
    })
    .await
    .map_err(Error::handler)?;
    request?;
    server?;
    assert_eq!(public, ResponseState::Replied);
    assert_eq!(private, ResponseState::Replied);
    Ok(())
}

#[tokio::test]
async fn classic_files_reject_invalid_or_locked_replies_before_completing_deferred() -> Result<()> {
    let (listener, http) = local_http().await?;
    let uploads = files();
    let mut interaction = command_interaction(None, None, 5);
    interaction.token = "token".into();
    interaction.attachment_size_limit = 3;
    let Some(InteractionData::ApplicationCommand(data)) = &interaction.data else {
        return Err(Error::msg(
            "試験用の Interaction にコマンドデータがありません",
        ));
    };
    let hooks = Hooks::default();
    let (_, timing) = Pending::new((), None).begin();
    let initial = InitialResponder {
        http: &http,
        interaction: &interaction,
        timing,
        info: &hooks.info,
        permissions: &hooks.permissions,
    };
    let mut state = ResponseState::Pending;
    let mut private_state = ResponseState::Pending;
    let mut ctx = SlashContext {
        initial,
        data,
        response_state: &mut state,
    };
    let request = async {
        let limits = AttachmentLimits::default();
        let oversized = [Attachment::from_bytes("too-big.txt".into(), vec![0; 4], 0)];
        for error in [
            ctx.reply_files("", &[], limits).await,
            ctx.reply_files("", &oversized, limits).await,
            ctx.reply_files("", &uploads, limits.max_total_bytes(Some(2)))
                .await,
        ] {
            assert!(matches!(error, Err(Error::InvalidUi(_))));
        }

        ctx.defer().await?;
        assert!(matches!(
            ctx.reply_files_ephemeral("", &uploads, limits).await,
            Err(Error::ResponseState(ResponseStateError::VisibilityLocked))
        ));
        ctx.reply_files("", &uploads, limits).await?;
        assert!(matches!(
            ctx.reply_files("", &uploads, limits).await,
            Err(Error::ResponseState(ResponseStateError::AlreadyResponded))
        ));
        let mut private = SlashContext {
            initial,
            data,
            response_state: &mut private_state,
        };
        private.defer_ephemeral().await?;
        private
            .reply_files_ephemeral("private", &uploads, limits)
            .await?;
        Ok::<_, Error>(())
    };
    let server = async {
        let defer = exchange_response(&listener, "POST", Some(CALLBACK), None)
            .await
            .map_err(Error::handler)?;
        assert_eq!(
            serde_json::from_str::<Value>(&defer).map_err(Error::handler)?,
            json!({"type": 5})
        );
        let patch = multipart(&listener, "PATCH", ORIGINAL, false).await?;
        assert!(patch.get("flags").is_none());
        assert!(patch.get("content").is_some_and(Value::is_null));
        let defer = exchange_response(&listener, "POST", Some(CALLBACK), None)
            .await
            .map_err(Error::handler)?;
        assert_eq!(
            serde_json::from_str::<Value>(&defer).map_err(Error::handler)?,
            json!({"type": 5, "data": {"flags": 64}})
        );
        let patch = multipart(&listener, "PATCH", ORIGINAL, false).await?;
        assert!(patch.get("flags").is_none());
        assert_eq!(patch["content"], "private");
        Ok::<_, Error>(())
    };
    let (request, server) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(request, server)
    })
    .await
    .map_err(Error::handler)?;
    request?;
    server?;
    assert_eq!(state, ResponseState::Replied);
    assert_eq!(private_state, ResponseState::Replied);
    Ok(())
}

#[tokio::test]
async fn file_followups_preserve_visibility_and_validate_before_sending() -> Result<()> {
    let (listener, http) = local_http().await?;
    let uploads = files();
    let mut interaction = command_interaction(None, None, 5);
    interaction.token = "token".into();
    interaction.attachment_size_limit = 3;
    let Some(InteractionData::ApplicationCommand(data)) = &interaction.data else {
        return Err(Error::msg(
            "試験用の Interaction にコマンドデータがありません",
        ));
    };
    let hooks = Hooks::default();
    let (_, timing) = Pending::new((), None).begin();
    let mut state = ResponseState::Pending;
    let mut ctx = SlashContext {
        initial: InitialResponder {
            http: &http,
            interaction: &interaction,
            timing,
            info: &hooks.info,
            permissions: &hooks.permissions,
        },
        data,
        response_state: &mut state,
    };
    let request = async {
        let limits = AttachmentLimits::default();
        assert!(matches!(
            ctx.followup_files_ephemeral("", &uploads, limits).await,
            Err(Error::ResponseState(
                ResponseStateError::FollowupBeforeResponse
            ))
        ));
        ctx.defer().await?;
        assert!(matches!(
            ctx.followup_files_ephemeral("", &uploads, limits).await,
            Err(Error::ResponseState(
                ResponseStateError::FollowupBeforeResponse
            ))
        ));
        ctx.reply("準備完了").await?;

        let oversized = [Attachment::from_bytes("too-big.txt".into(), vec![0; 4], 0)];
        for error in [
            ctx.followup_files("", &[], limits).await,
            ctx.followup_files("", &oversized, limits).await,
            ctx.followup_files("", &uploads, limits.max_total_bytes(Some(2)))
                .await,
        ] {
            assert!(matches!(error, Err(Error::InvalidUi(_))));
        }

        ctx.followup_files("<@123>", &uploads, limits).await?;
        ctx.followup_files_ephemeral("", &uploads, limits).await
    };
    let server = async {
        exchange_response(&listener, "POST", Some(CALLBACK), None)
            .await
            .map_err(Error::handler)?;
        exchange_response(&listener, "PATCH", Some(ORIGINAL), None)
            .await
            .map_err(Error::handler)?;
        let public = multipart(&listener, "POST", FOLLOWUP, false).await?;
        assert_eq!(public["content"], "<@123>");
        assert!(public.get("flags").is_none());
        let private = multipart(&listener, "POST", FOLLOWUP, false).await?;
        assert_eq!(private["flags"], 64);
        assert!(private.get("content").is_none());
        Ok::<_, Error>(())
    };
    let (request, server) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(request, server)
    })
    .await
    .map_err(Error::handler)?;
    request?;
    server?;
    assert_eq!(state, ResponseState::Replied);
    Ok(())
}
