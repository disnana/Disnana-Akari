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
    use std::time::Duration;

    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::TcpListener,
        time::timeout,
    };

    use super::*;

    const ROUTE: &str = "/api/v10/channels/4/messages/6/reactions/%E2%9D%A4";

    async fn local() -> (TcpListener, Client) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http = Client::builder()
            .proxy(listener.local_addr().unwrap().to_string(), true)
            .build();
        (listener, http)
    }

    async fn reply(listener: &TcpListener, query: &str, ids: &[u64]) {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = BufReader::new(socket);
        let mut line = String::new();
        socket.read_line(&mut line).await.unwrap();
        assert_eq!(line.trim_end(), format!("GET {ROUTE}?{query} HTTP/1.1"));
        while !line.trim_end().is_empty() {
            line.clear();
            assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
        }
        let users: Vec<_> = ids
            .iter()
            .map(|id| {
                serde_json::json!({
                    "id":id.to_string(), "username":"fixture", "discriminator":"0"
                })
            })
            .collect();
        let body = serde_json::to_string(&users).unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket
            .get_mut()
            .write_all(response.as_bytes())
            .await
            .unwrap();
    }

    fn pages(http: &Client) -> ReactionPages<'_> {
        ReactionPages::new(
            http,
            Id::new(4),
            Id::new(6),
            RequestReactionType::Unicode { name: "❤" },
        )
        .after(Id::new(10))
    }

    #[tokio::test]
    async fn pages_sort_each_response_and_forward_cursor_kind_and_total_limit() {
        let (listener, http) = local().await;
        let mut pages = pages(&http)
            .kind(ReactionType::Burst)
            .page_size(NonZeroU16::new(2).unwrap())
            .limit(3);
        let server = async {
            reply(&listener, "after=10&limit=2&type=1", &[12, 11]).await;
            reply(&listener, "after=12&limit=1&type=1", &[15]).await;
        };
        let read = async {
            let page = pages.next_page().await.unwrap().unwrap();
            assert!(page.iter().map(|user| user.id.get()).eq([11, 12]));
            assert_eq!(pages.next_page().await.unwrap().unwrap()[0].id.get(), 15);
            assert!(pages.next_page().await.unwrap().is_none());
        };
        timeout(Duration::from_secs(5), async {
            tokio::join!(server, read);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn invalid_pages_do_not_mutate_the_cursor_or_remaining_size() {
        let (listener, http) = local().await;
        let mut pages = pages(&http).limit(2);
        let server = async {
            for ids in [[10, 11], [11, 12]] {
                reply(&listener, "after=10&limit=2&type=0", &ids).await;
            }
        };
        let read = async {
            assert!(matches!(
                pages.next_page().await,
                Err(ReactionPagesError::InvalidPage)
            ));
            assert_eq!(pages.next_page().await.unwrap().unwrap().len(), 2);
            assert!(pages.next_page().await.unwrap().is_none());
        };
        timeout(Duration::from_secs(5), async {
            tokio::join!(server, read);
        })
        .await
        .unwrap();
    }
}
