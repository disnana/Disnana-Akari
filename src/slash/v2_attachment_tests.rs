use serde_json::{Value, json};
use tokio::net::TcpListener;
use twilight_model::application::interaction::{
    InteractionData, InteractionType, message_component::MessageComponentInteractionData,
};
use twilight_model::channel::message::component::ComponentType;

use super::*;
use crate::{
    ButtonContext, Context, Events,
    hooks::Hooks,
    permissions::PermissionResolver,
    slash::{
        ResponseState, SlashContext,
        tests::{command_interaction, exchange_response, test_command_message},
    },
    timing::Pending,
    ui::{Button, FileDisplay, UiMessage},
    validation::AttachmentLimits,
};

fn upload() -> ComponentsV2 {
    ComponentsV2::new()
        .file(FileDisplay::new("manual.pdf").spoiler(true))
        .attachment(Upload::from_bytes(
            "manual.pdf".into(),
            b"pdf-content".to_vec(),
            0,
        ))
}

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

async fn multipart_payload(listener: &TcpListener, method: &str, path: &str) -> Result<Value> {
    let body = exchange_response(listener, method, Some(path), None)
        .await
        .map_err(Error::handler)?;
    assert!(body.contains("name=\"payload_json\""));
    assert!(body.contains("name=\"files[0]\"; filename=\"manual.pdf\""));
    assert!(body.contains("\r\n\r\npdf-content\r\n--"));
    let payload = body
        .split_once("Content-Type: application/json\r\n\r\n")
        .and_then(|(_, contents)| contents.lines().next())
        .unwrap();
    serde_json::from_str(payload).map_err(Error::handler)
}

async fn multipart(listener: &TcpListener, method: &str, path: &str) -> Result<Value> {
    let payload = multipart_payload(listener, method, path).await?;
    let data = payload.get("data").unwrap_or(&payload);
    assert_eq!(data["allowed_mentions"]["parse"], json!([]));
    assert_eq!(
        data["components"],
        json!([{
            "type": 13, "file": {"url": "attachment://manual.pdf"}, "spoiler": true
        }])
    );
    assert_eq!(
        data["attachments"][0],
        json!({"id": 0, "filename": "manual.pdf"})
    );
    Ok(payload)
}

#[tokio::test]
async fn channel_and_private_initial_v2_reply_upload_files() -> Result<()> {
    let (listener, http) = local_http().await?;
    let message = test_command_message("!file")?;
    let (_, timing) = Pending::new((), None).begin();
    let permissions = PermissionResolver::new(None);
    let ctx = Context {
        http: &http,
        message: &message,
        timing,
        permissions: &permissions,
    };
    let mut interaction = command_interaction(None, None, 5);
    interaction.attachment_size_limit = 100;
    interaction.token = "token".into();
    let Some(InteractionData::ApplicationCommand(data)) = &interaction.data else {
        return Err(Error::msg(
            "試験用の Interaction にコマンドデータがありません",
        ));
    };
    let hooks = Hooks::default();
    let mut state = ResponseState::Pending;
    let mut slash = SlashContext {
        initial: InitialResponder {
            http: &http,
            interaction: &interaction,
            timing,
            info: &hooks.info,
        },
        data,
        response_state: &mut state,
    };
    let message = upload();
    let client = async {
        ctx.reply_v2(&message).await?;
        slash.reply_v2_ephemeral(&message).await
    };
    let server = async {
        let channel_side = multipart(&listener, "POST", "/channels/2/messages").await?;
        assert_eq!(channel_side["flags"], 32768);
        assert_eq!(channel_side["message_reference"]["message_id"], "1");
        let callback = multipart(&listener, "POST", "/interactions/1/token/callback").await?;
        assert_eq!(callback["type"], 4);
        assert_eq!(callback["data"]["flags"], 32832);
        Ok::<_, Error>(())
    };
    let (client, server) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(client, server)
    })
    .await
    .map_err(Error::handler)?;
    client?;
    server?;
    assert_eq!(state, ResponseState::RepliedV2);
    Ok(())
}

#[tokio::test]
async fn deferred_v2_reply_uploads_file_when_completing_private_original() -> Result<()> {
    let (listener, http) = local_http().await?;
    let mut interaction = command_interaction(None, None, 5);
    interaction.attachment_size_limit = 100;
    interaction.token = "token".into();
    let Some(InteractionData::ApplicationCommand(data)) = &interaction.data else {
        return Err(Error::msg(
            "試験用の Interaction にコマンドデータがありません",
        ));
    };
    let hooks = Hooks::default();
    let (_, timing) = Pending::new((), None).begin();
    let mut state = ResponseState::Pending;
    let mut slash = SlashContext {
        initial: InitialResponder {
            http: &http,
            interaction: &interaction,
            timing,
            info: &hooks.info,
        },
        data,
        response_state: &mut state,
    };
    let client = async {
        slash.defer_ephemeral().await?;
        slash.reply_v2(&upload()).await
    };
    let server = async {
        let defer: Value = serde_json::from_str(
            &exchange_response(
                &listener,
                "POST",
                Some("/interactions/1/token/callback"),
                None,
            )
            .await
            .map_err(Error::handler)?,
        )
        .map_err(Error::handler)?;
        assert_eq!(defer["type"], 5);
        assert_eq!(defer["data"]["flags"], 64);
        let edit = multipart(&listener, "PATCH", "/webhooks/10/token/messages/@original").await?;
        assert_eq!(edit["flags"], 32768);
        assert_eq!(edit["attachments"].as_array().unwrap().len(), 1);
        Ok::<_, Error>(())
    };
    let (client, server) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(client, server)
    })
    .await
    .map_err(Error::handler)?;
    client?;
    server?;
    assert_eq!(state, ResponseState::RepliedV2);
    Ok(())
}

fn retained_pdf(id: u64, filename: &str) -> Result<Attachment> {
    let url = format!("https://cdn.example/{filename}");
    serde_json::from_value(json!({
        "id": id.to_string(), "filename": filename, "size": 1, "url": url, "proxy_url": url,
    }))
    .map_err(Error::handler)
}

#[tokio::test]
async fn component_v2_updates_keep_existing_files_for_callback_and_patch() -> Result<()> {
    let (listener, http) = local_http().await?;
    let mut interaction = command_interaction(None, None, 5);
    interaction.attachment_size_limit = 100;
    interaction.kind = InteractionType::MessageComponent;
    interaction.token = "token".into();
    let mut source = test_command_message("")?;
    source.flags = Some(MessageFlags::IS_COMPONENTS_V2);
    source.attachments.push(retained_pdf(900, "old.pdf")?);
    upload()
        .file(FileDisplay::new("old.pdf"))
        .validate_attachment_references(&source.attachments)?;
    let conflicting_name = Upload::from_bytes("old.pdf".into(), vec![1], 1);
    let conflicting_id = Upload::from_bytes("new.pdf".into(), vec![1], 900);
    assert!(retained_ids(&[conflicting_name], &source.attachments).is_err());
    assert!(retained_ids(&[conflicting_id], &source.attachments).is_err());
    let mut current = source.clone();
    current.attachments.push(retained_pdf(901, "later.pdf")?);
    interaction.message = Some(source);
    let mut events = Events::new();
    events.on_button("upload", async |ctx: &mut ButtonContext<'_>| {
        ctx.update_v2(&upload()).await
    })?;
    events.on_button("defer", async |ctx: &mut ButtonContext<'_>| -> Result<()> {
        ctx.defer_update().await?;
        let oversized = ComponentsV2::new()
            .text("over limit")
            .attachment(Upload::from_bytes("oversized.pdf".into(), vec![0; 101], 0));
        assert!(matches!(
            ctx.update_v2(&oversized).await,
            Err(Error::InvalidUi(_))
        ));
        ctx.update_v2(&upload()).await
    })?;
    events.on_button("plain", async |ctx: &mut ButtonContext<'_>| -> Result<()> {
        ctx.defer_update().await?;
        ctx.update_v2(&ComponentsV2::new().text("plain")).await
    })?;
    let hooks = Hooks::default();
    let client = async {
        for id in ["upload", "defer", "plain"] {
            interaction.data = Some(InteractionData::MessageComponent(Box::new(
                MessageComponentInteractionData {
                    component_type: ComponentType::Button,
                    custom_id: id.into(),
                    resolved: None,
                    values: Vec::new(),
                },
            )));
            let (_, timing) = Pending::new((), None).begin();
            events
                .interaction_handlers
                .dispatch(&http, &interaction, timing, &hooks, &[])
                .await;
        }
    };
    let server = async {
        let callback_path = "/interactions/1/token/callback";
        let callback = multipart(&listener, "POST", callback_path).await?;
        assert_eq!(callback["type"], 7);
        assert_eq!(callback["data"]["attachments"][1], json!({"id": 900}));
        let defer: Value = serde_json::from_str(
            &exchange_response(&listener, "POST", Some(callback_path), None)
                .await
                .map_err(Error::handler)?,
        )
        .map_err(Error::handler)?;
        assert_eq!(defer["type"], 6);
        let original = "/webhooks/10/token/messages/@original";
        let current = serde_json::to_string(&current).map_err(Error::handler)?;
        exchange_response(&listener, "GET", Some(original), Some(&current))
            .await
            .map_err(Error::handler)?;
        let edit = multipart(&listener, "PATCH", original).await?;
        assert_eq!(edit["attachments"][1], json!({"id": 900}));
        assert_eq!(edit["attachments"][2], json!({"id": 901}));
        exchange_response(&listener, "POST", Some(callback_path), None)
            .await
            .map_err(Error::handler)?;
        let plain = exchange_response(&listener, "PATCH", Some(original), None)
            .await
            .map_err(Error::handler)?;
        let plain: Value = serde_json::from_str(&plain).map_err(Error::handler)?;
        assert_eq!(plain["components"][0]["content"], "plain");
        assert!(plain.get("attachments").is_none());
        Ok::<_, Error>(())
    };
    let (_, server) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(client, server)
    })
    .await
    .map_err(Error::handler)?;
    server
}

#[tokio::test]
async fn invalid_v2_upload_limits_are_rejected_before_http() -> Result<()> {
    let (listener, http) = local_http().await?;
    let mut interaction = command_interaction(None, None, 5);
    interaction.attachment_size_limit = 100;
    let mut source = test_command_message("")?;
    let stub: Attachment = serde_json::from_value(json!({
        "id": "900", "filename": "old.pdf", "size": 500, "url": "https://cdn.example/old.pdf",
        "proxy_url": "https://cdn.example/old.pdf"
    }))
    .map_err(Error::handler)?;
    for id in 900..910 {
        let mut old = stub.clone();
        old.id = Id::new(id);
        old.filename = format!("{id}.pdf");
        source.attachments.push(old);
    }
    interaction.message = Some(source);
    let mut tiny_limit = command_interaction(None, None, 5);
    tiny_limit.attachment_size_limit = 10;
    let message = upload();
    let hooks = Hooks::default();
    let (_, timing) = Pending::new((), None).begin();
    let initial = InitialResponder {
        http: &http,
        interaction: &interaction,
        timing,
        info: &hooks.info,
    };
    let tiny = InitialResponder {
        interaction: &tiny_limit,
        ..initial
    };
    let retained = &interaction.message.as_ref().unwrap().attachments;
    message.validate_attachment_references(retained.get(..9).unwrap())?;
    let check = async {
        for result in [
            initial.v2_update(&message).await,
            edit_v2_response(&http, &interaction, &message, Some(retained)).await,
            tiny.v2_reply(&message, true).await,
            edit_v2_response(&http, &tiny_limit, &message, Some(&[])).await,
            ResponseState::RepliedV2
                .edit_original_v2(tiny, &message)
                .await,
        ] {
            assert!(matches!(result, Err(Error::InvalidUi(_))));
        }
        Ok(())
    };
    tokio::select! {
        biased;
        connection = listener.accept() => panic!("invalid upload attempted HTTP: {connection:?}"),
        result = check => result,
    }
}

#[tokio::test]
async fn premium_channel_replies_preserve_json_and_multipart_contracts() -> Result<()> {
    let (listener, http) = local_http().await?;
    let source = test_command_message("!store")?;
    let (_, timing) = Pending::new((), None).begin();
    let permissions = PermissionResolver::new(None);
    let ctx = Context {
        http: &http,
        message: &source,
        timing,
        permissions: &permissions,
    };
    let premium = Button::premium(Id::new(123));
    let legacy = UiMessage::new()
        .content("Store")
        .buttons(vec![premium.clone()]);
    let v2 = upload().buttons(vec![premium]);
    let client = async {
        ctx.reply_ui(&legacy).await?;
        ctx.reply_v2(&v2).await
    };
    let server = async {
        let body = exchange_response(&listener, "POST", Some("/channels/2/messages"), None)
            .await
            .map_err(Error::handler)?;
        let legacy: Value = serde_json::from_str(&body).map_err(Error::handler)?;
        let v2 = multipart_payload(&listener, "POST", "/channels/2/messages").await?;
        assert_eq!(legacy["content"], "Store");
        assert_eq!(v2["flags"], 32768);
        assert_eq!(v2["attachments"][0]["filename"], "manual.pdf");
        for (json, index) in [(&legacy, 0), (&v2, 1)] {
            assert_eq!(json["message_reference"]["message_id"], "1");
            assert_eq!(json["allowed_mentions"]["parse"], json!([]));
            assert_eq!(json["components"][index]["components"][0]["sku_id"], "123");
            assert!(
                json["components"][index]["components"][0]
                    .get("custom_id")
                    .is_none()
            );
        }
        Ok::<_, Error>(())
    };
    let (client, server) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(client, server)
    })
    .await
    .map_err(Error::handler)?;
    client?;
    server
}

#[tokio::test]
async fn channel_replies_reject_oversized_multipart_before_sending() -> Result<()> {
    let (listener, http) = local_http().await?;
    let source = test_command_message("!file")?;
    let (_, timing) = Pending::new((), None).begin();
    let permissions = PermissionResolver::new(None);
    let ctx = Context {
        http: &http,
        message: &source,
        timing,
        permissions: &permissions,
    };
    let check = async {
        let size = crate::validation::CREATE_MESSAGE_REQUEST_MAX_BYTES as usize - 1;
        let file = Upload::from_bytes("manual.pdf".into(), vec![0; size], 0);
        assert!(matches!(
            ctx.reply_files("", std::slice::from_ref(&file), AttachmentLimits::default())
                .await,
            Err(Error::InvalidUi(_))
        ));

        let message = ComponentsV2::new()
            .file(FileDisplay::new("manual.pdf"))
            .attachment(file);
        assert!(matches!(
            ctx.reply_v2(&message).await,
            Err(Error::InvalidUi(_))
        ));
        let message = message.buttons(vec![Button::premium(Id::new(123))]);
        assert!(matches!(
            ctx.reply_v2(&message).await,
            Err(Error::InvalidUi(_))
        ));
        Ok(())
    };
    tokio::select! {
        biased;
        _ = listener.accept() => Err(Error::msg("上限を超えた multipart リクエストを送信しました")),
        result = check => result,
    }
}

#[tokio::test]
async fn app_limit_rejects_before_lookup_or_send() -> Result<()> {
    let (listener, http) = local_http().await?;
    let source = test_command_message("!file")?;
    let (_, timing) = Pending::new((), None).begin();
    let permissions = PermissionResolver::new(None);
    let ctx = Context {
        http: &http,
        message: &source,
        timing,
        permissions: &permissions,
    };
    let mut interaction = command_interaction(None, None, 5);
    interaction.attachment_size_limit = 100;
    let hooks = Hooks::default();
    let initial = InitialResponder {
        http: &http,
        interaction: &interaction,
        timing,
        info: &hooks.info,
    };
    let message = upload().attachment_limits(AttachmentLimits::default().max_file_bytes(Some(2)));
    let check = async {
        assert!(matches!(
            ctx.reply_v2(&message).await,
            Err(Error::InvalidUi(_))
        ));
        assert!(matches!(
            ResponseState::RepliedV2
                .edit_original_v2(initial, &message)
                .await,
            Err(Error::InvalidUi(_))
        ));
        Ok(())
    };
    tokio::select! {
        biased;
        connection = listener.accept() => panic!("invalid upload attempted HTTP: {connection:?}"),
        result = check => result,
    }
}
