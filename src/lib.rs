#![doc = include_str!("../docs/api.md")]

mod commands;
mod cooldown;
mod events;
mod hybrid;
mod interactions;
mod slash;
mod timing;
pub mod ui;
mod work_queue;

use std::{env, error::Error, sync::Arc};

use tokio::sync::mpsc;
use twilight_gateway::{Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt as _};
use twilight_http::Client;
use twilight_model::{
    application::interaction::{Interaction, InteractionType},
    channel::{
        Message,
        message::{AllowedMentions, MessageFlags},
    },
};

pub use commands::{CommandContext, Commands};
pub use cooldown::Cooldown;
pub use events::{Events, Ready};
pub use hybrid::{CommandSource, HybridContext, register_hybrid};
pub use interactions::{
    ButtonContext, ChannelSelectContext, EntitySelectContext, Mentionable,
    MentionableSelectContext, ModalContext, RoleSelectContext, SelectContext, UserSelectContext,
};
pub use slash::{SlashCommands, SlashContext};
pub use timing::Timing;
pub use twilight_http as http;
pub use twilight_model as model;

use timing::Pending;

const INTERACTION_WORKERS: usize = 8;
const TEXT_WORKERS: usize = 8;
const BUSY_WORKERS: usize = 8;
const GATEWAY_QUEUE_CAPACITY: usize = 64;
const BUSY_MESSAGE: &str = "現在混雑しています。少し待ってからもう一度お試しください。";

type NoMessageHandler = fn(Context<'_>) -> std::future::Ready<Result<()>>;

/// Akari とメッセージハンドラーで使う結果型です。
pub type Result<T>   = std::result::Result<T, Box<dyn Error + Send + Sync>>;

/// テスト用サーバー向けの、単一シャードの Discord Bot です。
pub struct Bot {
    token: String,
    events: Events,
    slash: Option<SlashCommands>,
    intents: Option<Intents>,
    additional_intents: Intents,
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
            additional_intents: Intents::empty(),
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
    /// テキストコマンドを使う場合は `GUILD_MESSAGES` と `MESSAGE_CONTENT` を含めてください。
    pub fn with_intents(mut self, intents: Intents) -> Self {
        self.intents = Some(intents);
        self
    }

    /// 自動で算出した Intent、または [`Self::with_intents`] の値に Intent を追加します。
    /// 複数回指定すると和集合になり、`with_intents` との呼び出し順に関わらず適用します。
    pub fn add_intents(mut self, intents: Intents) -> Self {
        self.additional_intents |= intents;
        self
    }

    /// イベントハンドラーやスラッシュコマンドを設定した Bot を起動します。
    ///
    /// # エラー
    ///
    /// スラッシュコマンドの登録に失敗した場合や、Gateway が終了した場合に失敗します。
    pub async fn start(self) -> Result<()> {
        self.connect(None::<NoMessageHandler>, None).await
    }

    /// Discord に接続し、登録されたテキストコマンドを実行します。
    ///
    /// コマンド関数が失敗したときは、呼び出したメッセージへエラーを返信します。
    /// 複数のコマンドは同時進行でき、受信順に処理するイベントハンドラーから独立しています。
    ///
    /// # エラー
    ///
    /// スラッシュコマンドの登録に失敗した場合や、Gateway が終了した場合に失敗します。
    pub async fn run_commands(self, commands: Commands) -> Result<()> {
        self.connect(None::<NoMessageHandler>, Some(commands)).await
    }

    /// Discord に接続し、人がサーバーで送った新着メッセージを順に処理します。
    ///
    /// Tokio ランタイム内で呼び出してください。Bot や Webhook によるメッセージは
    /// ハンドラーに渡しません。ハンドラーのエラーは標準エラー出力に記録して処理を続けます。
    /// 受信とハンドラーは協調的に進みます。ブロッキング処理や長い計算は分離してください。
    ///
    /// # エラー
    ///
    /// スラッシュコマンドの登録に失敗した場合や、Gateway が終了した場合に失敗します。
    pub async fn run(
        self,
        on_message: impl for<'a> AsyncFn(Context<'a>) -> Result<()>,
    ) -> Result<()> {
        self.connect(Some(on_message), None).await
    }

    async fn connect(
        self,
        on_message: Option<impl for<'a> AsyncFn(Context<'a>) -> Result<()>>,
        text_commands: Option<Commands>,
    ) -> Result<()> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let Bot {
            token,
            events,
            slash,
            intents,
            additional_intents,
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

        if events.interaction_handlers.has_handlers() {
            event_types |= EventTypeFlags::INTERACTION_CREATE;
        }

        let observe_messages = on_message.is_some() || events.has_message_handlers();
        let text_commands = text_commands.as_ref();
        if observe_messages || text_commands.is_some() {
            default_intents |= Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT;
            event_types |= EventTypeFlags::MESSAGE_CREATE;
        }

        let intents = intents.unwrap_or(default_intents) | additional_intents;
        let mut shard = Shard::new(ShardId::ONE, token, intents);
        let (event_tx, mut event_queue) = mpsc::channel(GATEWAY_QUEUE_CAPACITY);
        let (text_tx, text_queue) = mpsc::channel::<Pending<Arc<Message>>>(GATEWAY_QUEUE_CAPACITY);
        let (text_busy_tx, text_busy_queue) = mpsc::channel(GATEWAY_QUEUE_CAPACITY);
        let (interactions, interaction_queue) =
            mpsc::channel::<Pending<Interaction>>(GATEWAY_QUEUE_CAPACITY);
        let (busy, busy_queue) = mpsc::channel(GATEWAY_QUEUE_CAPACITY);
        let slash = slash.as_ref();

        let handle_events = async {
            while let Some(event) = event_queue.recv().await {
                match event {
                    MessageEvent::Ready(ready) => {
                        events.ready(&ready).await;
                    }
                    MessageEvent::Message(pending) => {
                        let (message, timing) = pending.begin();
                        if let Some(handler) = &on_message {
                            let context = Context {
                                http: &http,
                                message: &message,
                                timing,
                            };
                            if let Err(error) = handler(context).await {
                                eprintln!("メッセージ処理に失敗しました ({error})");
                            }
                        }
                        events.message(&message).await;
                    }
                }
            }
        };

        let handle_text = work_queue::dispatch(
            text_queue,
            TEXT_WORKERS,
            |pending| async {
                let (message, timing) = Pending::begin(pending);
                if let Some(commands) = text_commands {
                    commands
                        .dispatch_with_feedback(Context {
                            http: &http,
                            message: &message,
                            timing,
                        })
                        .await;
                }
            },
            |pending| queue_text_busy(&text_busy_tx, pending),
        );
        let handle_text_busy =
            work_queue::respond(text_busy_queue, BUSY_WORKERS, |pending| async {
                let (message, timing) = Pending::begin(pending);
                let context = Context {
                    http: &http,
                    message: &message,
                    timing,
                };
                if let Err(error) = context.reply(BUSY_MESSAGE).await {
                    eprintln!("テキストコマンドの混雑応答に失敗しました ({error})");
                }
            });

        let handle_interactions = work_queue::dispatch(
            interaction_queue,
            INTERACTION_WORKERS,
            |pending| async {
                let (interaction, timing) = Pending::begin(pending);
                if matches!(
                    interaction.kind,
                    InteractionType::MessageComponent | InteractionType::ModalSubmit
                ) {
                    events
                        .interaction_handlers
                        .dispatch(&http, &interaction, timing)
                        .await;
                } else if let Some(commands) = slash {
                    commands.dispatch(&http, &interaction, timing).await;
                }
            },
            |pending| queue_busy_response(&busy, pending.into_event()),
        );
        let handle_busy = work_queue::respond(busy_queue, BUSY_WORKERS, |interaction| {
            let http = &http;
            async move {
                if slash::send_initial_message(http, &interaction, BUSY_MESSAGE, true)
                    .await
                    .is_err()
                {
                    eprintln!("interaction の混雑応答に失敗しました");
                }
            }
        });

        let receive = async {
            while let Some(event) = shard.next_event(event_types).await {
                let gateway_rtt = shard.latency().recent().first().copied();
                match event {
                    Ok(Event::Ready(ready)) => {
                        println!("Discord に接続しました");
                        if event_tx
                            .try_send(MessageEvent::Ready(Box::new(ready)))
                            .is_err()
                        {
                            eprintln!("受信キューが満杯のため READY を処理できませんでした");
                        }
                    }
                    Ok(Event::MessageCreate(message))
                        if !message.author.bot && message.webhook_id.is_none() =>
                    {
                        let run_command = text_commands
                            .is_some_and(|commands| commands.matches_message(&message.content));
                        if !observe_messages && !run_command {
                            continue;
                        }
                        let message = Arc::new(message.message);
                        if observe_messages
                            && event_tx
                                .try_send(MessageEvent::Message(Pending::new(
                                    Arc::clone(&message),
                                    gateway_rtt,
                                )))
                                .is_err()
                        {
                            eprintln!(
                                "イベントキューが満杯のため MESSAGE_CREATE を処理できませんでした"
                            );
                        }
                        if run_command
                            && let Err(error) = text_tx.try_send(Pending::new(message, gateway_rtt))
                        {
                            queue_text_busy(&text_busy_tx, error.into_inner());
                        }
                    }
                    Ok(Event::InteractionCreate(interaction))
                        if events.interaction_handlers.accepts(&interaction.0)
                            || slash.is_some_and(|commands| commands.accepts(&interaction.0)) =>
                    {
                        if let Err(error) =
                            interactions.try_send(Pending::new(interaction.0, gateway_rtt))
                        {
                            queue_busy_response(&busy, error.into_inner().into_event());
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
            () = handle_events => {},
            () = handle_text => {},
            () = handle_text_busy => {},
            () = handle_interactions => {},
            () = handle_busy => {},
        }

        Err("Discord Gateway の接続が終了しました".into())
    }
}

enum MessageEvent {
    Ready(Box<Ready>),
    Message(Pending<Arc<Message>>),
}

fn queue_text_busy(queue: &mpsc::Sender<Pending<Arc<Message>>>, pending: Pending<Arc<Message>>) {
    if queue.try_send(pending).is_err() {
        eprintln!("混雑応答のキューが満杯のためテキストコマンドに応答できませんでした");
    }
}

fn queue_busy_response(queue: &mpsc::Sender<Interaction>, interaction: Interaction) {
    if queue.try_send(interaction).is_err() {
        eprintln!("混雑応答のキューが満杯のため INTERACTION_CREATE に応答できませんでした");
    }
}

/// ハンドラーに渡される、受信メッセージと返信先の情報です。
#[derive(Clone, Copy)]
pub struct Context<'a> {
    http: &'a Client,
    message: &'a Message,
    timing: Timing,
}

impl<'a> Context<'a> {
    /// Akari が使用している Twilight の REST クライアントを借ります。
    pub fn http(&self) -> &'a Client {
        self.http
    }

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

    /// 元のメッセージに Embed やボタンを付けて返信します。
    ///
    /// # エラー
    ///
    /// メッセージの構成が無効、または Discord が送信を受け付けない場合に失敗します。
    pub async fn reply_ui(&self, message: &ui::UiMessage) -> Result<()> {
        message.validate()?;
        let mentions = AllowedMentions::default();
        let mut request = self
            .http
            .create_message(self.message.channel_id)
            .reply(self.message.id)
            .allowed_mentions(Some(&mentions));
        if let Some(content) = &message.content {
            request = request.content(content);
        }
        if !message.embeds.is_empty() {
            request = request.embeds(&message.embeds);
        }
        if !message.components.is_empty() {
            request = request.components(&message.components);
        }
        request.await?;
        Ok(())
    }

    /// Components V2 のメッセージを元の投稿への返信として送ります。
    ///
    /// # エラー
    /// 無効なコンポーネント、または Discord が送信を受け付けない場合に失敗します。
    pub async fn reply_v2(&self, message: &ui::ComponentsV2) -> Result<()> {
        message.validate()?;
        self.http
            .create_message(self.message.channel_id)
            .reply(self.message.id)
            .allowed_mentions(Some(&AllowedMentions::default()))
            .flags(MessageFlags::IS_COMPONENTS_V2)
            .components(&message.components)
            .await?;
        Ok(())
    }
}
