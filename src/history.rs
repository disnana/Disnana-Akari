use twilight_http::{Client, Error as HttpError, response::DeserializeBodyError};
use twilight_model::{
    channel::Message,
    id::{
        Id,
        marker::{ChannelMarker, MessageMarker},
    },
};

use crate::rest_pagination::advance_after;

const MAX_PAGE_SIZE: u16 = 100;

/// チャンネルのメッセージ履歴を取得する起点です。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum HistoryCursor {
    /// 取得できる最新のメッセージから、古い方向へ進みます。
    Latest,
    /// 指定したメッセージより古い方向へ進みます。指定した ID 自体は含めません。
    Before(Id<MessageMarker>),
    /// 指定したメッセージより新しい方向へ進みます。指定した ID 自体は含めません。
    After(Id<MessageMarker>),
    /// 指定したメッセージの前後を、1 回だけ取得します。
    Around(Id<MessageMarker>),
}

/// Discord REST から、チャンネルのメッセージを必要なページだけ取得します。
///
/// `After` は古い方から新しい方へ返し、ほかの起点では新しい方から古い方へ返します。
/// 取得したメッセージを Akari のキャッシュに保存しません。ページ間にメッセージが投稿・
/// 削除されることがあるため、全ページで同じ時点の履歴は保証されません。
pub struct MessageHistory<'http> {
    http: &'http Client,
    channel: Id<ChannelMarker>,
    cursor: HistoryCursor,
    remaining: usize,
}

impl<'http> MessageHistory<'http> {
    /// 通信せずに取得を準備し、返すメッセージの総件数の上限を設定します。
    /// 0 件なら取得しません。一度のリクエストで要求する上限は 100 件です。
    ///
    /// # エラー
    ///
    /// `Around` を指定して総件数の上限が 100 件を超える場合は失敗します。
    pub fn new(
        http: &'http Client,
        channel: Id<ChannelMarker>,
        cursor: HistoryCursor,
        limit: usize,
    ) -> Result<Self, MessageHistoryError> {
        if matches!(cursor, HistoryCursor::Around(_)) && limit > usize::from(MAX_PAGE_SIZE) {
            return Err(MessageHistoryError::AroundLimit);
        }
        Ok(Self {
            http,
            channel,
            cursor,
            remaining: limit,
        })
    }

    /// 次のページを取得します。空の応答を受け取った場合や、総件数に達した場合は `None` です。
    ///
    /// 指定件数より短いページに値があれば、得られた最後の ID を起点に次の取得を行えます。
    /// `Around` では、Discord が偶数の要求件数より 1 件多く返す場合でも、総件数の上限を守ります。
    ///
    /// # エラー
    ///
    /// 通信や応答の読み取りに失敗したとき、または応答に別チャンネルのメッセージや重複 ID、
    /// 指定した方向の範囲外の ID が含まれるときに失敗します。エラー時は取得位置を進めません。
    pub async fn next_page(&mut self) -> Result<Option<Vec<Message>>, MessageHistoryError> {
        if self.remaining == 0 {
            return Ok(None);
        }
        let limit = u16::try_from(self.remaining)
            .unwrap_or(MAX_PAGE_SIZE)
            .min(MAX_PAGE_SIZE);
        let request = self.http.channel_messages(self.channel).limit(limit);
        let response = match self.cursor {
            HistoryCursor::Latest => request.await?,
            HistoryCursor::Before(id) => request.before(id).await?,
            HistoryCursor::After(id) => request.after(id).await?,
            HistoryCursor::Around(id) => request.around(id).await?,
        };
        let mut messages = response.models().await?;
        let limit = usize::from(limit);
        let newest = validate_page(&mut messages, self.channel, self.cursor, limit)?;
        let (Some(oldest), Some(newest)) = (messages.first().map(|message| message.id), newest)
        else {
            self.remaining = 0;
            return Ok(None);
        };
        if matches!(self.cursor, HistoryCursor::After(_)) {
            self.cursor = HistoryCursor::After(newest);
        } else {
            messages.reverse();
            if matches!(self.cursor, HistoryCursor::Around(_)) {
                messages.truncate(limit);
                self.remaining = 0;
                return Ok(Some(messages));
            }
            self.cursor = HistoryCursor::Before(oldest);
        }
        self.remaining -= messages.len();
        Ok(Some(messages))
    }
}

fn validate_page(
    messages: &mut [Message],
    channel: Id<ChannelMarker>,
    cursor: HistoryCursor,
    limit: usize,
) -> Result<Option<Id<MessageMarker>>, MessageHistoryError> {
    let limit =
        limit + usize::from(matches!(cursor, HistoryCursor::Around(_)) && limit.is_multiple_of(2));
    if messages.len() > limit || messages.iter().any(|message| message.channel_id != channel) {
        return Err(MessageHistoryError::InvalidPage);
    }
    let after = match cursor {
        HistoryCursor::After(after) => Some(after),
        _ => None,
    };
    let newest = advance_after(messages, limit, after, |message| message.id)
        .ok_or(MessageHistoryError::InvalidPage)?;
    if let HistoryCursor::Before(before) = cursor
        && newest.is_some_and(|newest| newest >= before)
    {
        return Err(MessageHistoryError::InvalidPage);
    }
    Ok(newest)
}

/// メッセージ履歴の取得に失敗した理由です。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MessageHistoryError {
    /// `Around` の上限は 100 件です。
    #[error("前後を指定する履歴の取得は 100 件以内にしてください")]
    AroundLimit,
    /// Discord へのリクエストが失敗しました。
    #[error("Discord のメッセージ履歴を取得できませんでした")]
    Http(HttpError),
    /// Discord の応答本文をメッセージ一覧として読み取れませんでした。
    #[error("Discord のメッセージ履歴の応答を読み取れませんでした")]
    HttpBody(DeserializeBodyError),
    /// 応答に要求件数を超える項目、別チャンネルのメッセージ、重複 ID、または範囲外の ID が含まれます。
    #[error("Discord のメッセージ履歴のページが無効です")]
    InvalidPage,
}

impl From<HttpError> for MessageHistoryError {
    fn from(error: HttpError) -> Self {
        Self::Http(error)
    }
}

impl From<DeserializeBodyError> for MessageHistoryError {
    fn from(error: DeserializeBodyError) -> Self {
        Self::HttpBody(error)
    }
}

#[cfg(test)]
mod tests;
