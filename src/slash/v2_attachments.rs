use std::time::Instant;

use serde::Serialize;
use twilight_http::{
    Client,
    request::{
        Request,
        attachment::{AttachmentManager, PartialAttachment},
    },
    response::marker::EmptyBody,
    routing::Route,
};
use twilight_model::{
    application::interaction::Interaction,
    channel::{
        Attachment,
        message::{AllowedMentions, Component, MessageFlags},
    },
    http::{attachment::Attachment as Upload, interaction::InteractionResponseType},
    id::{Id, marker::AttachmentMarker},
};

use crate::{Error, Result, ui::ComponentsV2};

use super::{InitialResponder, InitialResponse, initial_response, v2_update_response};

#[cfg(test)]
#[path = "v2_attachment_tests.rs"]
mod tests;

#[derive(Serialize)]
struct Callback<'a> {
    #[serde(rename = "type")]
    kind: InteractionResponseType,
    data: CallbackData<'a>,
}

#[derive(Serialize)]
struct CallbackData<'a> {
    components: &'a [Component],
    allowed_mentions: AllowedMentions,
    flags: MessageFlags,
    attachments: Vec<PartialAttachment<'a>>,
}

impl InitialResponder<'_> {
    pub(super) async fn v2_reply(self, message: &ComponentsV2, ephemeral: bool) -> Result<()> {
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
        let started_at = self.info.measures_timing().then(Instant::now);
        let retained_ids = retained_ids(&message.attachments, retained)?;
        let files = AttachmentManager::new()
            .set_files(message.attachments.iter().collect())
            .set_ids(retained_ids);
        let body = serde_json::to_vec(&Callback {
            kind,
            data: CallbackData {
                components: &message.components,
                allowed_mentions: AllowedMentions::default(),
                flags,
                attachments: files.get_partial_attachments(),
            },
        })
        .map_err(Error::handler)?;
        let request = Request::builder(&Route::InteractionCallback {
            interaction_id: self.interaction.id.get(),
            interaction_token: &self.interaction.token,
            with_response: false,
        })
        .use_authorization_token(false)
        .form(files.build_form(&body))
        .build()?;
        self.http.request::<EmptyBody>(request).await?;
        self.report_sent(kind, started_at);
        Ok(())
    }
}

pub(crate) async fn edit_v2_response(
    http: &Client,
    interaction: &Interaction,
    message: &ComponentsV2,
    retained: Option<&[Attachment]>,
) -> Result<()> {
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

fn retained_ids(uploads: &[Upload], retained: &[Attachment]) -> Result<Vec<Id<AttachmentMarker>>> {
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
