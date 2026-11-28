use std::{
    fmt::Write,
    num::{NonZeroU8, NonZeroU16},
};

use serde::Deserialize;
use twilight_http::{
    Client,
    request::{Method, RequestBuilder},
};
use twilight_model::{
    id::{
        Id,
        marker::{ChannelMarker, MessageMarker, UserMarker},
    },
    user::User,
};

use crate::rest_pagination::UserPageCursor;

/// 一つの投票の選択肢に投票したユーザーを、ID の小さい順に読み取ります。
///
/// ページを要求するまで通信せず、1 回の取得は最大 100 件です。
#[must_use]
pub struct PollVoterPages<'a> {
    http: &'a Client,
    channel: Id<ChannelMarker>,
    message: Id<MessageMarker>,
    answer: NonZeroU8,
    cursor: UserPageCursor,
}

impl<'a> PollVoterPages<'a> {
    /// チャンネル、投票のあるメッセージ、選択肢の ID を指定します。
    pub const fn new(
        http: &'a Client,
        channel: Id<ChannelMarker>,
        message: Id<MessageMarker>,
        answer: NonZeroU8,
    ) -> Self {
        Self {
            http,
            channel,
            message,
            answer,
            cursor: UserPageCursor::new(),
        }
    }

    /// 指定したユーザー ID より後から取得します。その ID は含めません。
    pub const fn after(mut self, user: Id<UserMarker>) -> Self {
        self.cursor.after = Some(user);
        self
    }

    /// 取得する総件数を制限します。ゼロの場合はリクエストを送りません。
    pub const fn limit(mut self, limit: usize) -> Self {
        self.cursor.remaining = Some(limit);
        self
    }

    /// 1 ページの取得上限を指定します。100 を超える値は 100 にします。
    pub fn page_size(mut self, size: NonZeroU16) -> Self {
        self.cursor.set_page_size(size);
        self
    }

    /// 次のページを取得します。すべて読み終えると `Ok(None)` です。
    ///
    /// # エラー
    ///
    /// REST やデコードに失敗した場合、重複、カーソル以前の ID、上限超過の応答を受けた場合に
    /// 失敗します。エラー時は同じ位置から再試行できます。
    pub async fn next_page(&mut self) -> Result<Option<Vec<User>>, PollVoterPagesError> {
        let Some(limit) = self.cursor.request_size() else {
            return Ok(None);
        };
        // Twilight 0.18 の GetAnswerVoters はパラメーターの手前に「??」を出力する。
        // 数字だけの固定経路を組み立て、認証とレート制限は同じクライアントを通す。
        let mut path = format!(
            "channels/{}/polls/{}/answers/{}?limit={limit}",
            self.channel, self.message, self.answer
        );
        if let Some(after) = self.cursor.after {
            let _ = write!(path, "&after={after}");
        }
        let request = RequestBuilder::raw(Method::Get, path).build()?;
        let payload = self
            .http
            .request::<VoterPage>(request)
            .await?
            .model()
            .await?;
        self.cursor
            .accept(payload.users, limit)
            .map_err(|_| PollVoterPagesError::InvalidPage)
    }
}

#[derive(Deserialize)]
struct VoterPage {
    users: Vec<User>,
}

/// 投票したユーザー一覧のページ取得に失敗した理由です。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PollVoterPagesError {
    /// 取得リクエストが失敗しました。
    #[error("投票したユーザーの一覧を取得できませんでした")]
    Http(#[from] twilight_http::Error),
    /// Discord の応答本文を読み取れませんでした。
    #[error("投票したユーザーの一覧を読み取れませんでした")]
    HttpBody(#[from] twilight_http::response::DeserializeBodyError),
    /// 応答に重複した ID、範囲外の ID、または上限を超える件数が含まれます。
    #[error("投票したユーザー一覧のページが無効です")]
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

    async fn respond(listener: &TcpListener, query: &str, ids: &[u64]) {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = BufReader::new(socket);
        let mut line = String::new();
        socket.read_line(&mut line).await.unwrap();
        assert_eq!(
            line.trim_end(),
            format!("GET /api/v10/channels/4/polls/6/answers/2?{query} HTTP/1.1")
        );
        loop {
            line.clear();
            assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
            if line == "\r\n" {
                break;
            }
        }
        let users: Vec<_> = ids
            .iter()
            .map(|id| {
                serde_json::json!({
                    "id":id.to_string(), "username":"fixture", "discriminator":"0"
                })
            })
            .collect();
        let body = serde_json::json!({"users": users}).to_string();
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

    #[tokio::test]
    async fn advances_only_after_valid_pages_and_encodes_single_question_mark() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http = Client::builder()
            .proxy(listener.local_addr().unwrap().to_string(), true)
            .build();
        let mut pages =
            PollVoterPages::new(&http, Id::new(4), Id::new(6), NonZeroU8::new(2).unwrap())
                .after(Id::new(7))
                .limit(3)
                .page_size(NonZeroU16::new(2).unwrap());
        let server = async {
            respond(&listener, "limit=2&after=7", &[7]).await;
            respond(&listener, "limit=2&after=7", &[9, 8]).await;
            respond(&listener, "limit=1&after=9", &[10]).await;
        };
        let read = async {
            assert!(matches!(
                pages.next_page().await,
                Err(PollVoterPagesError::InvalidPage)
            ));
            let first = pages.next_page().await.unwrap().unwrap();
            assert!(first.iter().map(|user| user.id.get()).eq([8, 9]));
            assert_eq!(pages.next_page().await.unwrap().unwrap()[0].id.get(), 10);
            assert!(pages.next_page().await.unwrap().is_none());
        };
        timeout(Duration::from_secs(5), async {
            tokio::join!(server, read);
        })
        .await
        .unwrap();
    }
}
