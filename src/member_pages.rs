use std::num::NonZeroU16;

use twilight_http::Client;
use twilight_model::{
    guild::Member,
    id::{
        Id,
        marker::{GuildMarker, UserMarker},
    },
};

use crate::rest_pagination::advance_after;

const MAX_PAGE_SIZE: u16 = 1_000;

/// Discord REST でサーバーのメンバーを必要なページだけ取得します。
///
/// Discord 側で特権 Intent の `GUILD_MEMBERS` が必要です。取得した情報はキャッシュへ保存しません。
/// ページ間にメンバーが参加・退出することがあるため、全ページで同じ時点の一覧は保証されません。
pub struct MemberPages<'http> {
    http: &'http Client,
    guild: Id<GuildMarker>,
    page_size: u16,
    after: Option<Id<UserMarker>>,
    remaining: Option<usize>,
    exhausted: bool,
}

impl<'http> MemberPages<'http> {
    /// 通信せずに取得を準備します。1ページあたりの上限は既定で1,000件です。
    /// 総件数を制限する場合は [`Self::limit`] を指定します。
    #[must_use]
    pub fn new(http: &'http Client, guild: Id<GuildMarker>) -> Self {
        Self {
            http,
            guild,
            page_size: MAX_PAGE_SIZE,
            after: None,
            remaining: None,
            exhausted: false,
        }
    }

    /// 指定したユーザー ID より後から取得します。その ID 自体は含みません。
    #[must_use]
    pub fn after(mut self, user: Id<UserMarker>) -> Self {
        self.after = Some(user);
        self
    }

    /// 返すメンバーの総件数の上限を設定します。0件の場合はリクエストを送りません。
    #[must_use]
    pub fn limit(mut self, limit: usize) -> Self {
        self.remaining = Some(limit);
        self
    }

    /// リクエスト1回あたりの上限を変更します。1,000件を超える値は1,000件にします。
    #[must_use]
    pub fn page_size(mut self, size: NonZeroU16) -> Self {
        self.page_size = size.get().min(MAX_PAGE_SIZE);
        self
    }

    /// 次の1ページをユーザー ID の昇順で返します。終了した場合は `None` です。
    ///
    /// # エラー
    ///
    /// REST リクエストや応答の読み取りに失敗した場合はエラーを返します。
    /// 直前のカーソル以前の ID、重複した ID、または要求数を超える応答を拒否します。
    /// エラー時にカーソルは進みません。
    pub async fn next_page(&mut self) -> Result<Option<Vec<Member>>, MemberPagesError> {
        let limit = self
            .remaining
            .and_then(|remaining| u16::try_from(remaining).ok())
            .map_or(self.page_size, |remaining| self.page_size.min(remaining));
        if self.exhausted || limit == 0 {
            return Ok(None);
        }

        let mut request = self.http.guild_members(self.guild).limit(limit);
        if let Some(after) = self.after {
            request = request.after(after);
        }
        let mut members = request.await?.models().await?;
        let limit = usize::from(limit);
        let after = advance_after(&mut members, limit, self.after, |member| member.user.id)
            .ok_or(MemberPagesError::InvalidPage)?;
        self.exhausted = members.len() < limit;
        if let Some(remaining) = &mut self.remaining {
            *remaining -= members.len();
        }
        self.after = after;
        Ok((!members.is_empty()).then_some(members))
    }
}

/// REST でサーバーのメンバーをページ取得できなかった理由です。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MemberPagesError {
    /// Discord へのリクエストが失敗しました。
    #[error("メンバー一覧を取得できませんでした")]
    Http(#[from] twilight_http::Error),
    /// Discord の応答をメンバー一覧として読み取れませんでした。
    #[error("メンバー一覧の応答を読み取れませんでした")]
    HttpBody(#[from] twilight_http::response::DeserializeBodyError),
    /// 応答に要求上限を超える項目、重複 ID、またはカーソル以前の ID が含まれます。
    #[error("メンバー一覧のページが無効です")]
    InvalidPage,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::TcpListener,
    };

    use super::*;

    async fn reply(listener: &TcpListener, path: &str, ids: &[u64]) {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = BufReader::new(socket);
        let mut line = String::new();
        socket.read_line(&mut line).await.unwrap();
        assert_eq!(line.trim_end(), format!("GET {path} HTTP/1.1"));
        while !line.trim_end().is_empty() {
            line.clear();
            assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
        }
        let members: Vec<_> = ids
            .iter()
            .map(|id| {
                json!({
                    "user": {"id": id.to_string(), "username": "member", "discriminator": "0"},
                    "deaf": false, "mute": false, "flags": 0, "roles": []
                })
            })
            .collect();
        let body = serde_json::to_string(&members).unwrap();
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

    async fn local() -> (TcpListener, Client) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http = Client::builder()
            .proxy(listener.local_addr().unwrap().to_string(), true)
            .build();
        (listener, http)
    }

    fn ids(members: Vec<Member>) -> Vec<u64> {
        members
            .into_iter()
            .map(|member| member.user.id.get())
            .collect()
    }

    #[tokio::test]
    async fn uses_a_lazy_exclusive_cursor_and_a_total_limit() {
        let (listener, http) = local().await;
        let mut pages = MemberPages::new(&http, Id::new(2))
            .after(Id::new(3))
            .page_size(NonZeroU16::new(2).unwrap())
            .limit(3);
        let serve = async {
            reply(
                &listener,
                "/api/v10/guilds/2/members?after=3&limit=2",
                &[8, 5],
            )
            .await;
            reply(&listener, "/api/v10/guilds/2/members?after=8&limit=1", &[9]).await;
        };
        let read = async {
            assert_eq!(ids(pages.next_page().await.unwrap().unwrap()), [5, 8]);
            assert_eq!(ids(pages.next_page().await.unwrap().unwrap()), [9]);
            assert!(pages.next_page().await.unwrap().is_none());
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(serve, read);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn malformed_pages_cannot_advance_past_the_same_cursor() {
        let (listener, http) = local().await;
        let mut pages = MemberPages::new(&http, Id::new(2))
            .after(Id::new(7))
            .page_size(NonZeroU16::new(2).unwrap());
        let serve = async {
            let path = "/api/v10/guilds/2/members?after=7&limit=2";
            for ids in [[7, 8].as_slice(), &[8, 8], &[8, 9, 10], &[11]] {
                reply(&listener, path, ids).await;
            }
        };
        let read = async {
            for _ in 0..3 {
                assert!(matches!(
                    pages.next_page().await,
                    Err(MemberPagesError::InvalidPage)
                ));
            }
            assert_eq!(ids(pages.next_page().await.unwrap().unwrap()), [11]);
            assert!(pages.next_page().await.unwrap().is_none());
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(serve, read);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn zero_limit_ends_and_max_page_size_is_capped() {
        let http = Client::builder()
            .proxy("127.0.0.1:1".to_owned(), true)
            .build();
        let mut pages = MemberPages::new(&http, Id::new(2))
            .page_size(NonZeroU16::MAX)
            .limit(0);
        assert_eq!(pages.page_size, MAX_PAGE_SIZE);
        assert!(pages.next_page().await.unwrap().is_none());
    }
}
