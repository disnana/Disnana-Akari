use twilight_http::{
    Client,
    error::ErrorType,
    request::{Method, RequestBuilder},
    response::DeserializeBodyError,
};
use twilight_model::{
    channel::thread::ThreadMember,
    id::{
        Id,
        marker::{ChannelMarker, UserMarker},
    },
};

/// 指定したユーザーのスレッド参加情報を Discord REST から取得します。
///
/// Discord にサーバーメンバー情報も要求します。Discord が 404 を返した場合は `None` です。
/// 取得した情報はキャッシュに保存しません。
///
/// # エラー
///
/// 404 以外の通信失敗、応答の読み取り失敗、またはスレッド ID やユーザー ID が
/// 異なる応答を受け取った場合に失敗します。
pub async fn fetch_thread_member(
    http: &Client,
    thread: Id<ChannelMarker>,
    user: Id<UserMarker>,
) -> Result<Option<ThreadMember>, ThreadMemberLookupError> {
    // Twilight 0.18 の単件取得には with_member を指定するビルダーがない。
    let request = RequestBuilder::raw(
        Method::Get,
        format!("channels/{thread}/thread-members/{user}?with_member=true"),
    )
    .build()?;
    let response = match http.request::<ThreadMember>(request).await {
        Ok(response) => response,
        Err(error) if matches!(error.kind(), ErrorType::Response { status, .. } if status.get() == 404) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    let member = response.model().await?;
    if member.id != Some(thread)
        || member.user_id != Some(user)
        || member
            .member
            .as_ref()
            .is_some_and(|member| member.user.id != user)
    {
        return Err(ThreadMemberLookupError::MismatchedResponse);
    }
    Ok(Some(member))
}

/// スレッド参加情報の取得に失敗した理由です。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ThreadMemberLookupError {
    /// Discord へのリクエストが失敗しました。
    #[error("Discord のスレッド参加情報を取得できませんでした")]
    Http(#[from] twilight_http::Error),
    /// Discord の応答をスレッド参加情報として読めませんでした。
    #[error("Discord のスレッド参加情報を読み取れませんでした")]
    HttpBody(#[from] DeserializeBodyError),
    /// 応答に必要な ID がないか、要求した ID が一致しません。
    #[error("Discord の応答が要求したスレッドまたはユーザーと一致しません")]
    MismatchedResponse,
}
