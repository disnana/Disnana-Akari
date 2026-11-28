use twilight_http::{Client, Error as HttpError, response::DeserializeBodyError};
use twilight_model::{
    channel::{
        Message,
        message::{AllowedMentions, MessageReferenceType},
    },
    id::{
        Id,
        marker::{ChannelMarker, MessageMarker},
    },
};

/// Discord の転送機能で、指定したメッセージを指定先のチャンネルに送ります。
///
/// Bot が転送元の内容を読めて、転送先に投稿できることが必要です。利用者にこの操作を提供する場合は、
/// アプリ側で利用者の転送元の閲覧権限と、転送先への公開可否も確認してください。
/// 元メッセージを取得する追加リクエストは行わず、メンション通知は抑止します。
///
/// # エラー
///
/// Discord が転送を拒否した場合、応答を読み取れない場合、または応答が転送元・転送先の指定と
/// 一致しない場合はエラーを返します。応答の読み取りや照合に失敗しても、Discord 側では投稿が
/// 完了している場合があります。
pub async fn forward_message(
    http: &Client,
    source_channel: Id<ChannelMarker>,
    source_message: Id<MessageMarker>,
    destination: Id<ChannelMarker>,
) -> Result<Message, ForwardMessageError> {
    let forwarded = http
        .create_message(destination)
        .forward(source_channel, source_message)
        .fail_if_not_exists(true)
        .allowed_mentions(Some(&AllowedMentions::default()))
        .await?
        .model()
        .await?;
    if forwarded.channel_id != destination
        || forwarded.id == source_message
        || forwarded.reference.as_ref().is_none_or(|reference| {
            reference.kind != MessageReferenceType::Forward
                || reference.channel_id != Some(source_channel)
                || reference.message_id != Some(source_message)
        })
    {
        return Err(ForwardMessageError::InvalidResponse);
    }
    Ok(forwarded)
}

/// Discord REST でメッセージを転送できなかった理由です。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ForwardMessageError {
    /// Discord へのリクエストが失敗しました。
    #[error("Discord のメッセージを転送できませんでした")]
    Http(HttpError),
    /// Discord の応答本文をメッセージとして読み取れませんでした。
    #[error("Discord のメッセージ転送の応答を読み取れませんでした")]
    HttpBody(DeserializeBodyError),
    /// Discord の応答が転送元または転送先の指定と一致しません。
    #[error("Discord のメッセージ転送の応答が指定と一致しません")]
    InvalidResponse,
}

impl From<HttpError> for ForwardMessageError {
    fn from(error: HttpError) -> Self {
        Self::Http(error)
    }
}

impl From<DeserializeBodyError> for ForwardMessageError {
    fn from(error: DeserializeBodyError) -> Self {
        Self::HttpBody(error)
    }
}

#[cfg(test)]
mod tests;
