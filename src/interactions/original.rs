use super::{
    ButtonContext, ButtonResponse, EntitySelectContext, MentionableSelectContext, SelectContext,
};

use twilight_model::channel::Message;

use crate::{
    ResponseStateError, Result,
    slash::ResponseState,
    ui::{ComponentsV2, UiMessage},
};

impl ButtonContext<'_> {
    /// 操作した本人だけに見える Components V2 の初回応答を送ります。
    ///
    /// # エラー
    /// 応答済み、無効なメッセージ、または Discord が受け付けない場合に失敗します。
    pub async fn reply_v2_ephemeral(&mut self, message: &ComponentsV2) -> Result<()> {
        self.response.require_pending()?;
        message.validate()?;
        self.initial.v2_reply(message, true).await?;
        self.response = ButtonResponse::OwnResponse(ResponseState::RepliedV2);
        Ok(())
    }

    /// [`Self::reply_ephemeral`] または [`Self::reply_v2_ephemeral`] で作った応答を取得します。
    /// 操作部品が付いている元の投稿は [`Self::message`] で参照できます。
    ///
    /// # エラー
    /// 返信がない、削除済み、または Discord が取得を受け付けない場合に失敗します。
    pub async fn original_response(&self) -> Result<Message> {
        let ButtonResponse::OwnResponse(state) = self.response else {
            return Err(ResponseStateError::OriginalResponseUnavailable.into());
        };
        state.original_response(self.initial).await
    }

    /// この操作への非公開返信の本文を更新します。既存の添付・埋め込み・操作部品は保持されます。
    ///
    /// # エラー
    /// 返信がない、Components V2 の返信、または Discord が編集を受け付けない場合に失敗します。
    pub async fn edit_original_response(&mut self, content: &str) -> Result<()> {
        self.response
            .own_response_mut()?
            .edit_original_response(self.initial, content)
            .await
    }

    /// この操作への非公開返信の本文・埋め込み・従来の操作部品を置き換えます。
    ///
    /// # エラー
    /// 返信がない、形式が合わない、メッセージが無効、または Discord が編集を受け付けない場合に失敗します。
    pub async fn edit_original_ui(&mut self, message: &UiMessage) -> Result<()> {
        self.response
            .own_response_mut()?
            .edit_original_ui(self.initial, message)
            .await
    }

    /// この操作への非公開 Components V2 返信の部品を置き換えます。添付は保持します。
    ///
    /// # エラー
    /// 返信がない、形式が合わない、メッセージが無効、または Discord が編集を受け付けない場合に失敗します。
    pub async fn edit_original_v2(&mut self, message: &ComponentsV2) -> Result<()> {
        self.response
            .own_response_mut()?
            .edit_original_v2(self.initial, message)
            .await
    }

    /// この操作への非公開返信を削除します。
    ///
    /// # エラー
    /// 返信がない、削除済み、または Discord が削除を受け付けない場合に失敗します。
    /// [`Self::defer_update`] や [`Self::acknowledge`] だけを行った場合、エラーになり元の投稿は保持されます。
    pub async fn delete_original_response(&mut self) -> Result<()> {
        self.response
            .own_response_mut()?
            .delete_original_response(self.initial)
            .await
    }
}

impl ButtonResponse {
    fn own_response_mut(&mut self) -> Result<&mut ResponseState> {
        match self {
            Self::OwnResponse(state) => Ok(state),
            _ => Err(ResponseStateError::OriginalResponseUnavailable.into()),
        }
    }
}

macro_rules! select_original_response_methods {
    () => {
        /// 操作した本人だけに見える Components V2 の初回応答を送ります。
        ///
        /// # エラー
        /// [`ButtonContext::reply_v2_ephemeral`] と同じ条件で失敗します。
        pub async fn reply_v2_ephemeral(&mut self, message: &ComponentsV2) -> Result<()> {
            self.inner.reply_v2_ephemeral(message).await
        }

        /// この操作への非公開返信を取得します。セレクトが付いている投稿は [`Self::message`] で参照できます。
        ///
        /// # エラー
        /// [`ButtonContext::original_response`] と同じ条件で失敗します。
        pub async fn original_response(&self) -> Result<Message> {
            self.inner.original_response().await
        }

        /// この操作への非公開返信の本文を更新します。
        ///
        /// # エラー
        /// [`ButtonContext::edit_original_response`] と同じ条件で失敗します。
        pub async fn edit_original_response(&mut self, content: &str) -> Result<()> {
            self.inner.edit_original_response(content).await
        }

        /// この操作への非公開返信の本文・埋め込み・従来の操作部品を置き換えます。
        ///
        /// # エラー
        /// [`ButtonContext::edit_original_ui`] と同じ条件で失敗します。
        pub async fn edit_original_ui(&mut self, message: &UiMessage) -> Result<()> {
            self.inner.edit_original_ui(message).await
        }

        /// この操作への非公開 Components V2 返信の部品を置き換えます。
        ///
        /// # エラー
        /// [`ButtonContext::edit_original_v2`] と同じ条件で失敗します。
        pub async fn edit_original_v2(&mut self, message: &ComponentsV2) -> Result<()> {
            self.inner.edit_original_v2(message).await
        }

        /// この操作への非公開返信を削除します。
        ///
        /// # エラー
        /// [`ButtonContext::delete_original_response`] と同じ条件で失敗します。
        pub async fn delete_original_response(&mut self) -> Result<()> {
            self.inner.delete_original_response().await
        }
    };
}

impl SelectContext<'_> {
    select_original_response_methods!();
}

impl<M> EntitySelectContext<'_, M> {
    select_original_response_methods!();
}

impl MentionableSelectContext<'_> {
    select_original_response_methods!();
}
