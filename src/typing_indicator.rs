use std::{future::Future, time::Duration};

use twilight_http::Client;
use twilight_model::id::{Id, marker::ChannelMarker};

const REFRESH_INTERVAL: Duration = Duration::from_secs(5);

/// チャンネルに Bot の入力中表示を出します。
///
/// ひとつのリクエストによる表示は Discord 側で期限切れになります。
#[must_use]
pub struct TypingIndicator<'http> {
    http: &'http Client,
    channel: Id<ChannelMarker>,
}

impl<'http> TypingIndicator<'http> {
    /// 通信せずに入力中表示を準備します。
    pub const fn new(http: &'http Client, channel: Id<ChannelMarker>) -> Self {
        Self { http, channel }
    }

    /// 入力中表示を一度送ります。
    ///
    /// # エラー
    ///
    /// Discord がリクエストを拒否した場合や通信に失敗した場合にエラーを返します。
    pub async fn trigger(&self) -> Result<(), twilight_http::Error> {
        self.http.create_typing_trigger(self.channel).await?;
        Ok(())
    }

    /// 仕事と並行して入力中表示を送り、成功した送信から5秒後に更新します。
    ///
    /// 表示の送信に失敗すると更新を止め、仕事の完了を待ちます。仕事がすぐに
    /// 完了した場合は表示を送りません。完了やキャンセル後に更新は残りません。
    pub async fn while_running<T>(&self, work: impl Future<Output = T>) -> T {
        tokio::pin!(work);
        tokio::select! {
            biased;
            output = &mut work => output,
            () = self.refresh() => work.await,
        }
    }

    async fn refresh(&self) {
        while self.trigger().await.is_ok() {
            tokio::time::sleep(REFRESH_INTERVAL).await;
        }
    }
}

#[cfg(test)]
mod tests;
