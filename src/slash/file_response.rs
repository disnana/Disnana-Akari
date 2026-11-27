use std::time::Instant;

use serde::Serialize;
use twilight_http::{
    request::{
        Request,
        attachment::{AttachmentManager, PartialAttachment},
    },
    response::marker::EmptyBody,
    routing::Route,
};
use twilight_model::{
    channel::message::{AllowedMentions, Component, MessageFlags},
    http::{attachment::Attachment, interaction::InteractionResponseType},
    id::{Id, marker::AttachmentMarker},
};

use crate::{
    Error, Result,
    validation::{self, AttachmentLimits},
};

use super::{InitialResponder, ReplyAction, ResponseState, SlashContext};

#[cfg(test)]
#[path = "file_response_tests.rs"]
mod tests;

impl SlashContext<'_> {
    /// 本文とファイルで返信します。ファイルだけを送る場合は空の本文を指定できます。
    /// 遅延応答の後は待機表示を完成させ、既存の公開範囲を引き継ぎます。
    ///
    /// # エラー
    /// 初回応答済み、無効な本文・添付、上限超過、または Discord が受け付けない場合に失敗します。
    pub async fn reply_files(
        &mut self,
        content: &str,
        files: &[Attachment],
        limits: AttachmentLimits,
    ) -> Result<()> {
        self.respond_files(content, files, limits, false).await
    }

    /// 本文とファイルで、実行者だけに見える返信を送ります。
    /// [`Self::defer_ephemeral`] の後に呼ぶと待機表示を完成させます。
    ///
    /// # エラー
    /// 通常の遅延応答で公開範囲を確定した場合や、[`Self::reply_files`] と同じ条件で失敗します。
    pub async fn reply_files_ephemeral(
        &mut self,
        content: &str,
        files: &[Attachment],
        limits: AttachmentLimits,
    ) -> Result<()> {
        self.respond_files(content, files, limits, true).await
    }

    async fn respond_files(
        &mut self,
        content: &str,
        files: &[Attachment],
        limits: AttachmentLimits,
        ephemeral: bool,
    ) -> Result<()> {
        let action = self.response_state.reply_action(ephemeral)?;
        let initial = self.initial;
        let interaction = initial.interaction;
        validation::reply_files(
            content,
            files,
            limits,
            Some(interaction.attachment_size_limit),
        )?;
        if files.is_empty() {
            return self.response_state.reply(initial, content, ephemeral).await;
        }
        match action {
            ReplyAction::Initial => {
                initial
                    .send_file_callback(
                        FileCallback::Classic { content, ephemeral },
                        files,
                        Vec::new(),
                    )
                    .await?;
            }
            ReplyAction::EditDeferred => {
                initial
                    .http
                    .interaction(interaction.application_id)
                    .update_response(&interaction.token)
                    .content((!content.is_empty()).then_some(content))
                    .allowed_mentions(Some(&AllowedMentions::default()))
                    .attachments(files)
                    .await?;
            }
        }
        *self.response_state = ResponseState::Replied;
        Ok(())
    }
}

pub(super) enum FileCallback<'a> {
    Classic {
        content: &'a str,
        ephemeral: bool,
    },
    V2 {
        kind: InteractionResponseType,
        components: &'a [Component],
        flags: MessageFlags,
    },
}

#[derive(Serialize)]
struct Callback<'a> {
    #[serde(rename = "type")]
    kind: InteractionResponseType,
    data: CallbackData<'a>,
}

#[derive(Serialize)]
struct CallbackData<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    components: Option<&'a [Component]>,
    allowed_mentions: AllowedMentions,
    #[serde(skip_serializing_if = "Option::is_none")]
    flags: Option<MessageFlags>,
    attachments: Vec<PartialAttachment<'a>>,
}

impl InitialResponder<'_> {
    pub(super) async fn send_file_callback(
        self,
        response: FileCallback<'_>,
        uploads: &[Attachment],
        retained: Vec<Id<AttachmentMarker>>,
    ) -> Result<()> {
        let started_at = self.info.measures_timing().then(Instant::now);
        let files = AttachmentManager::new()
            .set_files(uploads.iter().collect())
            .set_ids(retained);
        let (kind, content, components, flags) = match response {
            FileCallback::Classic { content, ephemeral } => (
                InteractionResponseType::ChannelMessageWithSource,
                (!content.is_empty()).then_some(content),
                None,
                ephemeral.then_some(MessageFlags::EPHEMERAL),
            ),
            FileCallback::V2 {
                kind,
                components,
                flags,
            } => (kind, None, Some(components), Some(flags)),
        };
        let body = serde_json::to_vec(&Callback {
            kind,
            data: CallbackData {
                content,
                components,
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
