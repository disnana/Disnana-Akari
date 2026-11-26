use twilight_http::{Client, error::ErrorType, response::StatusCode};
use twilight_model::{
    application::interaction::{InteractionMetadata, InteractionType},
    channel::{
        Message,
        message::{MessageReferenceType, MessageType},
    },
    id::{
        Id,
        marker::{MessageMarker, UserMarker},
    },
};

use crate::Result;

pub(super) fn message_invoker(metadata: &InteractionMetadata) -> Option<Id<UserMarker>> {
    match metadata.kind {
        InteractionType::ApplicationCommand
        | InteractionType::MessageComponent
        | InteractionType::ModalSubmit => Some(metadata.user.id),
        _ => None,
    }
}

pub(super) fn message_owner(message: &Message) -> Option<Id<UserMarker>> {
    if let Some(metadata) = &message.interaction_metadata {
        return message_invoker(metadata);
    }

    let target_id = reply_target_id(message)?;
    let target = message.referenced_message.as_deref()?;
    reply_author(message, target_id, target)
}

pub(super) async fn resolve_owner(
    http: &Client,
    message: &Message,
) -> Result<Option<Id<UserMarker>>> {
    if message.interaction_metadata.is_some() || message.referenced_message.is_some() {
        return Ok(message_owner(message));
    }
    let Some(target_id) = reply_target_id(message) else {
        return Ok(None);
    };

    let response = match http.message(message.channel_id, target_id).await {
        Ok(response) => response,
        Err(error) if matches!(error.kind(), ErrorType::Response { status, .. } if *status == StatusCode::NOT_FOUND) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    let target = response.model().await?;
    Ok(reply_author(message, target_id, &target))
}

fn reply_target_id(message: &Message) -> Option<Id<MessageMarker>> {
    if message.kind != MessageType::Reply || !message.author.bot || message.webhook_id.is_some() {
        return None;
    }
    let reference = message.reference.as_ref()?;
    if reference.kind != MessageReferenceType::Default
        || reference.channel_id != Some(message.channel_id)
    {
        return None;
    }
    reference.message_id
}

fn reply_author(
    message: &Message,
    target_id: Id<MessageMarker>,
    target: &Message,
) -> Option<Id<UserMarker>> {
    (target.id == target_id
        && target.channel_id == message.channel_id
        && !target.author.bot
        && target.webhook_id.is_none())
    .then_some(target.author.id)
}
