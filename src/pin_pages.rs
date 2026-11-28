use std::{collections::HashSet, num::NonZeroU16};

use twilight_http::{
    Client,
    request::{Method, RequestBuilder},
};
use twilight_model::{
    channel::message::{MessagePin, PinsListing},
    id::{Id, marker::ChannelMarker},
    util::Timestamp,
};

use crate::rest_pagination::utc_cursor;

const MAX_PAGE_SIZE: u16 = 50;

/// チャンネル内のピン留めを、ピン留め日時の新しい順に読み取ります。
///
/// チャンネルの表示権限とメッセージ履歴を読む権限が必要です。
/// 取得中にピン留めが変わると、ページ間で結果が変わることがあります。
#[must_use]
pub struct PinPages<'http> {
    http: &'http Client,
    channel: Id<ChannelMarker>,
    before: Option<Timestamp>,
    page_size: u16,
    remaining: Option<usize>,
    exhausted: bool,
}

impl<'http> PinPages<'http> {
    /// 通信せずにページ取得を準備します。
    pub const fn new(http: &'http Client, channel: Id<ChannelMarker>) -> Self {
        Self {
            http,
            channel,
            before: None,
            page_size: MAX_PAGE_SIZE,
            remaining: None,
            exhausted: false,
        }
    }

    /// 指定した日時より前のピン留めから読みます。
    pub const fn before(mut self, timestamp: Timestamp) -> Self {
        self.before = Some(timestamp);
        self
    }

    /// 取得する総件数を指定します。0 件の場合はリクエストを送りません。
    pub const fn limit(mut self, limit: usize) -> Self {
        self.remaining = Some(limit);
        self
    }

    /// 1 回に取得する件数を指定します。50 件を超える値は 50 件にします。
    pub fn page_size(mut self, size: NonZeroU16) -> Self {
        self.page_size = size.get().min(MAX_PAGE_SIZE);
        self
    }

    /// ピン留め日時とメッセージを含む次のページを返します。取得を終えた場合は `None` です。
    ///
    /// Discord の日時カーソルでは、ページ境界にある同一日時のピン留めを区別できません。
    ///
    /// # エラー
    ///
    /// リクエストやデコードに失敗した場合、または件数、チャンネル、順序、日時が
    /// 不正な場合にエラーを返します。失敗時にカーソルは進みません。
    pub async fn next_page(&mut self) -> Result<Option<Vec<MessagePin>>, PinPagesError> {
        let limit = self
            .remaining
            .unwrap_or(usize::MAX)
            .min(usize::from(self.page_size));
        if self.exhausted || limit == 0 {
            return Ok(None);
        }

        let pins = self.fetch(limit).await?;
        self.check_page(&pins, limit)?;
        self.exhausted = !pins.has_more;
        if let Some(pin) = pins.items.last() {
            self.before = Some(pin.pinned_at);
        }
        if let Some(remaining) = &mut self.remaining {
            *remaining -= pins.items.len();
        }
        Ok((!pins.items.is_empty()).then_some(pins.items))
    }

    async fn fetch(&self, limit: usize) -> Result<PinsListing, PinPagesError> {
        if let Some(before) = self.before {
            // Twilight 0.18 は日時をそのままクエリに書くため、+00:00 が空白になる場合がある。
            let path = format!(
                "channels/{}/messages/pins?before={}&limit={limit}",
                self.channel,
                utc_cursor(before)
            );
            let request = RequestBuilder::raw(Method::Get, path).build()?;
            return Ok(self
                .http
                .request::<PinsListing>(request)
                .await?
                .model()
                .await?);
        }
        Ok(self
            .http
            .pins(self.channel)
            .limit(limit as u16)
            .await?
            .model()
            .await?)
    }

    fn check_page(&self, pins: &PinsListing, limit: usize) -> Result<(), PinPagesError> {
        if pins.items.len() > limit || (pins.has_more && pins.items.is_empty()) {
            return Err(PinPagesError::InvalidPage);
        }
        let mut ids = HashSet::with_capacity(pins.items.len());
        let mut previous = None;
        let before = self.before.map(|timestamp| timestamp.as_micros());
        for pin in &pins.items {
            let date = pin.pinned_at.as_micros();
            if pin.message.channel_id != self.channel
                || before.is_some_and(|before| date >= before)
                || previous.is_some_and(|previous| date > previous)
                || !ids.insert(pin.message.id)
            {
                return Err(PinPagesError::InvalidPage);
            }
            previous = Some(date);
        }
        Ok(())
    }
}

/// ピン留めのページ取得に失敗した理由です。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PinPagesError {
    /// Discord へのリクエストが失敗しました。
    #[error("ピン留め一覧を取得できませんでした")]
    Http(#[from] twilight_http::Error),
    /// Discord の応答をピン留め一覧として読み取れませんでした。
    #[error("ピン留め一覧を読み取れませんでした")]
    HttpBody(#[from] twilight_http::response::DeserializeBodyError),
    /// 返された件数、チャンネル、順序、日時、または続きのページに関する情報が無効です。
    #[error("ピン留め一覧のページが無効です")]
    InvalidPage,
}

#[cfg(test)]
mod tests;
