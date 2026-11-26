#![doc = include_str!("../docs/api.md")]

mod commands;
mod events;
mod hybrid;
mod slash;
mod timing;

use std::{env, error::Error};

use tokio::sync::mpsc;
use twilight_gateway::{Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt as _};
use twilight_http::Client;
use twilight_model::channel::{Message, message::AllowedMentions};

pub use commands::{CommandContext, Commands};
pub use events::{Events, Ready};
pub use hybrid::{CommandSource, HybridContext, register_hybrid};
pub use slash::{SlashCommands, SlashContext};
pub use timing::Timing;
pub use twilight_model as model;

use timing::Pending;

/// Akari とメッセージハンドラーで使う結果型です。
pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

/// テスト用サーバー向けの、単一シャードの Discord Bot です。
pub struct Bot {
    token: String,
    events: Events,
    slash: Option<SlashCommands>,
    intents: Option<Intents>,
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
            events: Events::default(),
            slash: None,
            intents: None,
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

    /// Gateway イベントを受け取るハンドラー一覧を設定します。
    pub fn with_events(mut self, events: Events) -> Self {
        self.events = events;
        self
    }

    /// 指定されたサーバーでスラッシュコマンドを有効にします。
    pub fn with_slash_commands(mut self, commands: SlashCommands) -> Self {
        self.slash = Some(commands);
        self
    }

    /// 自動で算出する Gateway Intent を、指定したビット集合で置き換えます。
    pub fn with_intents(mut self, intents: Intents) -> Self {
        self.intents = Some(intents);
        self
    }

    /// イベントハンドラーやスラッシュコマンドを設定した Bot を起動します。
    ///
    /// # エラー
    ///
    /// スラッシュコマンドの登録に失敗した場合や、Gateway が終了した場合に失敗します。
    pub async fn start(self) -> Result<()> {
        self.connect(None::<fn(Context<'_>) -> std::future::Ready<Result<()>>>)
            .await
    }

    /// Discord に接続し、登録されたテキストコマンドを実行します。
    ///
    /// 接続とエラー処理は [`Self::run`] と同じです。
    ///
    /// # エラー
    ///
    /// スラッシュコマンドの登録に失敗した場合や、Gateway が終了した場合に失敗します。
    pub async fn run_commands(self, commands: Commands) -> Result<()> {
        self.run(async |ctx| commands.dispatch(ctx).await).await
    }

    /// Discord に接続し、人がサーバーで送った新着メッセージを順に処理します。
    ///
    /// Tokio ランタイム内で呼び出してください。Bot や Webhook によるメッセージは
    /// ハンドラーに渡しません。ハンドラーのエラーは標準エラー出力に記録して処理を続けます。
    /// 受信処理とハンドラー処理は並行して進みます。キュー満杯時は新着イベントを破棄します。
    ///
    /// # エラー
    ///
    /// スラッシュコマンドの登録に失敗した場合や、Gateway が終了した場合に失敗します。
    pub async fn run(
        self,
        on_message: impl for<'a> AsyncFn(Context<'a>) -> Result<()>,
    ) -> Result<()> {
        self.connect(Some(on_message)).await
    }

    async fn connect(
        self,
        on_message: Option<impl for<'a> AsyncFn(Context<'a>) -> Result<()>>,
    ) -> Result<()> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let Bot {
            token,
            events,
            slash,
            intents,
        } = self;
        let http = Client::new(token.clone());
        let mut event_types = EventTypeFlags::READY;
        let mut default_intents = Intents::empty();

        let slash = match slash {
            Some(commands) => {
                event_types |= EventTypeFlags::INTERACTION_CREATE;
                Some(commands.sync(&http).await?)
            }
            None => None,
        };

        if on_message.is_some() || events.has_message_handlers() {
            default_intents |= Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT;
            event_types |= EventTypeFlags::MESSAGE_CREATE;
        }

        let mut shard = Shard::new(ShardId::ONE, token, intents.unwrap_or(default_intents));
        let (messages, mut message_queue) = mpsc::channel(64);
        let (interactions, mut interaction_queue) = mpsc::channel(64);

        let handle_messages = async {
            while let Some(pending) = message_queue.recv().await {
                let (event, timing) = Pending::begin(pending);
                match event {
                    Event::Ready(ready) => {
                        events.ready(&ready).await;
                    }
                    Event::MessageCreate(message) => {
                        if let Some(handler) = &on_message {
                            let context = Context {
                                http: &http,
                                message: &message.message,
                                timing,
                            };
                            if let Err(error) = handler(context).await {
                                eprintln!("メッセージ処理に失敗しました ({error})");
                            }
                        }
                        events.message(&message.message).await;
                    }
                    _ => {}
                }
            }
        };

        let handle_interactions = async {
            while let Some(pending) = interaction_queue.recv().await {
                let (interaction, timing) = Pending::begin(pending);
                if let Some(commands) = &slash {
                    commands.dispatch(&http, &interaction, timing).await;
                }
            }
        };

        let receive = async {
            while let Some(event) = shard.next_event(event_types).await {
                let gateway_rtt = shard.latency().recent().first().copied();
                match event {
                    Ok(event @ Event::Ready(_)) => {
                        println!("Discord に接続しました");
                        if messages.try_send(Pending::new(event, gateway_rtt)).is_err() {
                            eprintln!("受信キューが満杯のため READY を処理できませんでした");
                        }
                    }
                    Ok(Event::MessageCreate(message))
                        if !message.author.bot && message.webhook_id.is_none() =>
                    {
                        let event = Event::MessageCreate(message);
                        if messages.try_send(Pending::new(event, gateway_rtt)).is_err() {
                            eprintln!(
                                "受信キューが満杯のため MESSAGE_CREATE を処理できませんでした"
                            );
                        }
                    }
                    Ok(Event::InteractionCreate(interaction)) => {
                        if interactions
                            .try_send(Pending::new(interaction.0, gateway_rtt))
                            .is_err()
                        {
                            eprintln!(
                                "受信キューが満杯のため INTERACTION_CREATE を処理できませんでした"
                            );
                        }
                    }
                    Ok(Event::GatewayClose(Some(frame))) => {
                        eprintln!("Discord Gateway が接続を閉じました (コード {})", frame.code);
                    }
                    Ok(_) => {}
                    Err(error) => eprintln!("Gateway の受信エラー ({error})"),
                }
            }
        };

        // どのワーカーも Gateway 受信を待たせない。切断で終了したら保留中の処理も中止する。
        tokio::select! {
            () = receive => {},
            () = handle_messages => {},
            () = handle_interactions => {},
        }

        Err("Discord Gateway の接続が終了しました".into())
    }
}

/// ハンドラーに渡される、受信メッセージと返信先の情報です。
pub struct Context<'a> {
    http: &'a Client,
    message: &'a Message,
    timing: Timing,
}

impl<'a> Context<'a> {
    /// Gateway の往復時間と、このイベントの処理時間を確認します。
    pub fn timing(&self) -> Timing {
        self.timing
    }

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
