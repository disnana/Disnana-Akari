#![doc = include_str!("../docs/api.md")]

mod commands;

use std::{env, error::Error};

use twilight_gateway::{Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt as _};
use twilight_http::Client;
use twilight_model::channel::{Message, message::AllowedMentions};

pub use commands::{CommandContext, Commands};

/// Akari とメッセージハンドラーで使う結果型です。
pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

/// テスト用サーバー向けの、単一シャードの Discord Bot です。
pub struct Bot {
    token: String,
}

impl Bot {
    /// 渡された Discord Bot トークンで Bot を作ります。
    ///
    /// # エラー
    ///
    /// トークンが空、または空白だけの場合は失敗します。
    pub fn new(token: &str) -> Result<Self> {
        let token = token.trim();
        if token.is_empty() {
            return Err("Discord Bot トークンが空です".into());
        }

        Ok(Self {
            token: token.to_owned(),
        })
    }

    /// `DISCORD_TOKEN` 環境変数から Bot を作ります。
    ///
    /// # エラー
    ///
    /// 環境変数が未設定、空、または Unicode 文字列として読み取れない場合に失敗します。
    pub fn from_env() -> Result<Self> {
        let token = env::var("DISCORD_TOKEN")
            .map_err(|_| "DISCORD_TOKEN に Discord Bot トークンを設定してください")?;
        Self::new(&token)
            .map_err(|_| "DISCORD_TOKEN に Discord Bot トークンを設定してください".into())
    }

    /// Discord に接続し、登録されたテキストコマンドを実行します。
    ///
    /// 接続とエラー処理は [`Self::run`] と同じです。
    ///
    /// # エラー
    ///
    /// Discord Gateway のイベントストリームが終了した場合に失敗します。
    pub async fn run_commands(self, commands: Commands) -> Result<()> {
        self.run(async |ctx| commands.dispatch(ctx).await).await
    }

    /// Discord に接続し、人がサーバーで送った新着メッセージを順に処理します。
    ///
    /// Tokio ランタイム内で呼び出してください。Bot や Webhook によるメッセージは
    /// ハンドラーに渡しません。ハンドラーのエラーは標準エラー出力に記録して処理を続けます。
    /// ハンドラーが待機している間は Gateway のイベント受信も待つため、処理は短く保ちます。
    ///
    /// # エラー
    ///
    /// Gateway が再接続できない状態でイベントストリームを閉じた場合に失敗します。
    pub async fn run(
        self,
        on_message: impl for<'a> AsyncFn(Context<'a>) -> Result<()>,
    ) -> Result<()> {
        // 既にプロバイダーが登録されている場合は、アプリ側の選択を使う。
        let _ = rustls::crypto::ring::default_provider().install_default();

        let intents = Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT;
        let mut shard = Shard::new(ShardId::ONE, self.token.clone(), intents);
        let http = Client::new(self.token);
        let events = EventTypeFlags::READY | EventTypeFlags::MESSAGE_CREATE;

        while let Some(event) = shard.next_event(events).await {
            match event {
                Ok(Event::Ready(_)) => println!("Discord に接続しました"),
                Ok(Event::MessageCreate(message))
                    if !message.author.bot && message.webhook_id.is_none() =>
                {
                    let context = Context {
                        http: &http,
                        message: &message.message,
                    };
                    if let Err(error) = on_message(context).await {
                        eprintln!("メッセージ処理に失敗しました ({error})");
                    }
                }
                Ok(Event::GatewayClose(Some(frame))) => {
                    eprintln!("Discord Gateway が接続を閉じました (コード {})", frame.code);
                }
                Ok(_) => {}
                Err(error) => eprintln!("Gateway の受信エラー ({error})"),
            }
        }

        Err("Discord Gateway の接続が終了しました".into())
    }
}

/// ハンドラーに渡される、受信メッセージと返信先の情報です。
pub struct Context<'a> {
    http: &'a Client,
    message: &'a Message,
}

impl<'a> Context<'a> {
    /// 受信した Twilight の [`Message`] を借ります。
    pub fn message(&self) -> &'a Message {
        self.message
    }

    /// 受信したメッセージに返信します。返信元の著者や本文中のメンションへの通知を抑止します。
    ///
    /// # エラー
    ///
    /// 本文が Discord の制約に合わない場合や、権限不足などで送信が失敗した場合にエラーを返します。
    pub async fn reply(&self, content: &str) -> Result<()> {
        self.http
            .create_message(self.message.channel_id)
            .content(content)
            .reply(self.message.id)
            .allowed_mentions(Some(&AllowedMentions::default()))
            .await?;

        Ok(())
    }
}
