use twilight_model::http::attachment::Attachment;

use crate::{
    Result,
    slash::{Followups, send_followup_files},
    validation::AttachmentLimits,
};

use super::{
    ButtonContext, EntitySelectContext, MentionableSelectContext, ModalContext, SelectContext,
};

impl<'a> ButtonContext<'a> {
    /// 操作を受け付けた後、追加応答を送信・編集するためのハンドルを返します。
    ///
    /// # エラー
    /// 操作を受け付けていない場合は失敗します。
    pub fn followups(&self) -> Result<Followups<'a>> {
        self.response.require_followup()?;
        Ok(Followups::new(self.initial))
    }

    /// 操作を受け付けた後、本文とファイルをチャンネルに追加で送ります。
    /// ファイルだけの場合は空の本文を指定できます。
    ///
    /// # エラー
    /// 初回応答前、添付や本文が無効、上限超過、または Discord が受け付けない場合に失敗します。
    pub async fn followup_files(
        &self,
        content: &str,
        files: &[Attachment],
        limits: AttachmentLimits,
    ) -> Result<()> {
        self.send_files(content, files, limits, false).await
    }

    /// 操作を受け付けた後、本文とファイルを実行者だけに追加で送ります。
    ///
    /// # エラー
    /// [`Self::followup_files`] と同じ条件で失敗します。
    pub async fn followup_files_ephemeral(
        &self,
        content: &str,
        files: &[Attachment],
        limits: AttachmentLimits,
    ) -> Result<()> {
        self.send_files(content, files, limits, true).await
    }

    async fn send_files(
        &self,
        content: &str,
        files: &[Attachment],
        limits: AttachmentLimits,
        ephemeral: bool,
    ) -> Result<()> {
        self.response.require_followup()?;
        send_followup_files(self.initial, content, files, limits, ephemeral).await
    }
}

impl<'a> ModalContext<'a> {
    /// 返信を完成させるか元のメッセージの更新を受け付けた後、追加応答のハンドルを返します。
    ///
    /// # エラー
    /// 返信が未完成で、元のメッセージの更新も受け付けていない場合は失敗します。
    pub fn followups(&self) -> Result<Followups<'a>> {
        self.response.require_followup()?;
        Ok(Followups::new(self.initial))
    }

    /// モーダル送信へ返信した後、または元のメッセージの更新を受け付けた後に、本文とファイルをチャンネルへ送ります。
    /// ファイルだけの場合は空の本文を指定できます。
    ///
    /// # エラー
    /// 初回返信が未完成、添付や本文が無効、上限超過、または Discord が受け付けない場合に失敗します。
    pub async fn followup_files(
        &self,
        content: &str,
        files: &[Attachment],
        limits: AttachmentLimits,
    ) -> Result<()> {
        self.send_files(content, files, limits, false).await
    }

    /// モーダル送信へ返信した後、または元のメッセージの更新を受け付けた後に、本文とファイルを送信者だけに送ります。
    ///
    /// # エラー
    /// [`Self::followup_files`] と同じ条件で失敗します。
    pub async fn followup_files_ephemeral(
        &self,
        content: &str,
        files: &[Attachment],
        limits: AttachmentLimits,
    ) -> Result<()> {
        self.send_files(content, files, limits, true).await
    }

    async fn send_files(
        &self,
        content: &str,
        files: &[Attachment],
        limits: AttachmentLimits,
        ephemeral: bool,
    ) -> Result<()> {
        self.response.require_followup()?;
        send_followup_files(self.initial, content, files, limits, ephemeral).await
    }
}

macro_rules! select_followups {
    ($lifetime:lifetime) => {
        /// 操作を受け付けた後、追加応答を送信・編集するためのハンドルを返します。
        ///
        /// # エラー
        /// [`ButtonContext::followups`] と同じ条件で失敗します。
        pub fn followups(&self) -> Result<Followups<$lifetime>> {
            self.inner.followups()
        }

        /// セレクト操作を受け付けた後、本文とファイルをチャンネルに追加で送ります。
        /// ファイルだけの場合は空の本文を指定できます。
        ///
        /// # エラー
        /// [`ButtonContext::followup_files`] と同じ条件で失敗します。
        pub async fn followup_files(
            &self,
            content: &str,
            files: &[Attachment],
            limits: AttachmentLimits,
        ) -> Result<()> {
            self.inner.followup_files(content, files, limits).await
        }

        /// セレクト操作を受け付けた後、本文とファイルを実行者だけに追加で送ります。
        ///
        /// # エラー
        /// [`ButtonContext::followup_files_ephemeral`] と同じ条件で失敗します。
        pub async fn followup_files_ephemeral(
            &self,
            content: &str,
            files: &[Attachment],
            limits: AttachmentLimits,
        ) -> Result<()> {
            self.inner
                .followup_files_ephemeral(content, files, limits)
                .await
        }
    };
}

impl<'a> SelectContext<'a> {
    select_followups!('a);
}

impl<'a, M> EntitySelectContext<'a, M> {
    select_followups!('a);
}

impl<'a> MentionableSelectContext<'a> {
    select_followups!('a);
}
