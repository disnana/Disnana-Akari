use serde::Serialize;
use tracing::{dispatcher::Dispatch, instrument::WithSubscriber};
use twilight_http::{
    Client,
    api_error::ApiError,
    error::ErrorType,
    request::{
        Request,
        attachment::{AttachmentManager, PartialAttachment},
    },
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
        marker::{ChannelMarker, TagMarker, WebhookMarker},
    },
};

use crate::{
    Error, PollDraft,
    forum_post::validate_forum_tags,
    poll::PollPayload,
    validation::{self, AttachmentLimits},
};

/// Webhook で投稿する本文、Embed、添付、投票と投稿先を組み立てます。
///
/// メンション通知を抑止し、Discord が投稿を確認するまで待ちます。
#[derive(Default)]
#[must_use]
pub struct WebhookPost<'a> {
    content: Option<&'a str>,
    embeds: &'a [Embed],
    attachments: &'a [Attachment],
    attachment_limits: AttachmentLimits,
    poll: Option<PollDraft<'a>>,
    destination: Destination<'a>,
}

#[derive(Clone, Copy, Default)]
enum Destination<'a> {
    #[default]
    Channel,
    Thread(Id<ChannelMarker>),
    NewThread {
        title: &'a str,
        tags: &'a [Id<TagMarker>],
    },
}

impl<'a> WebhookPost<'a> {
    /// Webhook のチャンネルに投稿します。本文・Embed・添付・投票のいずれかを設定してください。
    pub fn new() -> Self {
        Self::default()
    }

    /// 本文を設定します。
    pub const fn content(mut self, text: &'a str) -> Self {
        self.content = Some(text);
        self
    }

    /// Embed を設定します。
    pub const fn embeds(mut self, embeds: &'a [Embed]) -> Self {
        self.embeds = embeds;
        self
    }

    /// 添付ファイルを設定します。
    pub const fn attachments(mut self, attachments: &'a [Attachment]) -> Self {
        self.attachments = attachments;
        self
    }

    /// アプリが採用する、添付ファイルの件数とバイト数の上限を設定します。
    pub const fn attachment_limits(mut self, limits: AttachmentLimits) -> Self {
        self.attachment_limits = limits;
        self
    }

    /// 検証済みの投票を付けます。
    pub const fn poll(mut self, poll: PollDraft<'a>) -> Self {
        self.poll = Some(poll);
        self
    }

    /// Webhook のチャンネルにある既存のスレッドへ投稿先を切り替えます。
    pub const fn in_thread(mut self, thread: Id<ChannelMarker>) -> Self {
        self.destination = Destination::Thread(thread);
        self
    }

    /// Forum または Media チャンネルにスレッドを作り、最初のメッセージを投稿します。
    ///
    /// タグは投稿先で使用できる ID を最大 5 件まで指定します。
    pub const fn new_thread(mut self, title: &'a str, tags: &'a [Id<TagMarker>]) -> Self {
        self.destination = Destination::NewThread { title, tags };
        self
    }

    /// Webhook の ID と専用トークンを使い、投稿されたメッセージを受け取ります。
    /// クライアントの既定ヘッダーに `Authorization` を設定しないでください。
    ///
    /// # エラー
    ///
    /// 入力が無効、Discord が拒否した、応答を読み取れない、または応答の Webhook ID や
    /// 指定したスレッドが一致しない場合に失敗します。応答の失敗時も投稿済みの場合があります。
    /// Discord エラーの本文とリクエストパスを返さず、リクエスト自身の Twilight スパンを抑止します。
    /// 別タスクの Twilight レートリミッターにはパスが含まれる場合があります。ログ購読では
    /// `twilight_http_ratelimiting` を対象外にしてください。
    pub async fn send(
        &self,
        http: &Client,
        webhook: Id<WebhookMarker>,
        token: &str,
    ) -> Result<Message, WebhookSendError> {
        let request = self.request(webhook, token)?;
        let message = read_webhook_message(http, request).await?;
        if message.webhook_id != Some(webhook)
            || matches!(self.destination, Destination::Thread(thread) if message.channel_id != thread)
            || (self.poll.is_some() && message.poll.is_none())
        {
            return Err(WebhookSendError::InvalidResponse);
        }
        Ok(message)
    }

    fn request(
        &self,
        webhook: Id<WebhookMarker>,
        token: &str,
    ) -> Result<Request, WebhookSendError> {
        validate_token(token)?;
        self.validate()?;
        let (thread_id, thread_name, applied_tags) = match self.destination {
            Destination::Channel => (None, None, &[][..]),
            Destination::Thread(id) => (Some(id.get()), None, &[][..]),
            Destination::NewThread { title, tags } => (None, Some(title), tags),
        };
        let route = Route::ExecuteWebhook {
            webhook_id: webhook.get(),
            token,
            wait: Some(true),
            with_components: None,
            thread_id,
        };
        let files = AttachmentManager::new().set_files(self.attachments.iter().collect());
        let payload = Payload {
            content: self.content,
            embeds: self.embeds,
            attachments: files.get_partial_attachments(),
            allowed_mentions: AllowedMentions::default(),
            poll: self.poll.as_ref().map(PollDraft::poll_payload),
            thread_name,
            applied_tags,
        };
        let builder = Request::builder(&route).use_authorization_token(false);
        let request = if self.attachments.is_empty() {
            builder.json(&payload)
        } else {
            let body = serde_json::to_vec(&payload).map_err(|_| WebhookSendError::Request)?;
            builder.form(files.build_form(&body))
        }
        .build()
        .map_err(WebhookSendError::from_http)?;
        let size = request
            .form()
            .map_or_else(|| request.body().map_or(0, <[u8]>::len), |form| form.len());
        if size as u64 > validation::CREATE_MESSAGE_REQUEST_MAX_BYTES {
            return Err(
                Error::InvalidUi("Webhook 投稿リクエストが 25 MiB を超えています。").into(),
            );
        }
        Ok(request)
    }

    fn validate(&self) -> crate::Result<()> {
        if let Some(content) = self.content {
            validation::message_content(content)?;
        }
        if self.content.is_none_or(|text| text.trim().is_empty())
            && self.embeds.is_empty()
            && self.attachments.is_empty()
            && self.poll.is_none()
        {
            return Err(Error::InvalidUi(
                "Webhook 投稿には本文、Embed、添付、投票のいずれかが必要です。",
            ));
        }
        validation::embeds(self.embeds)?;
        self.attachment_limits
            .validate_uploads(self.attachments, None)?;
        validation::attachment_metadata(self.attachments)?;
        validation::channel_message_upload_size(self.attachments)?;
        if let Destination::NewThread { title, tags } = self.destination {
            twilight_validate::channel::name(title)?;
            if title.trim().is_empty() {
                return Err(Error::InvalidUi("スレッド名を指定してください。"));
            }
            validate_forum_tags(tags)?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct Payload<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<&'a str>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    embeds: &'a [Embed],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    attachments: Vec<PartialAttachment<'a>>,
    allowed_mentions: AllowedMentions,
    #[serde(skip_serializing_if = "Option::is_none")]
    poll: Option<PollPayload<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_name: Option<&'a str>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    applied_tags: &'a [Id<TagMarker>],
}

/// Webhook 投稿の失敗理由です。トークンや Discord の応答本文を保持しません。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WebhookSendError {
    /// トークンが空、またはパスに使用できない文字を含みます。
    #[error("Webhook トークンの形式が無効です")]
    InvalidToken,
    /// 投稿データが送信前の検証に失敗しました。
    #[error(transparent)]
    InvalidPost(#[from] Error),
    /// Discord へのリクエストを作成・送信できませんでした。
    #[error("Discord の Webhook API と通信できませんでした")]
    Request,
    /// Discord がリクエストを拒否しました。
    #[error("Discord が Webhook の操作を拒否しました（HTTP {status}、コード {code:?}）")]
    Rejected {
        /// HTTP のステータスコードです。
        status: u16,
        /// Discord が返したエラーコードです。
        code: Option<u64>,
    },
    /// Discord の応答を読めないか、指定と一致しませんでした。
    #[error("Discord が返した Webhook 投稿を確認できませんでした")]
    InvalidResponse,
}

impl WebhookSendError {
    pub(crate) fn from_http(error: twilight_http::Error) -> Self {
        match error.kind() {
            ErrorType::Response { status, error, .. } => Self::Rejected {
                status: status.get(),
                code: match error {
                    ApiError::General(error) => Some(error.code),
                    _ => None,
                },
            },
            ErrorType::Unauthorized => Self::Rejected {
                status: 401,
                code: None,
            },
            _ => Self::Request,
        }
    }
}

pub(crate) fn validate_token(token: &str) -> Result<(), WebhookSendError> {
    if token.is_empty()
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(WebhookSendError::InvalidToken);
    }
    Ok(())
}

pub(crate) async fn request_with_webhook_token<T: Unpin>(
    http: &Client,
    request: Request,
) -> Result<twilight_http::Response<T>, WebhookSendError> {
    // Twilight 0.18 の HTTP スパンはパスに含まれる Webhook トークンを記録する。
    async {
        http.request::<T>(request)
            .await
            .map_err(WebhookSendError::from_http)
    }
    .with_subscriber(Dispatch::none())
    .await
}

pub(crate) async fn read_webhook_message(
    http: &Client,
    request: Request,
) -> Result<Message, WebhookSendError> {
    request_with_webhook_token::<Message>(http, request)
        .await?
        .model()
        .with_subscriber(Dispatch::none())
        .await
        .map_err(|_| WebhookSendError::InvalidResponse)
}

#[cfg(test)]
mod tests;
