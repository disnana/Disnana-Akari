use twilight_model::channel::Message;

use crate::{
    ResponseStateError, Result,
    ui::{ComponentsV2, UiMessage},
};

use super::{
    InitialResponder, ResponseState, SlashContext, edit_text_response, edit_ui_response,
    edit_v2_response,
};

impl SlashContext<'_> {
    /// この操作への元の応答を Discord から取得します。非公開の応答も取得できます。
    ///
    /// # エラー
    /// 元の応答がない、すでに削除した、または Discord が取得を受け付けない場合に失敗します。
    pub async fn original_response(&self) -> Result<Message> {
        self.response_state.original_response(self.initial).await
    }

    /// 元の応答の本文を更新します。添付、埋め込み、従来のコンポーネントは保持されます。
    /// メンション通知は抑止します。遅延応答の直後に呼ぶと、処理中の表示を完成させます。
    ///
    /// # エラー
    /// 元の応答がない、Components V2 形式で送っている、または Discord が編集を受け付けない場合に失敗します。
    pub async fn edit_original_response(&mut self, content: &str) -> Result<()> {
        self.response_state
            .edit_original_response(self.initial, content)
            .await
    }

    /// 元の応答の本文・埋め込み・従来のコンポーネントを置き換えます。
    /// 遅延応答をこの形式で完成させることもできます。公開範囲と添付は維持されます。
    ///
    /// # エラー
    /// 元の応答や形式が対応しない、メッセージが無効、または Discord が編集を受け付けない場合に失敗します。
    pub async fn edit_original_ui(&mut self, message: &UiMessage) -> Result<()> {
        self.response_state
            .edit_original_ui(self.initial, message)
            .await
    }

    /// 元の Components V2 応答の部品を置き換えます。公開範囲と既存の添付は維持します。
    /// 新しい添付を指定した場合は、元の応答の添付一覧を取得してから追加します。
    /// 遅延応答から直接 Components V2 のメッセージを完成させることもできます。
    ///
    /// # エラー
    /// 元の応答や形式が対応しない、メッセージが無効、または Discord が編集を受け付けない場合に失敗します。
    pub async fn edit_original_v2(&mut self, message: &ComponentsV2) -> Result<()> {
        self.response_state
            .edit_original_v2(self.initial, message)
            .await
    }

    /// 元の応答メッセージを削除します。遅延応答だけを送った場合は処理中の表示を取り除きます。
    ///
    /// # エラー
    /// 元の応答がない、すでに削除した、または Discord が削除を受け付けない場合に失敗します。
    pub async fn delete_original_response(&mut self) -> Result<()> {
        self.response_state
            .delete_original_response(self.initial)
            .await
    }
}

impl ResponseState {
    pub(crate) async fn original_response(self, initial: InitialResponder<'_>) -> Result<Message> {
        self.require_original()?;
        let interaction = initial.interaction;
        Ok(initial
            .http
            .interaction(interaction.application_id)
            .response(&interaction.token)
            .await?
            .model()
            .await?)
    }

    pub(crate) async fn edit_original_response(
        &mut self,
        initial: InitialResponder<'_>,
        content: &str,
    ) -> Result<()> {
        self.require_original_format(false)?;
        edit_text_response(initial.http, initial.interaction, content).await?;
        *self = Self::Replied;
        Ok(())
    }

    pub(crate) async fn edit_original_ui(
        &mut self,
        initial: InitialResponder<'_>,
        message: &UiMessage,
    ) -> Result<()> {
        self.require_original_format(false)?;
        message.validate()?;
        edit_ui_response(initial.http, initial.interaction, message).await?;
        *self = Self::Replied;
        Ok(())
    }

    pub(crate) async fn edit_original_v2(
        &mut self,
        initial: InitialResponder<'_>,
        message: &ComponentsV2,
    ) -> Result<()> {
        self.require_original_format(true)?;
        message.validate()?;
        message.validate_attachment_sizes(initial.interaction.attachment_size_limit)?;
        let previous = if message.needs_existing_attachments() && *self == Self::RepliedV2 {
            Some(self.original_response(initial).await?)
        } else {
            None
        };
        let retained = previous
            .as_ref()
            .map(|source| source.attachments.as_slice())
            .or(matches!(self, Self::Deferred { .. }).then_some(&[][..]));
        edit_v2_response(initial.http, initial.interaction, message, retained).await?;
        *self = Self::RepliedV2;
        Ok(())
    }

    pub(crate) async fn delete_original_response(
        &mut self,
        initial: InitialResponder<'_>,
    ) -> Result<()> {
        self.require_original()?;
        let completed = matches!(self, Self::Replied | Self::RepliedV2);
        let interaction = initial.interaction;
        initial
            .http
            .interaction(interaction.application_id)
            .delete_response(&interaction.token)
            .await?;
        *self = Self::OriginalDeleted { completed };
        Ok(())
    }

    fn require_original(self) -> Result<()> {
        match self {
            Self::Deferred { .. } | Self::Replied | Self::RepliedV2 => Ok(()),
            Self::Pending | Self::OriginalDeleted { .. } | Self::ModalOpened => {
                Err(ResponseStateError::OriginalResponseUnavailable.into())
            }
        }
    }

    fn require_original_format(self, v2: bool) -> Result<()> {
        self.require_original()?;
        match self {
            Self::RepliedV2 if !v2 => {
                Err(ResponseStateError::OriginalResponseFormatMismatch.into())
            }
            Self::Replied if v2 => Err(ResponseStateError::OriginalResponseFormatMismatch.into()),
            _ => Ok(()),
        }
    }
}
