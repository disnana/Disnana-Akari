use std::num::NonZeroU16;

use twilight_http::{Client, Error as HttpError, response::DeserializeBodyError};
use twilight_model::{
    guild::Ban,
    id::{
        Id,
        marker::{GuildMarker, UserMarker},
    },
};

use crate::rest_pagination::IdPageCursor as Cursor;

const MAX_PAGE_SIZE: u16 = 1_000;

/// Discord REST でサーバーの BAN 一覧を必要なページだけ取得します。
///
/// Bot に `BAN_MEMBERS` または `VIEW_AUDIT_LOG` が必要です。取得結果はキャッシュに保存しません。
/// ページの取得中に BAN が追加・解除されるため、全ページで同じ時点の一覧は保証されません。
pub struct BanPages<'http> {
    http: &'http Client,
    guild: Id<GuildMarker>,
    page_size: u16,
    cursor: Cursor<UserMarker>,
    remaining: Option<usize>,
    exhausted: bool,
}

impl<'http> BanPages<'http> {
    /// 通信せずに取得を準備します。先頭からユーザー ID の昇順で読み進めます。
    /// 1 ページの上限は 1,000 件です。総件数を制限する場合は [`Self::limit`] を指定します。
    #[must_use]
    pub fn new(http: &'http Client, guild: Id<GuildMarker>) -> Self {
        Self {
            http,
            guild,
            page_size: MAX_PAGE_SIZE,
            cursor: Cursor::After(None),
            remaining: None,
            exhausted: false,
        }
    }

    /// 指定したユーザー ID より大きい ID を持つ項目から、ID の昇順で取得します。
    /// 以前に指定した `before` または `after` は置き換わります。
    #[must_use]
    pub fn after(mut self, user: Id<UserMarker>) -> Self {
        self.cursor = Cursor::After(Some(user));
        self
    }

    /// 指定したユーザー ID より小さい ID を持つ項目から、より小さい ID の範囲へ進みます。
    /// 以前に指定した `before` または `after` は置き換わります。
    #[must_use]
    pub fn before(mut self, user: Id<UserMarker>) -> Self {
        self.cursor = Cursor::Before(user);
        self
    }

    /// 返す BAN の総件数の上限を設定します。0 件の場合はリクエストを送りません。
    #[must_use]
    pub fn limit(mut self, limit: usize) -> Self {
        self.remaining = Some(limit);
        self
    }

    /// リクエスト 1 回あたりの上限を変更します。1,000 件を超える場合は 1,000 件とします。
    #[must_use]
    pub fn page_size(mut self, size: NonZeroU16) -> Self {
        self.page_size = size.get().min(MAX_PAGE_SIZE);
        self
    }

    /// 次のページを返します。すべて取得したか、総件数の上限に達した場合は `None` です。
    ///
    /// Discord と同じく、各ページ内はユーザー ID の昇順です。`before` で続けて取得すると、
    /// 次のページには前のページより小さい ID が入ります。ユーザー ID は BAN 時刻を表しません。
    ///
    /// # エラー
    ///
    /// REST リクエストや応答の読み取りに失敗した場合、または応答に重複・範囲外の ID や
    /// 要求件数を超える項目がある場合に失敗します。エラー時はカーソルを進めません。
    pub async fn next_page(&mut self) -> Result<Option<Vec<Ban>>, BanPagesError> {
        let limit = self
            .remaining
            .and_then(|remaining| u16::try_from(remaining).ok())
            .map_or(self.page_size, |remaining| self.page_size.min(remaining));
        if self.exhausted || limit == 0 {
            return Ok(None);
        }

        let request = self.http.bans(self.guild).limit(limit);
        let request = match self.cursor {
            Cursor::After(None) => request,
            Cursor::After(Some(id)) => request.after(id),
            Cursor::Before(id) => request.before(id),
        };
        let mut bans = request.await?.models().await?;
        let limit = usize::from(limit);
        let cursor = self
            .cursor
            .advance(&mut bans, limit, |ban| ban.user.id)
            .ok_or(BanPagesError::InvalidPage)?;
        self.exhausted = bans.len() < limit;
        if let Some(remaining) = &mut self.remaining {
            *remaining -= bans.len();
        }
        self.cursor = cursor;
        Ok((!bans.is_empty()).then_some(bans))
    }
}

/// REST でサーバーの BAN 一覧をページ取得できなかった理由です。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BanPagesError {
    /// Discord へのリクエストが失敗しました。
    #[error("Discord の BAN 一覧を取得できませんでした")]
    Http(HttpError),
    /// Discord の応答本文を BAN 一覧として読み取れませんでした。
    #[error("Discord の BAN 一覧の応答を読み取れませんでした")]
    HttpBody(DeserializeBodyError),
    /// 応答に要求件数を超える項目、重複 ID、または範囲外の ID が含まれます。
    #[error("Discord の BAN 一覧のページが無効です")]
    InvalidPage,
}

impl From<HttpError> for BanPagesError {
    fn from(error: HttpError) -> Self {
        Self::Http(error)
    }
}

impl From<DeserializeBodyError> for BanPagesError {
    fn from(error: DeserializeBodyError) -> Self {
        Self::HttpBody(error)
    }
}

#[cfg(test)]
mod tests;
