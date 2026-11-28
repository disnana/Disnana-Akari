use std::num::NonZeroU16;

use twilight_http::{Client, Error as HttpError, response::DeserializeBodyError};
use twilight_model::{
    channel::thread::ThreadMember,
    id::{
        Id,
        marker::{ChannelMarker, UserMarker},
    },
};

const MAX_PAGE_SIZE: u16 = 100;

/// Discord REST でスレッドの参加メンバーを必要なページだけ取得します。
///
/// Discord はこの API で `GUILD_MEMBERS` の特権 Intent が有効かを確認します。
/// 取得中に参加者が変わることがあるため、全ページが同じ時点の一覧になる保証はありません。
pub struct ThreadMemberPages<'http> {
    http: &'http Client,
    thread: Id<ChannelMarker>,
    after: Option<Id<UserMarker>>,
    page_size: u16,
    remaining: Option<usize>,
    exhausted: bool,
}

impl<'http> ThreadMemberPages<'http> {
    /// 通信せずに取得を準備します。ユーザー ID の小さい方から取得します。
    /// 1 ページあたり最大 100 件です。総件数を制限する場合は [`Self::limit`] を指定します。
    #[must_use]
    pub fn new(http: &'http Client, thread: Id<ChannelMarker>) -> Self {
        Self {
            http,
            thread,
            after: None,
            page_size: MAX_PAGE_SIZE,
            remaining: None,
            exhausted: false,
        }
    }

    /// 指定したユーザー ID より大きい ID を持つメンバーだけを取得します。
    #[must_use]
    pub fn after(mut self, user: Id<UserMarker>) -> Self {
        self.after = Some(user);
        self
    }

    /// リクエスト 1 回あたりの上限を指定します。100 件を超える場合は 100 件とします。
    #[must_use]
    pub fn page_size(mut self, size: NonZeroU16) -> Self {
        self.page_size = size.get().min(MAX_PAGE_SIZE);
        self
    }

    /// 返すメンバーの総件数の上限を指定します。0 件の場合は通信しません。
    #[must_use]
    pub fn limit(mut self, limit: usize) -> Self {
        self.remaining = Some(limit);
        self
    }

    /// 次のページをユーザー ID の昇順で取得します。空の応答を受け取ったか、総件数に達した場合は `None` です。
    ///
    /// Discord API v10 でページ分割を有効にするため `with_member=true` を送ります。
    /// サーバーのメンバー情報が省略されていれば、[`ThreadMember::member`] は `None` です。
    ///
    /// # エラー
    ///
    /// 通信やデコードに失敗した場合、あるいは応答の件数または ID が無効な場合にエラーを返します。
    /// エラー時はカーソルと残り件数を保持します。
    pub async fn next_page(&mut self) -> Result<Option<Vec<ThreadMember>>, ThreadMemberPagesError> {
        let limit = self
            .remaining
            .and_then(|remaining| u16::try_from(remaining).ok())
            .map_or(self.page_size, |remaining| self.page_size.min(remaining));
        if self.exhausted || limit == 0 {
            return Ok(None);
        }
        let mut request = self
            .http
            .thread_members(self.thread)
            .with_member(true)
            .limit(u32::from(limit));
        if let Some(after) = self.after {
            request = request.after(after);
        }
        let mut page = request.await?.models().await?;
        let Some(after) = self.advance(&mut page, usize::from(limit))? else {
            self.exhausted = true;
            return Ok(None);
        };
        self.after = Some(after);
        if let Some(remaining) = &mut self.remaining {
            *remaining -= page.len();
        }
        Ok(Some(page))
    }

    fn advance(
        &self,
        page: &mut [ThreadMember],
        limit: usize,
    ) -> Result<Option<Id<UserMarker>>, ThreadMemberPagesError> {
        if page.len() > limit {
            return Err(ThreadMemberPagesError::InvalidPage);
        }
        page.sort_unstable_by_key(|member| member.user_id);
        let mut last = None;
        for member in page {
            let user = member.user_id.ok_or(ThreadMemberPagesError::InvalidPage)?;
            if last.or(self.after).is_some_and(|after| user <= after)
                || member.id.is_some_and(|thread| thread != self.thread)
                || member
                    .member
                    .as_ref()
                    .is_some_and(|guild_member| guild_member.user.id != user)
            {
                return Err(ThreadMemberPagesError::InvalidPage);
            }
            last = Some(user);
        }
        Ok(last)
    }
}

/// Discord REST でスレッドの参加メンバーをページ取得できなかった理由です。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ThreadMemberPagesError {
    /// Discord へのリクエストが失敗しました。
    #[error("Discord のスレッドメンバー一覧を取得できませんでした")]
    Http(HttpError),
    /// Discord の応答本文をスレッドメンバー一覧として読み取れませんでした。
    #[error("Discord のスレッドメンバー一覧の応答を読み取れませんでした")]
    HttpBody(DeserializeBodyError),
    /// 応答の件数、スレッド ID、またはユーザー ID が取得条件を満たしません。
    #[error("Discord のスレッドメンバー一覧のページが無効です")]
    InvalidPage,
}

impl From<HttpError> for ThreadMemberPagesError {
    fn from(error: HttpError) -> Self {
        Self::Http(error)
    }
}

impl From<DeserializeBodyError> for ThreadMemberPagesError {
    fn from(error: DeserializeBodyError) -> Self {
        Self::HttpBody(error)
    }
}

#[cfg(test)]
mod tests;
