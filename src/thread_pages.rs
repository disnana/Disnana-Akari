use std::{collections::HashSet, num::NonZeroU16};

use twilight_http::Client;
use twilight_model::{
    channel::{Channel, ChannelType, thread::ThreadsListing},
    id::{Id, marker::ChannelMarker},
    util::Timestamp,
};

const MIN_PAGE_SIZE: u16 = 2;
const MAX_PAGE_SIZE: u16 = 100;

/// Discord REST でアーカイブ済みスレッドを必要なページだけ取得します。
///
/// 取得した情報はキャッシュへ保存しません。取得中もスレッドは更新されるため、
/// 全ページで同じ時点の一覧は保証されません。
pub struct ThreadPages<'http> {
    http: &'http Client,
    channel: Id<ChannelMarker>,
    route: Route,
    page_size: u16,
    remaining: Option<usize>,
    exhausted: bool,
}

#[derive(Clone, Copy)]
enum Route {
    Public(Option<Timestamp>),
    Private(Option<Timestamp>),
    JoinedPrivate(Option<Id<ChannelMarker>>),
}

impl<'http> ThreadPages<'http> {
    /// 公開スレッドをアーカイブ日時の降順で取得します。
    ///
    /// `before` に指定した日時より前から取得します。メッセージ履歴を読む権限が必要です。
    #[must_use]
    pub fn public(
        http: &'http Client,
        channel: Id<ChannelMarker>,
        before: Option<Timestamp>,
    ) -> Self {
        Self::new(http, channel, Route::Public(before))
    }

    /// 非公開スレッドをアーカイブ日時の降順で取得します。
    ///
    /// `before` に指定した日時より前から取得します。履歴を読み、スレッドを管理する権限が必要です。
    #[must_use]
    pub fn private(
        http: &'http Client,
        channel: Id<ChannelMarker>,
        before: Option<Timestamp>,
    ) -> Self {
        Self::new(http, channel, Route::Private(before))
    }

    /// Bot が参加した非公開スレッドをスレッド ID の降順で取得します。
    ///
    /// `before` に指定した ID より小さい ID から取得します。メッセージ履歴を読む権限が必要です。
    #[must_use]
    pub fn joined_private(
        http: &'http Client,
        channel: Id<ChannelMarker>,
        before: Option<Id<ChannelMarker>>,
    ) -> Self {
        Self::new(http, channel, Route::JoinedPrivate(before))
    }

    fn new(http: &'http Client, channel: Id<ChannelMarker>, route: Route) -> Self {
        Self {
            http,
            channel,
            route,
            page_size: MAX_PAGE_SIZE,
            remaining: None,
            exhausted: false,
        }
    }

    /// 返すスレッドの総件数の上限を設定します。0件の場合はリクエストを送りません。
    #[must_use]
    pub fn limit(mut self, limit: usize) -> Self {
        self.remaining = Some(limit);
        self
    }

    /// リクエスト1回あたりの上限を設定します。既定は100件で、範囲は2〜100件です。
    #[must_use]
    pub fn page_size(mut self, size: NonZeroU16) -> Self {
        self.page_size = size.get().clamp(MIN_PAGE_SIZE, MAX_PAGE_SIZE);
        self
    }

    /// スレッドと、Discord が返したスレッドメンバー情報を次の1ページとして返します。
    ///
    /// Discord の `has_more` が真なら、次の呼び出しで続きのページを取得します。
    /// 総件数の上限に達した場合や取得を終えた場合は `None` です。
    /// 日時カーソルでは、ページ境界にある同一日時のスレッドを区別できません。
    ///
    /// # エラー
    ///
    /// 通信、応答の読み取り、ページの検証に失敗するとエラーを返します。
    /// エラー時にカーソルは進みません。
    pub async fn next_page(&mut self) -> Result<Option<ThreadsListing>, ThreadPagesError> {
        let take = self
            .remaining
            .unwrap_or(usize::MAX)
            .min(usize::from(self.page_size));
        if self.exhausted || take == 0 {
            return Ok(None);
        }

        let requested = take.max(usize::from(MIN_PAGE_SIZE));
        let mut listing = self.fetch(requested as u64).await?;
        let has_more = listing.has_more.ok_or(ThreadPagesError::InvalidPage)?;
        if listing.threads.len() > requested || (has_more && listing.threads.is_empty()) {
            return Err(ThreadPagesError::InvalidPage);
        }
        let next = self.route.advance(&listing.threads)?;

        if listing.threads.len() > take {
            listing.threads.truncate(take);
            listing.members.retain(|member| {
                listing
                    .threads
                    .iter()
                    .any(|thread| member.id == Some(thread.id))
            });
        }
        if let Some(remaining) = &mut self.remaining {
            *remaining -= listing.threads.len();
        }
        self.exhausted = !has_more;
        self.route = next;
        Ok((!listing.threads.is_empty()).then_some(listing))
    }

    async fn fetch(&self, limit: u64) -> Result<ThreadsListing, ThreadPagesError> {
        let response = match self.route {
            Route::Public(before) => {
                let before = before.map(utc_cursor);
                let mut request = self.http.public_archived_threads(self.channel).limit(limit);
                if let Some(before) = before.as_deref() {
                    request = request.before(before);
                }
                request.await?
            }
            Route::Private(before) => {
                let before = before.map(utc_cursor);
                let mut request = self
                    .http
                    .private_archived_threads(self.channel)
                    .limit(limit);
                if let Some(before) = before.as_deref() {
                    request = request.before(before);
                }
                request.await?
            }
            Route::JoinedPrivate(before) => {
                let mut request = self
                    .http
                    .joined_private_archived_threads(self.channel)
                    .limit(limit);
                if let Some(before) = before {
                    request = request.before(before);
                }
                request.await?
            }
        };
        Ok(response.model().await?)
    }
}

impl Route {
    fn advance(self, threads: &[Channel]) -> Result<Self, ThreadPagesError> {
        let mut ids = HashSet::with_capacity(threads.len());
        for thread in threads {
            let matches_kind = match self {
                Self::Public(_) => matches!(
                    thread.kind,
                    ChannelType::AnnouncementThread | ChannelType::PublicThread
                ),
                _ => thread.kind == ChannelType::PrivateThread,
            };
            if !matches_kind || !ids.insert(thread.id) {
                return Err(ThreadPagesError::InvalidPage);
            }
        }
        match self {
            Self::Public(before) => Ok(Self::Public(advance_timestamp(before, threads)?)),
            Self::Private(before) => Ok(Self::Private(advance_timestamp(before, threads)?)),
            Self::JoinedPrivate(mut cursor) => {
                for thread in threads {
                    if cursor.is_some_and(|before| thread.id >= before) {
                        return Err(ThreadPagesError::InvalidPage);
                    }
                    cursor = Some(thread.id);
                }
                Ok(Self::JoinedPrivate(cursor))
            }
        }
    }
}

fn advance_timestamp(
    before: Option<Timestamp>,
    threads: &[Channel],
) -> Result<Option<Timestamp>, ThreadPagesError> {
    let mut last = None::<Timestamp>;
    for thread in threads {
        let timestamp = thread
            .thread_metadata
            .as_ref()
            .ok_or(ThreadPagesError::InvalidPage)?
            .archive_timestamp;
        if before.is_some_and(|before| timestamp.as_micros() >= before.as_micros())
            || last.is_some_and(|last| timestamp.as_micros() > last.as_micros())
        {
            return Err(ThreadPagesError::InvalidPage);
        }
        last = Some(timestamp);
    }
    Ok(last.or(before))
}

fn utc_cursor(timestamp: Timestamp) -> String {
    // Twilight inserts this value directly into the URL. A literal '+' can decode as a space.
    let mut value = timestamp.iso_8601().to_string();
    if let Some(utc) = value.strip_suffix("+00:00") {
        value.truncate(utc.len());
        value.push('Z');
    }
    value
}

/// REST でアーカイブ済みスレッドのページを取得できなかった理由です。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ThreadPagesError {
    /// Discord へのリクエストが失敗しました。
    #[error("アーカイブ済みスレッドを取得できませんでした")]
    Http(#[from] twilight_http::Error),
    /// Discord の応答をスレッド一覧として読み取れませんでした。
    #[error("アーカイブ済みスレッドの応答を読み取れませんでした")]
    HttpBody(#[from] twilight_http::response::DeserializeBodyError),
    /// 応答の種類、件数、並び順、または次ページの情報が無効です。
    #[error("アーカイブ済みスレッドのページが無効です")]
    InvalidPage,
}

#[cfg(test)]
mod tests;
