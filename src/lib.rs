#![doc = include_str!("../docs/api.md")]

mod commands;
mod cooldown;
mod error;
mod events;
mod hooks;
mod hybrid;
mod interactions;
mod limits;
mod permissions;
mod slash;
mod timing;
pub mod ui;
mod work_queue;

use std::{env, fmt, sync::Arc, time::Duration};

use tokio::sync::mpsc;
use twilight_gateway::{Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt as _};
use twilight_http::Client;
use twilight_model::{
    application::interaction::{Interaction, InteractionType},
    channel::{
        Message,
        message::{AllowedMentions, MessageFlags},
    },
    guild::Permissions,
    id::{
        Id,
        marker::{GuildMarker, UserMarker},
    },
};

pub use commands::{CommandContext, Commands};
pub use cooldown::Cooldown;
pub use error::{
    CheckFailure, CommandSyncError, Error, HandlerError, HandlerKind, QueueKind, RegistrationError,
    ResponseStateError, SelectionError,
};
pub use events::{Events, Ready};
pub use hooks::{ErrorReport, ErrorSource, InfoEvent, Messages};
pub use hybrid::{CommandSource, HybridContext, register_hybrid};
pub use interactions::{
    ButtonContext, ChannelSelectContext, EntitySelectContext, Mentionable,
    MentionableSelectContext, ModalContext, RoleSelectContext, SelectContext, UserSelectContext,
};
pub use limits::Limits;
pub use slash::{CommandGroup, SlashCommands, SlashContext};
pub use timing::Timing;
pub use twilight_http as http;
pub use twilight_model as model;

use hooks::{Hooks, InfoReporter, Reporter};
use permissions::{PermissionResolver, check_permissions};
use timing::Pending;

type NoMessageHandler = fn(Context<'_>) -> std::future::Ready<Result<()>>;

/// Akari とメッセージハンドラーで使う結果型です。
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// テスト用サーバー向けの、単一シャードの Discord Bot です。
pub struct Bot {
    token: String,
    events: Events,
    slash: Option<SlashCommands>,
    intents: Option<Intents>,
    additional_intents: Intents,
    reporter: Reporter,
    info: InfoReporter,
    messages: Messages,
    include_bots: bool,
    limits: Limits,
    permission_cache: Option<Duration>,
}

impl fmt::Debug for Bot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bot")
            .field("token", &"<redacted>")
            .field("events", &self.events)
            .field("slash", &self.slash)
            .field("intents", &self.intents())
            .field("has_error_hook", &self.reporter.has_hook())
            .field("has_info_hook", &self.info.has_hook())
            .field("messages", &self.messages)
            .field("include_bots", &self.include_bots)
            .field("limits", &self.limits)
            .field("permission_cache", &self.permission_cache)
            .finish()
    }
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
            return Err(Error::EmptyToken);
        }

        Ok(Self {
            token: token.to_owned(),
            events: Events::default(),
            slash: None,
            intents: None,
            additional_intents: Intents::empty(),
            reporter: Reporter::default(),
            info: InfoReporter::default(),
            messages: Messages::default(),
            include_bots: false,
            limits: Limits::default(),
            permission_cache: None,
        })
    }

    /// `DISCORD_TOKEN` 環境変数から Bot を作ります。
    ///
    /// # エラー
    ///
    /// 環境変数が未設定、空、または Unicode 文字列として読み取れない場合に失敗します。
    pub fn from_env() -> Result<Self> {
        let token = env::var("DISCORD_TOKEN").map_err(|_| Error::InvalidTokenEnvironment)?;
        Self::new(&token).map_err(|_| Error::InvalidTokenEnvironment)
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

    /// 要求する Gateway Intent を、指定したビット集合にします。
    ///
    /// Akari は Intent を自動では追加しません。既定は空です。
    /// テキストコマンドやメッセージのハンドラーを使う場合は、`GUILD_MESSAGES` と
    /// `MESSAGE_CONTENT` を含めてください。足りないときは、起動前にエラーを返します。
    /// `MESSAGE_CONTENT` は特権 Intent なので、Developer Portal の Bot ページでも有効にします。
    pub fn with_intents(mut self, intents: Intents) -> Self {
        self.intents = Some(intents);
        self
    }

    /// 要求する Intent に、渡された Intent を追加します。
    /// 複数回指定すると和集合になり、[`Self::with_intents`] との呼び出し順に関わらず適用します。
    pub fn add_intents(mut self, intents: Intents) -> Self {
        self.additional_intents |= intents;
        self
    }

    /// 起動時に Discord へ要求する Intent を返します。
    pub fn intents(&self) -> Intents {
        self.intents.unwrap_or_else(Intents::empty) | self.additional_intents
    }

    /// エラーを受け取る関数を設定します。
    ///
    /// ハンドラーが返したエラー、Gateway の切断や受信エラー、処理枠が満杯でイベントを捨てたことなどを、
    /// 一件ずつ [`ErrorReport`] で受け取ります。設定すると、Akari は標準エラー出力へは書きません。
    /// 設定しない場合は、これらを `[akari] ...` の形で標準エラー出力に書きます。
    ///
    /// 関数は処理の途中で同期的に呼ばれます。短く保ち、`panic` しないでください。
    /// 重い処理や非同期の処理は、チャンネルへ送るなどして、別のタスクで行います。
    pub fn on_error(mut self, hook: impl Fn(&ErrorReport<'_>) + Send + Sync + 'static) -> Self {
        self.reporter = Reporter::new(hook);
        self
    }

    /// 実行時の情報を受け取る関数を設定します。
    ///
    /// 設定した場合に限り、Gateway の準備や再開、スラッシュコマンドの同期、
    /// Interaction の初回応答を同期的に通知します。関数は処理の途中で呼ばれます。短く保ち、`panic` しないでください。
    pub fn on_info(mut self, hook: impl Fn(&InfoEvent) + Send + Sync + 'static) -> Self {
        self.info = InfoReporter::new(hook);
        self
    }

    /// 他の Bot やウェブフックが送ったメッセージも、コマンドとメッセージのハンドラーに渡します。
    ///
    /// 既定では、Bot とウェブフックの投稿は無視します。有効にしても、この Bot 自身の投稿は
    /// 渡しません（自分の返信に反応する無限ループを防ぐため）。ほかの Bot と
    /// 互いに反応し合うループは、アプリ側で防いでください。
    pub fn include_bots(mut self, include: bool) -> Self {
        self.include_bots = include;
        self
    }

    /// テキストコマンドの権限確認で取得したサーバーとチャンネルの情報を、指定した期間だけ保存します。
    ///
    /// テキストコマンドの権限確認（[`Context::require_permissions`] など）は、メッセージに権限が
    /// 付いていないため、確認のたびに Discord へ問い合わせます。既定では保存しないので、
    /// 呼び出しが多い Bot は、期間（数十秒〜数分）を指定すると問い合わせを減らせます。
    /// 保存中にロールやチャンネルの権限を変えると、期間が過ぎるまで反映されません。
    /// スラッシュコマンドは Discord が権限を送るため、この設定の影響を受けません。
    pub fn cache_permissions(mut self, ttl: Duration) -> Self {
        self.permission_cache = Some(ttl);
        self
    }

    /// 同時に処理する数と待ち行列の大きさを設定します。[`Limits`] を参照してください。
    ///
    /// 0 を含む設定は、起動時に [`Error::InvalidLimits`] になります。
    pub fn limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Akari が利用者へ自動で送る文言を設定します。[`Messages`] を参照してください。
    pub fn with_messages(mut self, messages: Messages) -> Self {
        self.messages = messages;
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
    ///
    /// 返る Future は `Send` を実装していません。呼び出し元で直接 `.await` するか、
    /// `LocalSet` 上で実行してください。
    ///
    /// ```compile_fail,E0277
    /// use disnana_akari::{Bot, Context, Result};
    ///
    /// async fn handle_message(_: Context<'_>) -> Result<()> {
    ///     Ok(())
    /// }
    ///
    /// fn assert_send(_: impl Send) {}
    /// let bot = Bot::new("test-token").unwrap();
    /// assert_send(bot.run(handle_message));
    /// ```
    pub async fn run<E>(
        self,
        on_message: impl for<'a> AsyncFn(Context<'a>) -> std::result::Result<(), E>,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.connect(
            Some(error::adapt_handler!(on_message, ctx: Context<'_>)),
            None,
        )
        .await
    }

    async fn connect(
        self,
        on_message: Option<impl for<'a> AsyncFn(Context<'a>) -> Result<()>>,
        text_commands: Option<Commands>,
    ) -> Result<()> {
        let Bot {
            token,
            events,
            slash,
            intents,
            additional_intents,
            reporter,
            info,
            messages,
            include_bots,
            limits,
            permission_cache,
        } = self;
        let hooks = Hooks {
            reporter,
            info,
            messages,
        };

        // Intent は自動では足さない。足りないときは、通信する前にエラーにする。
        let observe_messages = on_message.is_some() || events.has_message_handlers();
        let text_commands = text_commands.as_ref();
        let needs_messages = observe_messages || text_commands.is_some();
        let intents = intents.unwrap_or_else(Intents::empty) | additional_intents;
        check_intents(intents, needs_messages)?;
        limits.validate()?;

        let http = Client::new(token.clone());
        let permissions = PermissionResolver::new(permission_cache);
        let mut event_types = EventTypeFlags::READY;
        if hooks.info.has_hook() {
            event_types |= EventTypeFlags::RESUMED;
        }

        let slash = match slash {
            Some(commands) => {
                event_types |= EventTypeFlags::INTERACTION_CREATE;
                Some(commands.sync(&http, &hooks.info).await?)
            }
            None => None,
        };

        if events.interaction_handlers.has_handlers() {
            event_types |= EventTypeFlags::INTERACTION_CREATE;
        }

        if needs_messages {
            event_types |= EventTypeFlags::MESSAGE_CREATE;
        }

        let mut shard = Shard::new(ShardId::ONE, token, intents);
        let (event_tx, mut event_queue) = mpsc::channel(limits.queue_capacity);
        let (text_tx, text_queue) = mpsc::channel::<Pending<Arc<Message>>>(limits.queue_capacity);
        let (text_busy_tx, text_busy_queue) = mpsc::channel(limits.queue_capacity);
        let (interactions, interaction_queue) =
            mpsc::channel::<Pending<Interaction>>(limits.queue_capacity);
        let (busy, busy_queue) = mpsc::channel(limits.queue_capacity);
        let slash = slash.as_ref();

        let handle_events = async {
            while let Some(event) = event_queue.recv().await {
                match event {
                    MessageEvent::Ready(ready) => {
                        events.ready(&ready, &hooks.reporter).await;
                    }
                    MessageEvent::Message(pending) => {
                        let (message, timing) = pending.begin();
                        if let Some(handler) = &on_message {
                            let context = Context {
                                http: &http,
                                message: &message,
                                timing,
                                permissions: &permissions,
                            };
                            if let Err(error) = handler(context).await {
                                hooks.reporter.report(ErrorSource::Message, None, &error);
                            }
                        }
                        events.message(&message, &hooks.reporter).await;
                    }
                }
            }
        };

        let handle_text = work_queue::dispatch(
            text_queue,
            limits.text_workers,
            |pending| async {
                let (message, timing) = Pending::begin(pending);
                if let Some(commands) = text_commands {
                    commands
                        .dispatch_with_feedback(
                            Context {
                                http: &http,
                                message: &message,
                                timing,
                                permissions: &permissions,
                            },
                            &hooks,
                        )
                        .await;
                }
            },
            |pending| queue_text_busy(&text_busy_tx, pending, &hooks),
        );
        let handle_text_busy =
            work_queue::respond(text_busy_queue, limits.notice_workers, |pending| async {
                let (message, timing) = Pending::begin(pending);
                let context = Context {
                    http: &http,
                    message: &message,
                    timing,
                    permissions: &permissions,
                };
                if let Some(text) = &hooks.messages.text_command_busy
                    && let Err(error) = context.reply(text).await
                {
                    hooks
                        .reporter
                        .report(ErrorSource::Response, Some("混雑の通知"), &error);
                }
            });

        let handle_interactions = work_queue::dispatch(
            interaction_queue,
            limits.interaction_workers,
            |pending| async {
                let (interaction, timing) = Pending::begin(pending);
                if matches!(
                    interaction.kind,
                    InteractionType::MessageComponent | InteractionType::ModalSubmit
                ) {
                    events
                        .interaction_handlers
                        .dispatch(&http, &interaction, timing, &hooks)
                        .await;
                } else if let Some(commands) = slash {
                    commands.dispatch(&http, &interaction, timing, &hooks).await;
                }
            },
            |pending| queue_busy_response(&busy, pending, &hooks),
        );
        let handle_busy = work_queue::respond(
            busy_queue,
            limits.notice_workers,
            |pending: Pending<Interaction>| {
                let http = &http;
                let hooks = &hooks;
                async move {
                    let (interaction, timing) = pending.begin();
                    let initial = slash::InitialResponder {
                        http,
                        interaction: &interaction,
                        timing,
                        info: &hooks.info,
                    };
                    if let Some(text) = &hooks.messages.interaction_busy
                        && let Err(error) = initial.message(text, true).await
                    {
                        hooks
                            .reporter
                            .report(ErrorSource::Response, Some("混雑の通知"), &error);
                    }
                }
            },
        );

        let receive = async {
            let mut own_id = None;
            while let Some(event) = shard.next_event(event_types).await {
                let gateway_rtt = shard.latency().recent().first().copied();
                match event {
                    Ok(Event::Ready(ready)) => {
                        own_id = Some(ready.user.id);
                        hooks.info.report(InfoEvent::GatewayReady);
                        if event_tx
                            .try_send(MessageEvent::Ready(Box::new(ready)))
                            .is_err()
                        {
                            hooks.reporter.report(
                                ErrorSource::QueueFull,
                                Some("READY"),
                                &Error::QueueFull(QueueKind::Event),
                            );
                        }
                    }
                    Ok(Event::Resumed) => hooks.info.report(InfoEvent::GatewayResumed),
                    Ok(Event::MessageCreate(message))
                        if accepts_message(
                            include_bots,
                            own_id,
                            message.author.id,
                            message.author.bot,
                            message.webhook_id.is_some(),
                        ) =>
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
                            hooks.reporter.report(
                                ErrorSource::QueueFull,
                                Some("MESSAGE_CREATE"),
                                &Error::QueueFull(QueueKind::Event),
                            );
                        }
                        if run_command
                            && let Err(error) = text_tx.try_send(Pending::new(message, gateway_rtt))
                        {
                            queue_text_busy(&text_busy_tx, error.into_inner(), &hooks);
                        }
                    }
                    Ok(Event::InteractionCreate(interaction))
                        if events.interaction_handlers.accepts(&interaction.0)
                            || slash.is_some_and(|commands| commands.accepts(&interaction.0)) =>
                    {
                        if let Err(error) =
                            interactions.try_send(Pending::new(interaction.0, gateway_rtt))
                        {
                            queue_busy_response(&busy, error.into_inner(), &hooks);
                        }
                    }
                    Ok(Event::GatewayClose(Some(frame))) => {
                        hooks.reporter.report(
                            ErrorSource::Gateway,
                            None,
                            &Error::GatewayClose(frame.code),
                        );
                    }
                    Ok(_) => {}
                    Err(error) => {
                        hooks
                            .reporter
                            .report(ErrorSource::Gateway, None, &Error::from(error))
                    }
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

        Err(Error::GatewayEnded)
    }
}

enum MessageEvent {
    Ready(Box<Ready>),
    Message(Pending<Arc<Message>>),
}

/// テキストコマンドを処理できなかったことを報告し、文言が設定されていれば混雑を返信する。
fn queue_text_busy(
    queue: &mpsc::Sender<Pending<Arc<Message>>>,
    pending: Pending<Arc<Message>>,
    hooks: &Hooks,
) {
    hooks.reporter.report(
        ErrorSource::QueueFull,
        Some("テキストコマンド"),
        &Error::QueueFull(QueueKind::TextCommand),
    );
    if hooks.messages.text_command_busy.is_some() && queue.try_send(pending).is_err() {
        hooks.reporter.report(
            ErrorSource::QueueFull,
            Some("混雑の通知"),
            &Error::QueueFull(QueueKind::BusyNotice),
        );
    }
}

/// Interaction を処理できなかったことを報告し、文言が設定されていれば混雑を返す。
fn queue_busy_response(
    queue: &mpsc::Sender<Pending<Interaction>>,
    interaction: Pending<Interaction>,
    hooks: &Hooks,
) {
    hooks.reporter.report(
        ErrorSource::QueueFull,
        Some("Interaction"),
        &Error::QueueFull(QueueKind::Interaction),
    );
    if hooks.messages.interaction_busy.is_some() && queue.try_send(interaction).is_err() {
        hooks.reporter.report(
            ErrorSource::QueueFull,
            Some("混雑の通知"),
            &Error::QueueFull(QueueKind::BusyNotice),
        );
    }
}

/// 受け取ったメッセージをハンドラーへ渡すか判断する。自分自身の投稿は常に除く。
fn accepts_message(
    include_bots: bool,
    own_id: Option<Id<UserMarker>>,
    author: Id<UserMarker>,
    author_is_bot: bool,
    from_webhook: bool,
) -> bool {
    if Some(author) == own_id {
        return false;
    }
    include_bots || !(author_is_bot || from_webhook)
}

/// メッセージを扱うのに必要な Intent が、要求する Intent に含まれているか確認する。
fn check_intents(requested: Intents, needs_messages: bool) -> Result<()> {
    if !needs_messages {
        return Ok(());
    }
    let missing = (Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT).difference(requested);
    if missing.is_empty() {
        return Ok(());
    }
    Err(Error::MissingIntents(missing))
}

/// ハンドラーに渡される、受信メッセージと返信先の情報です。
#[derive(Clone, Copy, Debug)]
pub struct Context<'a> {
    http: &'a Client,
    message: &'a Message,
    timing: Timing,
    permissions: &'a PermissionResolver,
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

    /// メッセージが送られたサーバーの ID を返します。DM では `None` です。
    pub fn guild_id(&self) -> Option<Id<GuildMarker>> {
        self.message.guild_id
    }

    /// サーバーの中で送られたことを確認し、その ID を返します。
    ///
    /// # エラー
    ///
    /// DM などで送られた場合は、[`CheckFailure::GuildOnly`] を返します。
    pub fn require_guild(&self) -> Result<Id<GuildMarker>> {
        self.guild_id()
            .ok_or_else(|| CheckFailure::GuildOnly.into())
    }

    /// メッセージを送った人の、このチャンネルでの権限を計算して返します。
    ///
    /// メッセージには権限が付いていないため、サーバーとチャンネルの情報を Discord から取得して
    /// 計算します。呼び出しが多い場合は、[`Bot::cache_permissions`] で保存する期間を指定できます。
    ///
    /// # エラー
    ///
    /// DM では [`CheckFailure::GuildOnly`]、送り主のロールが届いていないときは
    /// [`CheckFailure::PermissionsUnavailable`]、取得に失敗したときは HTTP のエラーを返します。
    pub async fn permissions(&self) -> Result<Permissions> {
        self.permissions.resolve(self.http, self.message).await
    }

    /// メッセージを送った人が、指定した権限をすべて持っていることを確認します。
    ///
    /// # エラー
    ///
    /// 足りないときは、不足分を入れた [`CheckFailure::MissingPermissions`] を返します。
    /// そのほかは [`permissions`](Self::permissions) を参照してください。
    pub async fn require_permissions(&self, required: Permissions) -> Result<()> {
        let guild_id = self.require_guild()?;
        let held = self.permissions().await?;
        check_permissions(Some(guild_id), Some(held), required).map_err(Error::from)
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

    /// 通知する相手を指定して、受信したメッセージに返信します。
    ///
    /// [`reply`](Self::reply) は何も通知しませんが、こちらは `mentions` で許可した相手
    /// （返信元の著者、ユーザー、ロールなど）にだけ通知します。
    ///
    /// # エラー
    ///
    /// 本文が Discord の制約に合わない場合や、権限不足などで送信が失敗した場合にエラーを返します。
    pub async fn reply_with(&self, content: &str, mentions: &AllowedMentions) -> Result<()> {
        self.http
            .create_message(self.message.channel_id)
            .content(content)
            .reply(self.message.id)
            .allowed_mentions(Some(mentions))
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn ping(_: CommandContext<'_>) -> Result<()> {
        Ok(())
    }

    #[tokio::test]
    async fn zero_limits_fail_before_connecting() -> Result<()> {
        let bot = Bot::new("token")?
            .add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)
            .limits(Limits::default().text_workers(0));
        let result = bot.run_commands(Commands::new("!")?).await;
        assert!(matches!(
            result,
            Err(Error::InvalidLimits {
                field: "text_workers"
            })
        ));
        Ok(())
    }

    #[test]
    fn bot_and_webhook_messages_are_opt_in_and_never_our_own() {
        let me = Id::new(1);
        let other = Id::new(2);
        assert!(accepts_message(false, Some(me), other, false, false));
        assert!(!accepts_message(false, Some(me), other, true, false));
        assert!(!accepts_message(false, Some(me), other, false, true));
        assert!(accepts_message(true, Some(me), other, true, false));
        assert!(accepts_message(true, Some(me), other, false, true));
        assert!(!accepts_message(true, Some(me), me, true, false));
    }

    #[test]
    fn message_features_need_both_message_intents() {
        assert!(check_intents(Intents::empty(), false).is_ok());

        let message = check_intents(Intents::empty(), true)
            .unwrap_err()
            .to_string();
        assert!(message.contains("add_intents"));
        assert!(message.contains("MESSAGE_CONTENT"));

        // GUILD_MESSAGES だけでは、メッセージの本文を読めない。
        assert!(check_intents(Intents::GUILD_MESSAGES, true).is_err());
        assert!(check_intents(Intents::MESSAGE_CONTENT, true).is_err());
        assert!(check_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT, true).is_ok());
        assert!(check_intents(Intents::all(), true).is_ok());
    }

    #[test]
    fn bot_never_adds_intents_on_its_own() -> Result<()> {
        assert_eq!(Bot::new("token")?.intents(), Intents::empty());

        let both = Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT;
        let bot = Bot::new("token")?
            .add_intents(Intents::MESSAGE_CONTENT)
            .with_intents(Intents::GUILD_MESSAGES);
        assert_eq!(bot.intents(), both, "呼び出しの順序によらず和集合になる");
        Ok(())
    }

    #[tokio::test]
    async fn text_commands_without_intents_fail_before_any_network_use() -> Result<()> {
        let mut commands = Commands::new("!")?;
        commands.register("ping", ping)?;

        let error = Bot::new("token")?
            .run_commands(commands)
            .await
            .expect_err("Intent が足りないので、接続前に失敗する");
        assert!(matches!(
            error,
            Error::MissingIntents(missing)
                if missing == Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT
        ));
        assert!(error.to_string().contains("add_intents"));
        Ok(())
    }

    #[test]
    fn bot_debug_redacts_token() -> Result<()> {
        let token = "dummy-credential-for-debug-test";
        let bot = Bot::new(token)?;
        let debug = format!("{bot:#?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains(token));
        Ok(())
    }

    #[test]
    fn error_hook_and_messages_are_configured_on_the_bot() -> Result<()> {
        let messages = Messages {
            slash_command_failed: None,
            ..Messages::default()
        };
        let bot = Bot::new("token")?
            .on_error(|_| {})
            .with_messages(messages.clone());
        assert_eq!(bot.messages, messages);
        Ok(())
    }
}
