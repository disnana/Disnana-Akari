use std::num::NonZeroU16;

use twilight_http::{Client, Error as HttpError, response::DeserializeBodyError};
use twilight_model::{
    application::monetization::Subscription,
    id::{
        Id,
        marker::{SkuMarker, SubscriptionMarker, UserMarker},
    },
};

use crate::rest_pagination::IdPageCursor as Cursor;

const MAX_PAGE_SIZE: u8 = 100;

/// 指定したユーザーと SKU に対応するサブスクリプションを Discord REST からページ単位で取得します。
///
/// 取得中もサブスクリプションは更新されます。特典の付与可否には Entitlement を使ってください。
pub struct SubscriptionPages<'http> {
    http: &'http Client,
    sku: Id<SkuMarker>,
    user: Id<UserMarker>,
    cursor: Cursor<SubscriptionMarker>,
    page_size: u8,
    remaining: Option<usize>,
    exhausted: bool,
}

impl<'http> SubscriptionPages<'http> {
    /// 通信せずに準備します。Bot での取得には対象ユーザーの指定が必要です。
    #[must_use]
    pub fn new(http: &'http Client, sku: Id<SkuMarker>, user: Id<UserMarker>) -> Self {
        Self {
            http,
            sku,
            user,
            cursor: Cursor::After(None),
            page_size: MAX_PAGE_SIZE,
            remaining: None,
            exhausted: false,
        }
    }

    /// 指定したサブスクリプション ID より大きい ID から取得します。
    /// 以前に指定した `before` または `after` は置き換わります。
    #[must_use]
    pub fn after(mut self, subscription: Id<SubscriptionMarker>) -> Self {
        self.cursor = Cursor::After(Some(subscription));
        self
    }

    /// 指定したサブスクリプション ID より小さい ID から、より小さい範囲へ進みます。
    /// 以前に指定した `before` または `after` は置き換わります。
    #[must_use]
    pub fn before(mut self, subscription: Id<SubscriptionMarker>) -> Self {
        self.cursor = Cursor::Before(subscription);
        self
    }

    /// 返す総件数の上限を設定します。0 件の場合はリクエストを送りません。
    #[must_use]
    pub fn limit(mut self, limit: usize) -> Self {
        self.remaining = Some(limit);
        self
    }

    /// リクエスト 1 回あたりの上限を設定します。既定は 100 件で、100 件が上限です。
    #[must_use]
    pub fn page_size(mut self, size: NonZeroU16) -> Self {
        self.page_size = size.get().min(u16::from(MAX_PAGE_SIZE)) as u8;
        self
    }

    /// 次のページを ID の昇順で返します。取得を終えた場合は `None` です。
    ///
    /// `before` を指定した場合、次のページへ進むと前のページより小さい ID が入ります。
    ///
    /// # エラー
    ///
    /// 通信、応答の読み取り、ページの検証に失敗するとエラーを返します。
    /// エラー時はカーソルと残り件数を保持します。
    pub async fn next_page(&mut self) -> Result<Option<Vec<Subscription>>, SubscriptionPagesError> {
        let limit = self
            .remaining
            .and_then(|remaining| u8::try_from(remaining).ok())
            .map_or(self.page_size, |remaining| self.page_size.min(remaining));
        if self.exhausted || limit == 0 {
            return Ok(None);
        }

        let request = self
            .http
            .get_sku_subscriptions(self.sku)
            .user_id(self.user)
            .limit(limit);
        let request = match self.cursor {
            Cursor::After(None) => request,
            Cursor::After(Some(id)) => request.after(id),
            Cursor::Before(id) => request.before(id),
        };
        let mut subscriptions = request.await?.models().await?;
        let limit = usize::from(limit);
        if subscriptions.len() > limit
            || subscriptions.iter().any(|subscription| {
                subscription.user_id != self.user || !subscription.sku_ids.contains(&self.sku)
            })
        {
            return Err(SubscriptionPagesError::InvalidPage);
        }
        let cursor = self
            .cursor
            .advance(&mut subscriptions, limit, |subscription| subscription.id)
            .ok_or(SubscriptionPagesError::InvalidPage)?;
        self.exhausted = subscriptions.len() < limit;
        if let Some(remaining) = &mut self.remaining {
            *remaining -= subscriptions.len();
        }
        self.cursor = cursor;
        Ok((!subscriptions.is_empty()).then_some(subscriptions))
    }
}

/// REST でサブスクリプションのページを取得できなかった理由です。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SubscriptionPagesError {
    /// Discord へのリクエストが失敗しました。
    #[error("サブスクリプションを取得できませんでした")]
    Http(#[from] HttpError),
    /// Discord の応答をサブスクリプションの一覧として読み取れませんでした。
    #[error("サブスクリプションの応答を読み取れませんでした")]
    HttpBody(#[from] DeserializeBodyError),
    /// ページの件数、ID、ユーザーまたは SKU が要求と一致しません。
    #[error("サブスクリプションのページが無効です")]
    InvalidPage,
}

#[cfg(test)]
mod tests;
