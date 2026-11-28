use std::fmt;

use twilight_http::{
    Client, Response,
    api_error::ApiError,
    client::InteractionClient,
    error::{Error as HttpError, ErrorType},
};
use twilight_model::{
    application::interaction::Interaction,
    channel::{
        Message,
        message::{AllowedMentions, MessageFlags},
    },
    http::attachment::Attachment,
    id::{Id, marker::MessageMarker},
};

use crate::{
    Error, Result,
    ui::{ComponentsV2, UiMessage},
    validation::{self, AttachmentLimits},
};

use super::{InitialResponder, SlashContext, v2_attachments::retained_ids};

#[cfg(test)]
#[path = "followup_message_tests.rs"]
mod tests;

impl<'a> SlashContext<'a> {
    /// 初回応答を完成させた後、追加応答を送信・編集するためのハンドルを返します。
    ///
    /// # エラー
    /// 初回応答が未完成なら [`crate::ResponseStateError::FollowupBeforeResponse`] を返します。
    pub fn followups(&self) -> Result<Followups<'a>> {
        self.response_state.require_followup()?;
        Ok(Followups::new(self.initial))
    }
}

/// 一つの Interaction に対して、メッセージを追加するためのハンドルです。
///
/// Discord の Interaction トークンの有効期間内に使います。トークンは公開しません。
pub struct Followups<'a> {
    http: &'a Client,
    interaction: &'a Interaction,
}

impl fmt::Debug for Followups<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Followups")
            .field("interaction_id", &self.interaction.id)
            .finish_non_exhaustive()
    }
}

impl<'a> Followups<'a> {
    pub(crate) const fn new(initial: InitialResponder<'a>) -> Self {
        Self {
            http: initial.http,
            interaction: initial.interaction,
        }
    }

    fn client(&self) -> InteractionClient<'_> {
        self.http.interaction(self.interaction.application_id)
    }

    /// 本文で追加応答し、Discord が返したメッセージを追跡します。
    /// `ephemeral` が真なら実行者だけに表示されます。
    ///
    /// # エラー
    /// 本文が無効、Discord が受け付けない、または返されたメッセージを読めない場合に失敗します。
    pub async fn send(&self, content: &str, ephemeral: bool) -> Result<FollowupMessage<'a>> {
        self.send_files(content, &[], AttachmentLimits::default(), ephemeral)
            .await
    }

    /// 本文・埋め込み・従来のコンポーネントを含む追加応答を送ります。
    /// `ephemeral` が真なら実行者だけに表示されます。
    ///
    /// # エラー
    /// メッセージが無効、Discord が受け付けない、または返されたメッセージを読めない場合に失敗します。
    pub async fn send_ui(
        &self,
        message: &UiMessage,
        ephemeral: bool,
    ) -> Result<FollowupMessage<'a>> {
        message.validate()?;
        let client = self.client();
        let mentions = AllowedMentions::default();
        let mut request = client
            .create_followup(&self.interaction.token)
            .allowed_mentions(Some(&mentions))
            .flags(visibility(ephemeral));
        if let Some(content) = &message.content {
            request = request.content(content);
        }
        if !message.embeds.is_empty() {
            request = request.embeds(&message.embeds);
        }
        if !message.components.is_empty() {
            request = request.components(&message.components);
        }
        self.track(request.await).await
    }

    /// Components V2 の追加応答を送ります。指定した新しいファイルも送信します。
    /// `ephemeral` が真なら実行者だけに表示されます。
    ///
    /// # エラー
    /// 部品や添付が無効、Discord が受け付けない、または返されたメッセージを読めない場合に失敗します。
    pub async fn send_v2(
        &self,
        message: &ComponentsV2,
        ephemeral: bool,
    ) -> Result<FollowupMessage<'a>> {
        message.validate()?;
        message.validate_attachment_sizes(self.interaction.attachment_size_limit)?;
        message.validate_attachment_references(&[])?;
        let client = self.client();
        let mentions = AllowedMentions::default();
        let mut request = client
            .create_followup(&self.interaction.token)
            .allowed_mentions(Some(&mentions))
            .flags(MessageFlags::IS_COMPONENTS_V2 | visibility(ephemeral))
            .components(&message.components);
        if !message.attachments.is_empty() {
            request = request.attachments(&message.attachments);
        }
        self.track(request.await).await
    }

    /// 本文とファイルを追加で送ります。添付だけを送る場合、本文を空にできます。
    /// `ephemeral` が真なら実行者だけに表示されます。
    ///
    /// # エラー
    /// 本文や添付が無効、上限を超えた、Discord が拒否した、または応答を読めない場合に失敗します。
    pub async fn send_files(
        &self,
        content: &str,
        files: &[Attachment],
        limits: AttachmentLimits,
        ephemeral: bool,
    ) -> Result<FollowupMessage<'a>> {
        validation::reply_files(
            content,
            files,
            limits,
            Some(self.interaction.attachment_size_limit),
        )?;
        let client = self.client();
        let mentions = AllowedMentions::default();
        let mut request = client
            .create_followup(&self.interaction.token)
            .allowed_mentions(Some(&mentions))
            .flags(visibility(ephemeral));
        if !content.is_empty() {
            request = request.content(content);
        }
        if !files.is_empty() {
            request = request.attachments(files);
        }
        self.track(request.await).await
    }

    async fn track(
        &self,
        response: std::result::Result<Response<Message>, HttpError>,
    ) -> Result<FollowupMessage<'a>> {
        Ok(FollowupMessage {
            handle: Self {
                http: self.http,
                interaction: self.interaction,
            },
            message: decode_message(response).await?,
        })
    }
}

/// Discord が作成した追加応答と、その応答だけを編集・削除できるハンドルです。
pub struct FollowupMessage<'a> {
    handle: Followups<'a>,
    message: Message,
}

impl fmt::Debug for FollowupMessage<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FollowupMessage")
            .field("message_id", &self.message.id)
            .finish_non_exhaustive()
    }
}

impl FollowupMessage<'_> {
    /// 追加応答の Discord メッセージ ID です。
    pub const fn id(&self) -> Id<MessageMarker> {
        self.message.id
    }

    /// 作成時または直近の編集で Discord が返したメッセージを借ります。
    pub const fn message(&self) -> &Message {
        &self.message
    }

    /// 追加応答の本文を更新します。既存の埋め込みや添付、公開範囲は保持されます。
    ///
    /// # エラー
    /// V2 メッセージ、本文が無効、Discord が拒否した、または応答を読めない場合に失敗します。
    pub async fn edit(&mut self, content: &str) -> Result<()> {
        self.require_format(false)?;
        validation::message_content(content)?;
        let message = {
            let client = self.handle.client();
            decode_message(
                client
                    .update_followup(&self.handle.interaction.token, self.id())
                    .content(Some(content))
                    .allowed_mentions(Some(&AllowedMentions::default()))
                    .await,
            )
            .await?
        };
        self.accept_edit(message)
    }

    /// 追加応答の本文、埋め込み、従来のコンポーネントを置き換えます。添付と公開範囲は保持します。
    ///
    /// # エラー
    /// V2 メッセージ、編集内容が無効、Discord が拒否した、または応答を読めない場合に失敗します。
    pub async fn edit_ui(&mut self, message: &UiMessage) -> Result<()> {
        self.require_format(false)?;
        message.validate()?;
        let edited = {
            let client = self.handle.client();
            decode_message(
                client
                    .update_followup(&self.handle.interaction.token, self.id())
                    .content(message.content.as_deref())
                    .embeds(Some(&message.embeds))
                    .components(Some(&message.components))
                    .allowed_mentions(Some(&AllowedMentions::default()))
                    .await,
            )
            .await?
        };
        self.accept_edit(edited)
    }

    /// 既存の Components V2 追加応答を編集します。新しく指定したファイルを追加し、公開範囲と既存の添付を保持します。
    /// 添付を追加する場合は、保持する添付一覧を Discord から取得します。
    ///
    /// # エラー
    /// V2 以外、部品や添付が無効、Discord が拒否した、または応答を読めない場合に失敗します。
    pub async fn edit_v2(&mut self, message: &ComponentsV2) -> Result<()> {
        self.require_format(true)?;
        message.validate()?;
        message.validate_attachment_sizes(self.handle.interaction.attachment_size_limit)?;
        let edited = {
            let client = self.handle.client();
            let token = &self.handle.interaction.token;
            let id = self.id();
            let current;
            let retained = if message.needs_existing_attachments() {
                current = decode_message(client.followup(token, id).await).await?;
                if current.id != id {
                    return Err(Error::handler(FollowupFailure::UnexpectedMessage));
                }
                &current.attachments
            } else {
                &self.message.attachments
            };
            message.validate_attachment_references(retained)?;
            let mentions = AllowedMentions::default();
            let mut request = client
                .update_followup(token, id)
                .components(Some(&message.components))
                .allowed_mentions(Some(&mentions));
            let ids;
            if !message.attachments.is_empty() {
                ids = retained_ids(&message.attachments, retained)?;
                request = request
                    .attachments(&message.attachments)
                    .keep_attachment_ids(&ids);
            }
            decode_message(request.await).await?
        };
        self.accept_edit(edited)
    }

    /// この追加応答を Discord から削除します。
    ///
    /// # エラー
    /// トークンが期限切れ、すでに削除された、または Discord が削除を受け付けない場合に失敗します。
    pub async fn delete(&self) -> Result<()> {
        self.handle
            .client()
            .delete_followup(&self.handle.interaction.token, self.id())
            .await
            .map_err(safe_http_error)?;
        Ok(())
    }

    fn accept_edit(&mut self, message: Message) -> Result<()> {
        if message.id != self.id() {
            return Err(Error::handler(FollowupFailure::UnexpectedMessage));
        }
        self.message = message;
        Ok(())
    }

    fn require_format(&self, v2: bool) -> Result<()> {
        let current = self
            .message
            .flags
            .is_some_and(|flags| flags.contains(MessageFlags::IS_COMPONENTS_V2));
        if current != v2 {
            return Err(Error::InvalidUi(
                "追加応答の形式（従来形式または Components V2）に合う編集方法を使ってください。",
            ));
        }
        Ok(())
    }
}

fn visibility(ephemeral: bool) -> MessageFlags {
    if ephemeral {
        MessageFlags::EPHEMERAL
    } else {
        MessageFlags::empty()
    }
}

async fn decode_message(
    response: std::result::Result<Response<Message>, HttpError>,
) -> Result<Message> {
    response
        .map_err(safe_http_error)?
        .model()
        .await
        .map_err(|_| Error::handler(FollowupFailure::UnreadableMessage))
}

// Twilight の本文やエラー元には外部から受け取った文字列が入る。トークン付き経路では数値だけを残す。
#[derive(Debug, thiserror::Error)]
enum FollowupFailure {
    #[error("Discord の追加応答の通信に失敗しました")]
    Request,
    #[error("Discord の追加応答が HTTP {status} で拒否されました（コード {code:?}）")]
    Rejected { status: u16, code: Option<u64> },
    #[error("Discord が返した追加応答を読み取れませんでした")]
    UnreadableMessage,
    #[error("Discord が返した追加応答のメッセージ ID が一致しません")]
    UnexpectedMessage,
}

fn safe_http_error(error: HttpError) -> Error {
    let failure = match error.kind() {
        ErrorType::Response { status, error, .. } => FollowupFailure::Rejected {
            status: status.get(),
            code: match error {
                ApiError::General(error) => Some(error.code),
                _ => None,
            },
        },
        ErrorType::Unauthorized => FollowupFailure::Rejected {
            status: 401,
            code: None,
        },
        _ => FollowupFailure::Request,
    };
    Error::handler(failure)
}
