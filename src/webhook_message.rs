use serde::Serialize;
use twilight_http::{
    Client,
    request::{
        Request, TryIntoRequest,
        attachment::{AttachmentManager, PartialAttachment},
    },
    response::marker::EmptyBody,
    routing::Route,
};
use twilight_model::{
    channel::{
        Message,
        message::{AllowedMentions, Embed},
    },
    http::attachment::Attachment,
    id::{
        Id,
        marker::{AttachmentMarker, ChannelMarker, MessageMarker, WebhookMarker},
    },
};

use crate::{
    Error, WebhookSendError,
    validation::{self, AttachmentLimits},
    webhook_post::{read_webhook_message, request_with_webhook_token, validate_token},
};

/// Webhook 投稿を取得・編集・削除するための ID です。トークンは操作時に渡します。
///
/// クライアントの既定ヘッダーに `Authorization` を設定しないでください。
/// ログ購読では、トークンを含むパスが出る場合がある `twilight_http_ratelimiting` を対象外にしてください。
#[derive(Clone, Copy, Debug)]
#[must_use]
pub struct WebhookMessageRef {
    webhook: Id<WebhookMarker>,
    message: Id<MessageMarker>,
    thread: Option<Id<ChannelMarker>>,
}

impl WebhookMessageRef {
    /// Webhook ID と投稿のメッセージ ID を指定します。
    pub const fn new(webhook: Id<WebhookMarker>, message: Id<MessageMarker>) -> Self {
        Self {
            webhook,
            message,
            thread: None,
        }
    }

    /// 投稿先がスレッドの場合は、そのスレッド ID を指定します。
    pub const fn in_thread(mut self, thread: Id<ChannelMarker>) -> Self {
        self.thread = Some(thread);
        self
    }

    /// 投稿を取得します。
    ///
    /// # エラー
    /// トークンが無効、Discord が拒否した、または指定した投稿の応答を確認できない場合に失敗します。
    pub async fn fetch(self, http: &Client, token: &str) -> Result<Message, WebhookSendError> {
        validate_token(token)?;
        let mut request = http.webhook_message(self.webhook, token, self.message);
        if let Some(thread) = self.thread {
            request = request.thread_id(thread);
        }
        let request = request
            .try_into_request()
            .map_err(WebhookSendError::from_http)?;
        self.read(http, request).await
    }

    /// 指定した項目を編集し、編集後の投稿を返します。
    ///
    /// # エラー
    /// 編集内容やトークンが無効、Discord が拒否した、または指定した投稿の応答を確認できない場合に失敗します。
    pub async fn edit(
        self,
        http: &Client,
        token: &str,
        edit: &WebhookMessageEdit<'_>,
    ) -> Result<Message, WebhookSendError> {
        validate_token(token)?;
        let request = edit.request(self, token)?;
        self.read(http, request).await
    }

    /// 投稿を削除します。
    ///
    /// # エラー
    /// トークンが無効、または投稿がすでに存在しないなど Discord が拒否した場合に失敗します。
    pub async fn delete(self, http: &Client, token: &str) -> Result<(), WebhookSendError> {
        validate_token(token)?;
        let mut request = http.delete_webhook_message(self.webhook, token, self.message);
        if let Some(thread) = self.thread {
            request = request.thread_id(thread);
        }
        let request = request
            .try_into_request()
            .map_err(WebhookSendError::from_http)?;
        request_with_webhook_token::<EmptyBody>(http, request).await?;
        Ok(())
    }

    async fn read(self, http: &Client, request: Request) -> Result<Message, WebhookSendError> {
        let message = read_webhook_message(http, request).await?;
        if message.webhook_id != Some(self.webhook)
            || message.id != self.message
            || self
                .thread
                .is_some_and(|thread| message.channel_id != thread)
        {
            return Err(WebhookSendError::InvalidResponse);
        }
        Ok(message)
    }
}

/// Webhook 投稿の本文、Embed、添付を部分更新します。
#[derive(Default)]
#[must_use]
pub struct WebhookMessageEdit<'a> {
    content: Option<Option<&'a str>>,
    embeds: Option<&'a [Embed]>,
    files: Option<Files<'a>>,
}

struct Files<'a> {
    retained: &'a [Id<AttachmentMarker>],
    uploads: &'a [Attachment],
    limits: AttachmentLimits,
}

impl<'a> WebhookMessageEdit<'a> {
    /// 変更内容を作ります。最低 1 項目を指定してから編集します。
    pub fn new() -> Self {
        Self::default()
    }

    /// 本文を置き換えます。 `None` は本文を削除します。
    pub const fn content(mut self, value: Option<&'a str>) -> Self {
        self.content = Some(value);
        self
    }

    /// Embed を置き換えます。空のスライスはすべて削除します。
    pub const fn embeds(mut self, embeds: &'a [Embed]) -> Self {
        self.embeds = Some(embeds);
        self
    }

    /// 残す添付の ID と追加するファイルを指定します。両方が空なら添付をすべて削除します。
    pub const fn attachments(
        mut self,
        retained: &'a [Id<AttachmentMarker>],
        uploads: &'a [Attachment],
        limits: AttachmentLimits,
    ) -> Self {
        self.files = Some(Files {
            retained,
            uploads,
            limits,
        });
        self
    }

    fn request(&self, target: WebhookMessageRef, token: &str) -> Result<Request, WebhookSendError> {
        if self.content.is_none() && self.embeds.is_none() && self.files.is_none() {
            return Err(Error::InvalidUi("Webhook 投稿の編集項目を指定してください。").into());
        }
        if let Some(Some(text)) = self.content {
            validation::message_content(text)?;
        }
        if let Some(embeds) = self.embeds {
            validation::embeds(embeds)?;
        }
        let files = self.files.as_ref().map(Files::manager).transpose()?;
        let route = Route::UpdateWebhookMessage {
            webhook_id: target.webhook.get(),
            message_id: target.message.get(),
            thread_id: target.thread.map(Id::get),
            token,
        };
        let body = EditPayload {
            content: self.content,
            embeds: self.embeds,
            attachments: self
                .files
                .as_ref()
                .zip(files.as_ref())
                .map(|(edit, files)| {
                    edit.retained
                        .iter()
                        .copied()
                        .map(|id| FileEntry::Retained { id })
                        .chain(
                            files
                                .get_partial_attachments()
                                .into_iter()
                                .map(FileEntry::Upload),
                        )
                        .collect()
                }),
            allowed_mentions: AllowedMentions::default(),
        };
        let builder = Request::builder(&route).use_authorization_token(false);
        let request = if let Some(files) = files.filter(|files| !files.is_empty()) {
            let json = serde_json::to_vec(&body).map_err(|_| WebhookSendError::Request)?;
            builder.form(files.build_form(&json))
        } else {
            builder.json(&body)
        }
        .build()
        .map_err(WebhookSendError::from_http)?;
        let size = request
            .form()
            .map_or_else(|| request.body().map_or(0, <[u8]>::len), |form| form.len());
        if size as u64 > validation::CREATE_MESSAGE_REQUEST_MAX_BYTES {
            return Err(
                Error::InvalidUi("Webhook 編集リクエストが 25 MiB を超えています。").into(),
            );
        }
        Ok(request)
    }
}

impl<'a> Files<'a> {
    fn manager(&self) -> crate::Result<AttachmentManager<'a>> {
        self.limits.count(self.retained.len(), self.uploads.len())?;
        self.limits.validate_uploads(self.uploads, None)?;
        validation::attachment_metadata(self.uploads)?;
        validation::channel_message_upload_size(self.uploads)?;
        for (index, id) in self.retained.iter().enumerate() {
            if self
                .retained
                .iter()
                .take(index)
                .any(|earlier| earlier == id)
                || self.uploads.iter().any(|new| new.id == id.get())
            {
                return Err(Error::InvalidUi("Webhook 編集の添付 ID は重複できません。"));
            }
        }
        Ok(AttachmentManager::new().set_files(self.uploads.iter().collect()))
    }
}

#[derive(Serialize)]
struct EditPayload<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<Option<&'a str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    embeds: Option<&'a [Embed]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attachments: Option<Vec<FileEntry<'a>>>,
    allowed_mentions: AllowedMentions,
}

#[derive(Serialize)]
#[serde(untagged)]
enum FileEntry<'a> {
    Retained { id: Id<AttachmentMarker> },
    Upload(PartialAttachment<'a>),
}

#[cfg(test)]
mod tests;
