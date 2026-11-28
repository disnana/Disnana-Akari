use twilight_http::Client;
use twilight_model::{
    application::interaction::Interaction,
    channel::{
        Attachment,
        message::{AllowedMentions, MessageFlags},
    },
    http::{attachment::Attachment as Upload, interaction::InteractionResponseType},
    id::{Id, marker::AttachmentMarker},
};

use crate::{Error, Result, ui::ComponentsV2};

use super::{
    InitialResponder, InitialResponse, file_response::FileCallback, initial_response,
    v2_update_response,
};

#[cfg(test)]
#[path = "v2_attachment_tests.rs"]
mod tests;

impl InitialResponder<'_> {
    pub(crate) async fn v2_reply(self, message: &ComponentsV2, ephemeral: bool) -> Result<()> {
        message.validate_attachment_sizes(self.interaction.attachment_size_limit)?;
        message.validate_attachment_references(&[])?;
        if message.attachments.is_empty() {
            return self
                .send(&initial_response(InitialResponse::V2(message), ephemeral))
                .await;
        }
        let mut flags = MessageFlags::IS_COMPONENTS_V2;
        flags.set(MessageFlags::EPHEMERAL, ephemeral);
        self.send_v2_files(
            message,
            InteractionResponseType::ChannelMessageWithSource,
            flags,
            &[],
        )
        .await
    }

    pub(crate) async fn v2_update(self, message: &ComponentsV2) -> Result<()> {
        message.validate_attachment_sizes(self.interaction.attachment_size_limit)?;
        let retained = self
            .interaction
            .message
            .as_ref()
            .map_or(&[][..], |source| source.attachments.as_slice());
        message.validate_attachment_references(retained)?;
        if message.attachments.is_empty() {
            return self.send(&v2_update_response(message)).await;
        }
        self.send_v2_files(
            message,
            InteractionResponseType::UpdateMessage,
            MessageFlags::IS_COMPONENTS_V2,
            retained,
        )
        .await
    }

    async fn send_v2_files(
        self,
        message: &ComponentsV2,
        kind: InteractionResponseType,
        flags: MessageFlags,
        retained: &[Attachment],
    ) -> Result<()> {
        let retained_ids = retained_ids(&message.attachments, retained)?;
        self.send_file_callback(
            FileCallback::V2 {
                kind,
                components: &message.components,
                flags,
            },
            &message.attachments,
            retained_ids,
        )
        .await
    }
}

pub(crate) async fn edit_v2_response(
    http: &Client,
    interaction: &Interaction,
    message: &ComponentsV2,
    retained: Option<&[Attachment]>,
) -> Result<()> {
    message.validate_attachment_sizes(interaction.attachment_size_limit)?;
    if let Some(retained) = retained {
        message.validate_attachment_references(retained)?;
    }
    let mentions = AllowedMentions::default();
    let client = http.interaction(interaction.application_id);
    let mut request = client
        .update_response(&interaction.token)
        .flags(MessageFlags::IS_COMPONENTS_V2)
        .components(Some(&message.components))
        .allowed_mentions(Some(&mentions));
    let existing_ids;
    if !message.attachments.is_empty() {
        existing_ids = retained_ids(&message.attachments, retained.unwrap_or_default())?;
        request = request
            .attachments(&message.attachments)
            .keep_attachment_ids(&existing_ids);
    }
    request.await?;
    Ok(())
}

pub(super) fn retained_ids(
    uploads: &[Upload],
    retained: &[Attachment],
) -> Result<Vec<Id<AttachmentMarker>>> {
    if retained.iter().any(|existing| {
        uploads
            .iter()
            .any(|new| new.filename == existing.filename || new.id == existing.id.get())
    }) {
        return Err(Error::InvalidUi(
            "新しい添付ファイルの名前と ID は、保持する添付ファイルと重複できません。",
        ));
    }
    Ok(retained.iter().map(|attachment| attachment.id).collect())
}
