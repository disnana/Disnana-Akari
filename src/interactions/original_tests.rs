use std::time::Duration;

use serde_json::{Value, json};
use tokio::{net::TcpListener, time::timeout};
use twilight_model::channel::message::MessageFlags;

use super::{
    ButtonContext, Client, ComponentType, Id, InitialResponder, Interaction, InteractionType,
    SelectContext,
    tests::{component_payload, interaction, message, user},
};
use crate::{
    Error, ResponseStateError, Result,
    hooks::Hooks,
    slash::tests::exchange_response,
    timing::Pending,
    ui::{ComponentsV2, UiMessage},
};

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

fn component_interaction(kind: ComponentType) -> Interaction {
    let mut event = interaction(
        InteractionType::MessageComponent,
        component_payload(kind, "test"),
    );
    event.token = "component-token".into();
    event.message = Some(message(99, user(4, true)));
    event
}

fn component_initial<'a>(
    http: &'a Client,
    event: &'a Interaction,
    hooks: &'a Hooks,
) -> InitialResponder<'a> {
    let (_, timing) = Pending::new((), None).begin();
    InitialResponder {
        http,
        interaction: event,
        timing,
        info: &hooks.info,
    }
}

const CALLBACK: Option<&str> = Some("/interactions/1/component-token/callback");
const ORIGINAL: Option<&str> = Some("/webhooks/10/component-token/messages/@original");

async fn json_request(listener: &TcpListener, method: &str, path: Option<&str>) -> Result<Value> {
    let body = exchange_response(listener, method, path, None)
        .await
        .map_err(Error::handler)?;
    serde_json::from_str(&body).map_err(Error::handler)
}

fn unavailable<T>(result: Result<T>) {
    assert!(matches!(
        result,
        Err(Error::ResponseState(
            ResponseStateError::OriginalResponseUnavailable
        ))
    ));
}

#[tokio::test]
async fn button_original_edits_and_deletes_only_a_successful_private_reply() -> Result<()> {
    let (listener, http) = local_http().await?;
    let event = component_interaction(ComponentType::Button);
    let hooks = Hooks::default();
    let mut ctx = ButtonContext::new(component_initial(&http, &event, &hooks), "test", None);
    let client = async {
        unavailable(ctx.original_response().await);
        unavailable(ctx.edit_original_response("missing").await);
        unavailable(ctx.delete_original_response().await);
        assert!(ctx.reply_v2_ephemeral(&ComponentsV2::new()).await.is_err());
        unavailable(ctx.delete_original_response().await);

        ctx.reply_ephemeral("private").await?;
        assert_eq!(ctx.message().map(|message| message.id), Some(Id::new(99)));
        let received = ctx.original_response().await?;
        assert_eq!(received.id, Id::new(7));
        assert_eq!(received.flags, Some(MessageFlags::EPHEMERAL));
        assert!(matches!(
            ctx.update_message(&UiMessage::new().content("source"))
                .await,
            Err(Error::ResponseState(ResponseStateError::CannotUpdate))
        ));
        ctx.edit_original_response("<@123>").await?;
        ctx.edit_original_ui(&UiMessage::new().content("private edit"))
            .await?;
        ctx.delete_original_response().await?;
        unavailable(ctx.original_response().await);
        unavailable(ctx.edit_original_response("deleted").await);
        unavailable(ctx.delete_original_response().await);
        assert!(ctx.reply_ephemeral("duplicate").await.is_err());
        Ok::<_, Error>(())
    };
    let server = async {
        let callback = json_request(&listener, "POST", CALLBACK).await?;
        assert_eq!(callback["type"], 4);
        assert_eq!(callback["data"]["flags"], 64);
        assert_eq!(callback["data"]["content"], "private");
        let mut reply = message(7, user(4, true));
        reply.flags = Some(MessageFlags::EPHEMERAL);
        let model = serde_json::to_string(&reply).map_err(Error::handler)?;
        exchange_response(&listener, "GET", ORIGINAL, Some(&model))
            .await
            .map_err(Error::handler)?;
        let patch = json_request(&listener, "PATCH", ORIGINAL).await?;
        assert_eq!(patch["content"], "<@123>");
        assert_eq!(patch["allowed_mentions"]["parse"], json!([]));
        assert!(patch.get("flags").is_none());
        assert!(patch.get("attachments").is_none());
        let patch = json_request(&listener, "PATCH", ORIGINAL).await?;
        assert_eq!(patch["content"], "private edit");
        assert_eq!(patch["components"], json!([]));
        exchange_response(&listener, "DELETE", ORIGINAL, None)
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

#[tokio::test]
async fn select_defer_update_keeps_own_original_unavailable_but_can_update_source() -> Result<()> {
    let (listener, http) = local_http().await?;
    let event = component_interaction(ComponentType::TextSelectMenu);
    let hooks = Hooks::default();
    let mut ctx = SelectContext {
        inner: ButtonContext::new(component_initial(&http, &event, &hooks), "test", None),
        values: &[],
    };
    let client = async {
        unavailable(ctx.delete_original_response().await);
        ctx.defer_update().await?;
        unavailable(ctx.original_response().await);
        unavailable(ctx.edit_original_response("wrong target").await);
        unavailable(ctx.delete_original_response().await);
        assert_eq!(ctx.message().map(|message| message.id), Some(Id::new(99)));
        ctx.update_message(&UiMessage::new().content("source edit"))
            .await?;
        unavailable(ctx.original_response().await);
        unavailable(
            ctx.edit_original_ui(&UiMessage::new().content("wrong target"))
                .await,
        );
        unavailable(ctx.delete_original_response().await);
        Ok::<_, Error>(())
    };
    let server = async {
        let callback = json_request(&listener, "POST", CALLBACK).await?;
        assert_eq!(callback, json!({"type": 6}));
        let patch = json_request(&listener, "PATCH", ORIGINAL).await?;
        assert_eq!(patch["content"], "source edit");
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

#[tokio::test]
async fn select_can_create_and_edit_a_private_v2_original() -> Result<()> {
    let (listener, http) = local_http().await?;
    let event = component_interaction(ComponentType::TextSelectMenu);
    let hooks = Hooks::default();
    let mut ctx = SelectContext {
        inner: ButtonContext::new(component_initial(&http, &event, &hooks), "test", None),
        values: &[],
    };
    let client = async {
        ctx.reply_v2_ephemeral(&ComponentsV2::new().text("private panel"))
            .await?;
        assert!(matches!(
            ctx.edit_original_response("wrong format").await,
            Err(Error::ResponseState(
                ResponseStateError::OriginalResponseFormatMismatch
            ))
        ));
        ctx.edit_original_v2(&ComponentsV2::new().text("updated panel"))
            .await?;
        ctx.delete_original_response().await
    };
    let server = async {
        let callback = json_request(&listener, "POST", CALLBACK).await?;
        assert_eq!(callback["type"], 4);
        assert_eq!(callback["data"]["flags"], 32768 | 64);
        assert_eq!(
            callback["data"]["components"][0]["content"],
            "private panel"
        );
        let patch = json_request(&listener, "PATCH", ORIGINAL).await?;
        assert_eq!(patch["flags"], 32768);
        assert_eq!(patch["components"][0]["content"], "updated panel");
        assert!(patch.get("attachments").is_none());
        exchange_response(&listener, "DELETE", ORIGINAL, None)
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
