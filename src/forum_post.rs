use serde::Serialize;
use twilight_http::{
    Client,
    request::{
        AuditLogReason, Request, TryIntoRequest,
        attachment::{AttachmentManager, PartialAttachment},
    },
};
use twilight_model::{
    channel::{
        ChannelType,
        message::{AllowedMentions, Embed},
        thread::AutoArchiveDuration,
    },
    http::attachment::Attachment,
    id::{
        Id,
        marker::{ChannelMarker, TagMarker},
    },
};

pub use twilight_http::request::channel::thread::create_forum_thread::ForumThread;

use crate::{Error, Result, validation};

const MAX_APPLIED_TAGS: usize = 5;

/// Forum または Media チャンネルで作成する投稿です。
///
/// スレッドと最初のメッセージを 1 回の Discord REST リクエストで作成します。
/// メッセージからのメンション通知は無効です。投稿先で `SEND_MESSAGES` が必要です。
#[must_use]
pub struct ForumPost<'a> {
    title: &'a str,
    content: Option<&'a str>,
    embeds: &'a [Embed],
    attachments: &'a [Attachment],
    applied_tags: &'a [Id<TagMarker>],
    auto_archive_duration: Option<AutoArchiveDuration>,
    slowmode: Option<u16>,
    reason: Option<&'a str>,
}

impl<'a> ForumPost<'a> {
    /// 投稿タイトルを指定します。本文、Embed、添付のいずれかを設定してください。
    pub const fn new(title: &'a str) -> Self {
        Self {
            title,
            content: None,
            embeds: &[],
            attachments: &[],
            applied_tags: &[],
            auto_archive_duration: None,
            slowmode: None,
            reason: None,
        }
    }

    /// 最初のメッセージの本文を設定します。
    pub const fn content(mut self, content: &'a str) -> Self {
        self.content = Some(content);
        self
    }

    /// 最初のメッセージの Embed を設定します。
    pub const fn embeds(mut self, embeds: &'a [Embed]) -> Self {
        self.embeds = embeds;
        self
    }

    /// 最初のメッセージに添付するファイルを設定します。
    pub const fn attachments(mut self, attachments: &'a [Attachment]) -> Self {
        self.attachments = attachments;
        self
    }

    /// 投稿先で使用できるタグの ID を最大 5 件まで設定します。
    pub const fn applied_tags(mut self, tags: &'a [Id<TagMarker>]) -> Self {
        self.applied_tags = tags;
        self
    }

    /// 無活動で自動アーカイブされるまでの時間を設定します。
    pub const fn auto_archive_duration(mut self, duration: AutoArchiveDuration) -> Self {
        self.auto_archive_duration = Some(duration);
        self
    }

    /// 投稿内で同じユーザーが次のメッセージを送れるまでの秒数を設定します。
    /// 0 秒は低速モードを無効にします。上限は 21,600 秒です。
    pub const fn slowmode(mut self, seconds: u16) -> Self {
        self.slowmode = Some(seconds);
        self
    }

    /// 監査ログに残す理由を設定します。
    pub const fn reason(mut self, reason: &'a str) -> Self {
        self.reason = Some(reason);
        self
    }

    /// 投稿を作成し、Discord が返したスレッドと最初のメッセージを返します。
    ///
    /// # エラー
    ///
    /// 入力やリクエストのサイズが上限に合わない場合、Discord へのリクエストが失敗した場合、
    /// または応答の投稿先、スレッド種別、最初のメッセージの ID が一致しない場合に
    /// エラーを返します。応答の検証に失敗した場合も Discord 側で投稿が作成されていることがあります。
    pub async fn send(self, http: &Client, parent: Id<ChannelMarker>) -> Result<ForumThread> {
        let request = self.request(http, parent)?;
        let post = http.request::<ForumThread>(request).await?.model().await?;
        if post.channel.kind != ChannelType::PublicThread
            || post.channel.parent_id != Some(parent)
            || post.message.channel_id != post.channel.id
            || post.message.id != post.channel.id.cast()
        {
            return Err(Error::InvalidUi(
                "Discord の投稿作成応答が要求と一致しません。",
            ));
        }
        Ok(post)
    }

    fn request(&self, http: &Client, parent: Id<ChannelMarker>) -> Result<Request> {
        self.validate()?;
        let attachment_metadata = AttachmentManager::new()
            .set_files(self.attachments.iter().collect())
            .get_partial_attachments();
        let body = serde_json::to_vec(&PostBody {
            name: self.title,
            auto_archive_duration: self.auto_archive_duration,
            rate_limit_per_user: self.slowmode,
            applied_tags: self.applied_tags,
            message: StarterBody {
                content: self.content,
                embeds: self.embeds,
                attachments: &attachment_metadata,
                allowed_mentions: AllowedMentions::default(),
            },
        })
        .map_err(|_| Error::InvalidUi("投稿の内容を JSON に変換できません。"))?;

        let mut builder = http.create_forum_thread(parent, self.title);
        if let Some(reason) = self.reason {
            builder = builder.reason(reason);
        }
        // 低速モードを含む全体の JSON を渡し、Twilight がファイルパートと監査ログを組み立てる。
        let request = builder
            .message()
            .attachments(self.attachments)
            .payload_json(&body)
            .try_into_request()?;
        let size = request
            .form()
            .map_or_else(|| request.body().map_or(0, <[u8]>::len), |form| form.len());
        if size as u64 > validation::CREATE_MESSAGE_REQUEST_MAX_BYTES {
            return Err(Error::InvalidUi(
                "投稿作成リクエストが 25 MiB を超えています。",
            ));
        }
        Ok(request)
    }

    fn validate(&self) -> Result<()> {
        twilight_validate::channel::name(self.title)?;
        if self.title.trim().is_empty() {
            return Err(Error::InvalidUi("投稿タイトルを指定してください。"));
        }
        if let Some(content) = self.content {
            validation::message_content(content)?;
        }
        if self.content.is_none_or(|content| content.trim().is_empty())
            && self.embeds.is_empty()
            && self.attachments.is_empty()
        {
            return Err(Error::InvalidUi(
                "投稿の本文、Embed、添付のいずれかを指定してください。",
            ));
        }
        validation::embeds(self.embeds)?;
        validation::attachment_count(0, self.attachments.len())?;
        validation::attachment_metadata(self.attachments)?;
        validation::channel_message_upload_size(self.attachments)?;
        if self
            .auto_archive_duration
            .is_some_and(|duration| !matches!(duration.number(), 60 | 1440 | 4320 | 10080))
        {
            return Err(Error::InvalidUi("投稿の自動アーカイブ時間が無効です。"));
        }
        if let Some(seconds) = self.slowmode {
            twilight_validate::channel::rate_limit_per_user(seconds)?;
        }
        validate_forum_tags(self.applied_tags)
    }
}

pub(crate) fn validate_forum_tags(tags: &[Id<TagMarker>]) -> Result<()> {
    if tags.len() > MAX_APPLIED_TAGS {
        return Err(Error::InvalidUi("投稿に指定できるタグは 5 件までです。"));
    }
    if tags
        .iter()
        .enumerate()
        .any(|(index, tag)| tags.iter().take(index).any(|previous| previous == tag))
    {
        return Err(Error::InvalidUi("投稿に同じタグを複数回指定できません。"));
    }
    Ok(())
}

#[derive(Serialize)]
struct PostBody<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    auto_archive_duration: Option<AutoArchiveDuration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rate_limit_per_user: Option<u16>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    applied_tags: &'a [Id<TagMarker>],
    message: StarterBody<'a>,
}

#[derive(Serialize)]
struct StarterBody<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<&'a str>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    embeds: &'a [Embed],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    attachments: &'a [PartialAttachment<'a>],
    allowed_mentions: AllowedMentions,
}

#[cfg(test)]
mod tests;
