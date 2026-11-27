use serde::Serialize;
use twilight_http::{
    Client,
    request::{
        Request,
        attachment::{AttachmentManager, PartialAttachment},
    },
    routing::Route,
};
use twilight_model::{
    channel::{
        Message,
        message::{
            AllowedMentions, Component, Embed, MessageFlags, MessageReference, MessageReferenceType,
        },
    },
    http::attachment::Attachment,
};

use crate::{Error, Result};

#[derive(Serialize)]
struct ChannelUiReply<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<&'a str>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    embeds: &'a [Embed],
    components: &'a [Component],
    #[serde(skip_serializing_if = "Option::is_none")]
    flags: Option<MessageFlags>,
    allowed_mentions: AllowedMentions,
    message_reference: MessageReference,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    attachments: Vec<PartialAttachment<'a>>,
}

/// Twilight 0.18 の CreateMessage は Premium ボタンに custom_id を要求します。
/// 呼び出し元で Akari の検証が通ったメッセージを、同じ Twilight Client で送信します。
pub(crate) async fn reply_with_premium_button(
    http: &Client,
    source: &Message,
    content: Option<&str>,
    embeds: &[Embed],
    components: &[Component],
    flags: Option<MessageFlags>,
    uploads: &[Attachment],
) -> Result<()> {
    let files = AttachmentManager::new().set_files(uploads.iter().collect());
    let payload = ChannelUiReply {
        content,
        embeds,
        components,
        flags,
        allowed_mentions: AllowedMentions::default(),
        message_reference: MessageReference {
            kind: MessageReferenceType::Default,
            channel_id: Some(source.channel_id),
            guild_id: None,
            message_id: Some(source.id),
            fail_if_not_exists: None,
        },
        attachments: files.get_partial_attachments(),
    };
    let route = Route::CreateMessage {
        channel_id: source.channel_id.get(),
    };
    let request = Request::builder(&route);
    let request = if uploads.is_empty() {
        request.json(&payload).build()?
    } else {
        let body = serde_json::to_vec(&payload).map_err(Error::handler)?;
        request.form(files.build_form(&body)).build()?
    };
    http.request::<Message>(request).await?;
    Ok(())
}
