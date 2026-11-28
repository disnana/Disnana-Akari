use std::num::NonZeroU16;

use twilight_http::{
    Client, error::Error as HttpError, request::channel::reaction::RequestReactionType,
    response::DeserializeBodyError,
};
use twilight_model::{
    channel::message::ReactionType,
    id::{
        Id,
        marker::{ChannelMarker, MessageMarker, UserMarker},
    },
    user::User,
};

use crate::rest_pagination::advance_after;

const MAX_PAGE_SIZE: u16 = 100;

/// メッセージの絵文字にリアクションしたユーザーを、ID の小さい順に読み取ります。
///
/// ページは必要なときに取得します。一度に保持する結果は最大100件で、呼び出し間の先読みは行いません。
#[must_use]
pub struct ReactionPages<'a> {
    http: &'a Client,
    channel: Id<ChannelMarker>,
    message: Id<MessageMarker>,
    emoji: RequestReactionType<'a>,
    kind: ReactionType,
    cursor: Option<Id<UserMarker>>,
    remaining: Option<usize>,
    page_size: u16,
    finished: bool,
}

impl<'a> ReactionPages<'a> {
    /// メッセージと絵文字を指定します。既定は通常リアクションで1ページ最大100件です。
    pub const fn new(
        http: &'a Client,
        channel: Id<ChannelMarker>,
        message: Id<MessageMarker>,
        emoji: RequestReactionType<'a>,
    ) -> Self {
        Self {
            http,
            channel,
            message,
            emoji,
            kind: ReactionType::Normal,
            cursor: None,
            remaining: None,
            page_size: MAX_PAGE_SIZE,
            finished: false,
        }
    }

    /// 指定した種類のリアクションを読み取ります。
    pub const fn kind(mut self, kind: ReactionType) -> Self {
        self.kind = kind;
        self
    }

    /// 指定した ID より後から読み始めます。この ID 自体は含まれません。
    pub const fn after(mut self, id: Id<UserMarker>) -> Self {
        self.cursor = Some(id);
        self
    }

    /// 最大取得件数を指定します。ゼロの場合は通信しません。
    pub const fn limit(mut self, limit: usize) -> Self {
        self.remaining = Some(limit);
        self
    }

    /// 1 回の取得件数を指定します。100を超える場合は100に丸めます。
    pub fn page_size(mut self, page_size: NonZeroU16) -> Self {
        self.page_size = page_size.get().min(MAX_PAGE_SIZE);
        self
    }

    /// 次のページを取得します。終了した場合は `Ok(None)` です。
    ///
    /// # エラー
    ///
    /// 通信、応答の読み取り、応答の件数または ID の範囲に問題があれば失敗します。
    /// 失敗した場合はカーソルを進めず、同じページから再試行できます。
    pub async fn next_page(&mut self) -> Result<Option<Vec<User>>, ReactionPagesError> {
        if self.finished || self.remaining == Some(0) {
            return Ok(None);
        }
        let limit = usize::from(self.page_size);
        let limit = self
            .remaining
            .map_or(limit, |remaining| remaining.min(limit));
        let mut request = self
            .http
            .reactions(self.channel, self.message, &self.emoji)
            .kind(self.kind)
            .limit(limit as u16);
        if let Some(cursor) = self.cursor {
            request = request.after(cursor);
        }
        let mut users = request.await?.models().await?;
        let next = advance_after(&mut users, limit, self.cursor, |user| user.id)
            .ok_or(ReactionPagesError::InvalidPage)?;
        let Some(cursor) = next else {
            self.finished = true;
            return Ok(None);
        };
        self.cursor = Some(cursor);
        if let Some(remaining) = &mut self.remaining {
            *remaining -= users.len();
        }
        Ok(Some(users))
    }
}

/// リアクションしたユーザー一覧の読み取りに失敗した理由です。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ReactionPagesError {
    /// Discord へ取得を要求できませんでした。
    #[error("リアクションの一覧を取得できませんでした")]
    Http(#[from] HttpError),
    /// Discord の応答本文を読み取れませんでした。
    #[error("リアクション一覧の本文を読み取れませんでした")]
    HttpBody(#[from] DeserializeBodyError),
    /// 一覧に重複した ID、カーソル以前の ID、または上限を超える件数がありました。
    #[error("リアクション一覧のページが無効です")]
    InvalidPage,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::tests::Mock;

    const ROUTE: &str = "/channels/4/messages/6/reactions/%E2%9D%A4";

    fn users(ids: &[u64]) -> String {
        serde_json::Value::Array(
            ids.iter()
                .map(|id| {
                    serde_json::json!({
                        "id":id.to_string(), "username":"fixture", "discriminator":"0"
                    })
                })
                .collect(),
        )
        .to_string()
    }

    #[tokio::test]
    async fn pages_sort_each_response_and_forward_cursor_kind_and_total_limit() {
        let first = format!("{ROUTE}?after=10&limit=2&type=1");
        let second = format!("{ROUTE}?after=12&limit=1&type=1");
        let mock = Mock::start(vec![
            (first.clone(), users(&[12, 11])),
            (second.clone(), users(&[15])),
        ])
        .await;
        let http = mock.client();
        let mut pages = ReactionPages::new(
            &http,
            Id::new(4),
            Id::new(6),
            RequestReactionType::Unicode { name: "❤" },
        )
        .kind(ReactionType::Burst)
        .after(Id::new(10))
        .page_size(NonZeroU16::new(2).unwrap())
        .limit(3);
        assert_eq!(mock.count(&first), 0);
        let page = pages.next_page().await.unwrap().unwrap();
        assert!(page.iter().map(|user| user.id.get()).eq([11, 12]));
        assert_eq!(pages.next_page().await.unwrap().unwrap()[0].id.get(), 15);
        assert!(pages.next_page().await.unwrap().is_none());
        assert_eq!(mock.count(&first), 1);
        assert_eq!(mock.count(&second), 1);
    }

    #[tokio::test]
    async fn invalid_pages_do_not_mutate_the_cursor_or_remaining_size() {
        let route = format!("{ROUTE}?after=10&limit=2&type=0");
        let mock = Mock::start(vec![(route.clone(), users(&[10, 11]))]).await;
        let http = mock.client();
        let mut pages = ReactionPages::new(
            &http,
            Id::new(4),
            Id::new(6),
            RequestReactionType::Unicode { name: "❤" },
        )
        .after(Id::new(10))
        .limit(2);
        for count in 1..=2 {
            assert!(matches!(
                pages.next_page().await,
                Err(ReactionPagesError::InvalidPage)
            ));
            assert_eq!(mock.count(&route), count);
        }
    }
}
