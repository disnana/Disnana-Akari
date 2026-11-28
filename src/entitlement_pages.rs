use std::num::NonZeroU16;

use twilight_http::{
    Client, Error as HttpError,
    request::Request,
    response::{DeserializeBodyError, marker::ListBody},
    routing::Route,
};
use twilight_model::{
    application::monetization::Entitlement,
    id::{
        Id,
        marker::{ApplicationMarker, EntitlementMarker, GuildMarker, SkuMarker, UserMarker},
    },
};

use crate::rest_pagination::IdPageCursor as Cursor;

const MAX_PAGE_SIZE: u8 = 100;

/// Discord REST でアプリケーションのエンタイトルメントを必要なページだけ取得します。
///
/// 既定では終了済みの項目を含み、削除済みの項目を除きます。
/// 取得中に項目が変わることがあるため、全ページが同じ時点の一覧になる保証はありません。
pub struct EntitlementPages<'http> {
    http: &'http Client,
    application: Id<ApplicationMarker>,
    cursor: Cursor<EntitlementMarker>,
    user: Option<Id<UserMarker>>,
    guild: Option<Id<GuildMarker>>,
    skus: &'http [Id<SkuMarker>],
    exclude_ended: bool,
    exclude_deleted: bool,
    page_size: u8,
    remaining: Option<usize>,
    exhausted: bool,
}

impl<'http> EntitlementPages<'http> {
    /// 通信せずに取得を準備します。既定では ID の小さい方から取得します。
    /// 1 ページあたり最大 100 件です。総件数を制限する場合は [`Self::limit`] を指定します。
    #[must_use]
    pub fn new(http: &'http Client, application: Id<ApplicationMarker>) -> Self {
        Self {
            http,
            application,
            cursor: Cursor::After(None),
            user: None,
            guild: None,
            skus: &[],
            exclude_ended: false,
            exclude_deleted: true,
            page_size: MAX_PAGE_SIZE,
            remaining: None,
            exhausted: false,
        }
    }

    /// この ID より大きい項目を取得します。以前に指定した取得位置を置き換えます。
    #[must_use]
    pub fn after(mut self, entitlement: Id<EntitlementMarker>) -> Self {
        self.cursor = Cursor::After(Some(entitlement));
        self
    }

    /// この ID より小さい項目を取得します。以前に指定した取得位置を置き換えます。
    #[must_use]
    pub fn before(mut self, entitlement: Id<EntitlementMarker>) -> Self {
        self.cursor = Cursor::Before(entitlement);
        self
    }

    /// 指定したユーザーのエンタイトルメントを取得します。
    #[must_use]
    pub fn user_id(mut self, user: Id<UserMarker>) -> Self {
        self.user = Some(user);
        self
    }

    /// 指定したサーバーのエンタイトルメントを取得します。
    #[must_use]
    pub fn guild_id(mut self, guild: Id<GuildMarker>) -> Self {
        self.guild = Some(guild);
        self
    }

    /// 指定したいずれかの SKU に絞ります。空のスライスでは SKU による絞り込みを行いません。
    #[must_use]
    pub fn sku_ids(mut self, skus: &'http [Id<SkuMarker>]) -> Self {
        self.skus = skus;
        self
    }

    /// 終了済みの項目を除くかを Discord に指定します。既定は `false` です。
    #[must_use]
    pub fn exclude_ended(mut self, exclude: bool) -> Self {
        self.exclude_ended = exclude;
        self
    }

    /// 削除済みの項目を除くかを Discord に指定します。既定は `true` です。
    #[must_use]
    pub fn exclude_deleted(mut self, exclude: bool) -> Self {
        self.exclude_deleted = exclude;
        self
    }

    /// リクエスト 1 回あたりの上限を指定します。100 件を超える場合は 100 件とします。
    #[must_use]
    pub fn page_size(mut self, size: NonZeroU16) -> Self {
        self.page_size = u8::try_from(size.get())
            .unwrap_or(MAX_PAGE_SIZE)
            .min(MAX_PAGE_SIZE);
        self
    }

    /// 返す総件数の上限を指定します。0 件の場合は通信しません。
    #[must_use]
    pub fn limit(mut self, limit: usize) -> Self {
        self.remaining = Some(limit);
        self
    }

    /// 次のページを取得します。取得を終えた場合は `None` を返します。
    ///
    /// どちらの方向でも、各ページ内は ID の昇順です。`before` では次のページほど ID が小さくなります。
    ///
    /// # エラー
    ///
    /// 通信に失敗した場合や、応答の形式、件数、ID、絞り込みの条件が合わない場合に失敗します。
    /// エラー時はカーソルと残り件数を保持します。
    pub async fn next_page(&mut self) -> Result<Option<Vec<Entitlement>>, EntitlementPagesError> {
        let limit = self
            .remaining
            .and_then(|remaining| u8::try_from(remaining).ok())
            .map_or(self.page_size, |remaining| self.page_size.min(remaining));
        if self.exhausted || limit == 0 {
            return Ok(None);
        }

        let (after, before) = match self.cursor {
            Cursor::After(after) => (after.map(Id::get), None),
            Cursor::Before(before) => (None, Some(before.get())),
        };
        // Twilight 0.18's exclude_deleted builder currently sets exclude_ended instead.
        let request = Request::from_route(&Route::GetEntitlements {
            after,
            application_id: self.application.get(),
            before,
            exclude_deleted: Some(self.exclude_deleted),
            exclude_ended: Some(self.exclude_ended),
            guild_id: self.guild.map(Id::get),
            limit: Some(limit),
            sku_ids: self.skus,
            user_id: self.user.map(Id::get),
        });
        let mut page = self
            .http
            .request::<ListBody<Entitlement>>(request)
            .await?
            .models()
            .await?;
        if page.iter().any(|entitlement| !self.matches(entitlement)) {
            return Err(EntitlementPagesError::InvalidPage);
        }
        let limit = usize::from(limit);
        self.cursor = self
            .cursor
            .advance(&mut page, limit, |entitlement| entitlement.id)
            .ok_or(EntitlementPagesError::InvalidPage)?;
        self.exhausted = page.len() < limit;
        if let Some(remaining) = &mut self.remaining {
            *remaining -= page.len();
        }
        Ok((!page.is_empty()).then_some(page))
    }

    fn matches(&self, entitlement: &Entitlement) -> bool {
        entitlement.application_id == self.application
            && self
                .user
                .is_none_or(|user| entitlement.user_id == Some(user))
            && self
                .guild
                .is_none_or(|guild| entitlement.guild_id == Some(guild))
            && (self.skus.is_empty() || self.skus.contains(&entitlement.sku_id))
            && (!self.exclude_deleted || !entitlement.deleted)
    }
}

/// Discord REST でエンタイトルメントをページ取得できなかった理由です。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EntitlementPagesError {
    /// Discord へのリクエストが失敗しました。
    #[error("Discord のエンタイトルメントを取得できませんでした")]
    Http(HttpError),
    /// Discord の応答本文をエンタイトルメント一覧として読み取れませんでした。
    #[error("Discord のエンタイトルメントの応答を読み取れませんでした")]
    HttpBody(DeserializeBodyError),
    /// 応答に範囲外、重複、件数超過、または指定した条件に合わない項目が含まれます。
    #[error("Discord のエンタイトルメントのページが無効です")]
    InvalidPage,
}

impl From<HttpError> for EntitlementPagesError {
    fn from(error: HttpError) -> Self {
        Self::Http(error)
    }
}

impl From<DeserializeBodyError> for EntitlementPagesError {
    fn from(error: DeserializeBodyError) -> Self {
        Self::HttpBody(error)
    }
}

#[cfg(test)]
mod tests;
