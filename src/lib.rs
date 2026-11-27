#![doc = include_str!("../docs/api.md")]

mod access;
mod autocomplete;
mod channel_ui;
mod command_check;
mod commands;
mod concurrency;
mod cooldown;
mod error;
mod events;
mod flags;
mod gateway_runtime;
mod gateway_status;
mod hooks;
mod hybrid;
mod interactions;
mod limits;
mod manage;
mod manage_types;
mod member_cache;
mod message_cache;
mod message_events;
mod module;
mod overwrite;
#[cfg(test)]
mod permission_cache_tests;
mod permissions;
mod presence;
mod shutdown;
mod slash;
pub mod soundboard;
mod timing;
pub mod ui;
pub mod validation;
pub mod voice_channel;
mod voice_gateway;
mod work_queue;

use std::{env, fmt, num::NonZeroU16, sync::Arc, time::Duration};

use events::{ActivityEvent, ReactionEvent, VoiceNotification};
use gateway_runtime::GatewayPlan;
use member_cache::{
    BootstrapConfig, BootstrapWorker, CachedMemberEvent, MemberCacheRoute, MemberCacheRuntime,
};
use message_cache::{MessageCacheRoute, MessageCacheRuntime};
use message_events::{GatewayDecodeOptions, IncomingEvent};
use tokio::sync::mpsc;
use twilight_gateway::{ConfigBuilder, Event, EventTypeFlags, Intents, ShardId};
use twilight_http::{Client, request::TryIntoRequest};
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

pub use access::{Access, Needs, PermissionSnapshot};
pub use autocomplete::AutocompleteContext;
pub use command_check::{CommandCheck, CommandCheckContext, CommandCheckSource};
pub use commands::{
    ArgumentError, ArgumentValueError, Arguments, CommandContext, CommandInfo, CommandMetadata,
    CommandPrefixes, Commands, TextCommandGroup,
};
pub use concurrency::{ConcurrencyLimit, ConcurrencyPermit};
pub use cooldown::Cooldown;
pub use error::{
    CheckFailure, CommandSyncError, Error, HandlerError, HandlerKind, HierarchyDenied,
    InvalidArgument, QueueKind, RegistrationError, ResponseStateError, SelectionError,
};
pub use events::{
    Events, GatewayPayload, GuildSoundboardSoundCreate, GuildSoundboardSoundDelete,
    GuildSoundboardSoundUpdate, GuildSoundboardSoundsUpdate, MessageDelete, MessageDeleteBulk,
    MessagePollVoteAdd, MessagePollVoteRemove, ReactionAdd, ReactionRemove, ReactionRemoveAll,
    ReactionRemoveEmoji, Ready, SoundboardSound, SoundboardSoundId, SoundboardSoundMarker,
    SoundboardSounds, TypingStart, VoiceChannelEffectSend, VoiceEffectAnimation,
};
pub use gateway_status::{
    GatewayShardState, GatewayShardStatus, GatewayStatusHandle, GatewayStatusUpdates,
};
pub use hooks::{ErrorReport, ErrorSource, InfoEvent, Messages};
pub use hybrid::{
    CommandSource, HybridArgumentError, HybridArguments, HybridCommandGroup, HybridContext,
    register_hybrid, register_hybrid_group, register_hybrid_group_with_aliases,
    register_typed_hybrid,
};
pub use interactions::{
    ButtonContext, ChannelSelectContext, EntitySelectContext, Mentionable,
    MentionableSelectContext, ModalContext, RoleSelectContext, SelectContext, UserSelectContext,
};
pub use limits::Limits;
pub use manage::{MAX_BAN_MESSAGE_DELETE, MAX_TIMEOUT, Manage};
pub use manage_types::{
    NewCategory, NewRole, NewTextChannel, NewVoiceChannel, RoleEdit, TextChannelEdit,
    VoiceChannelEdit,
};
pub use member_cache::{CachedMemberRemove, CachedMemberUpdate, MemberCacheHandle};
pub use message_cache::CachedMessageEdit;
pub use message_events::MessageUpdate;
pub use module::{Module, Registry};
pub use overwrite::{Overwrite, OverwriteTarget, Tri};
pub use presence::{Presence, PresenceHandle};
pub use shutdown::ShutdownHandle;
pub use slash::{
    CommandGroup, CommandOptionLocalizations, CommandSettings, SlashCommands, SlashContext,
    UserMenuTarget,
};
pub use timing::Timing;
pub use twilight_http as http;
pub use twilight_model as model;
pub use voice_gateway::{
    VoiceConnection, VoiceConnectionInfo, VoiceConnectionStatus, VoiceGateway, VoiceGatewayError,
    VoiceStateChange,
};

/// 権限やフラグを名前で調べる拡張トレイトと、設定の日本語名をまとめて取り込みます。
///
/// `use disnana_akari::prelude::*;` と書くと、`perms.manage_messages()`、
/// `Permissions::text()`、`perms.labels()`、`channel_kind.label()` などが使えます。
pub mod prelude {
    pub use crate::flags::{
        ChannelFlagsExt, Label, MessageFlagsExt, PermissionPresets, PermissionsExt,
        SystemChannelFlagsExt, UserFlagsExt,
    };
}

use hooks::{Hooks, InfoReporter, Reporter};
use interactions::RadioSubmission;
use permissions::{PermissionResolver, check_permissions};
use timing::Pending;

type NoMessageHandler = fn(Context<'_>) -> std::future::Ready<Result<()>>;

/// Akari とメッセージハンドラーで使う結果型です。
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// イベントやコマンドを登録して起動できる Discord Bot です。
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
    message_cache_capacity: usize,
    member_cache_capacity: usize,
    member_cache_handle: Option<MemberCacheHandle>,
    member_cache_bootstrap: Option<BootstrapConfig>,
    presence: Option<PresenceHandle>,
    shutdown: Option<ShutdownHandle>,
    recommended_shards: bool,
    gateway_status: Option<GatewayStatusHandle>,
    voice_gateway: Option<VoiceGateway>,
    #[cfg(test)]
    gateway_test_port: Option<u16>,
    #[cfg(test)]
    http_test_port: Option<u16>,
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
            .field("message_cache_capacity", &self.message_cache_capacity)
            .field("member_cache_capacity", &self.member_cache_capacity)
            .field("has_shutdown_handle", &self.shutdown.is_some())
            .field("recommended_shards", &self.recommended_shards)
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
            message_cache_capacity: 0,
            member_cache_capacity: 0,
            member_cache_handle: None,
            member_cache_bootstrap: None,
            presence: None,
            shutdown: None,
            recommended_shards: false,
            gateway_status: None,
            voice_gateway: None,
            #[cfg(test)]
            gateway_test_port: None,
            #[cfg(test)]
            http_test_port: None,
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

    /// Gateway 接続時と実行中のステータスを、共有ハンドルから設定します。
    pub fn with_presence(mut self, presence: PresenceHandle) -> Self {
        self.presence = Some(presence);
        self
    }

    /// Discord が推奨するシャードを取得し、検証した範囲をすべて起動します。
    ///
    /// 既定では単一シャードで起動します。この設定を使うと、Discord のセッション開始制限に従い、
    /// 準備できた組から順に次の組を起動します。準備に時間がかかった組は 60 秒で失敗します。
    /// コマンドの登録と処理枠は Bot 全体で共有します。
    ///
    /// [`Events::on_ready`] の `Ready::shard` で各シャードを識別できます。
    /// [`Timing::gateway_rtt`] は、そのイベントを受信したシャードの往復時間です。
    pub fn with_recommended_shards(mut self) -> Self {
        self.recommended_shards = true;
        self
    }

    /// 各 Gateway シャードの接続状態と RTT を読み取るハンドルを取得します。
    ///
    /// 繰り返し呼ぶと同じ状態を共有します。ハンドルは Bot の実行中に別のタスクからも参照できます。
    pub fn with_gateway_status(mut self) -> (Self, GatewayStatusHandle) {
        let status = self
            .gateway_status
            .get_or_insert_with(GatewayStatusHandle::new)
            .clone();
        (self, status)
    }

    /// 正常終了を要求できるハンドルを取得します。複数回呼ぶと同じ終了要求を共有します。
    ///
    /// 起動前に要求すると Discord に接続せず、起動メソッドは `Ok(())` を返します。
    /// 稼働中の要求は新しいイベントの受け付けを止め、Gateway の切断を最大 2 秒、
    /// 受け付け済みの処理の終了を最大 5 秒、並行して待ちます。期限に達した処理は打ち切ります。
    /// ハンドルを破棄するだけでは終了しません。
    pub fn with_shutdown(mut self) -> (Self, ShutdownHandle) {
        let handle = self
            .shutdown
            .get_or_insert_with(ShutdownHandle::new)
            .clone();
        (self, handle)
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
    /// 一件ずつ [`ErrorReport`] で受け取ります。コードで反応するための関数です。
    /// Akari は、標準出力にも標準エラー出力にも書きません。ログとしては、同じ内容を `tracing` の
    /// イベント（`target` は `disnana_akari`）で発行します。この関数を設定しても、ログは変わりません。
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
    /// `Intents::GUILDS` を指定すると、ロールやチャンネルの変更イベントを受けた時点で該当する保存を破棄します。
    /// 次の権限確認で Discord から取り直します。イベントを受け取れない場合には保存期間が適用されます。
    /// スラッシュコマンドは Discord が権限を送るため、この設定の影響を受けません。
    pub fn cache_permissions(mut self, ttl: Duration) -> Self {
        self.permission_cache = Some(ttl);
        self
    }

    /// 編集・削除イベントのために、一度受信したメッセージを最大 `capacity` 件保存します。
    ///
    /// 保存件数は Bot 全体で共有し、古い ID から外します。0 は無効化です。
    /// キャッシュを読むイベントを登録した場合に保存を始めます。
    /// 新しい Gateway セッションの Ready を受け取ると、そのシャードの保存を破棄します。
    pub fn cache_messages(mut self, capacity: usize) -> Self {
        self.message_cache_capacity = capacity;
        self
    }

    /// メンバーの更新前後と退出前の通知に使う保存件数を設定します。
    ///
    /// キャッシュ付きの通知を登録した場合は、正の容量と
    /// `GUILDS` および `GUILD_MEMBERS` Intent が必要です。
    #[must_use]
    pub fn cache_members(mut self, capacity: usize) -> Self {
        self.member_cache_capacity = capacity;
        self
    }

    /// 保存済みのメンバーを読み取れるハンドルを取得します。
    ///
    /// `GUILDS` と `GUILD_MEMBERS` Intent、および正の容量が必要です。
    /// メンバーの通知を登録しなくても保存し、ハンドルから保存済みの情報を参照できます。
    #[must_use]
    pub fn with_member_cache(mut self, capacity: usize) -> (Self, MemberCacheHandle) {
        let handle = MemberCacheHandle::new();
        self.member_cache_capacity = capacity;
        self.member_cache_handle = Some(handle.clone());
        (self, handle)
    }

    /// 指定したサーバーのメンバーを、空きのあるキャッシュに REST で追加します。
    ///
    /// [`Self::cache_members`] または [`Self::with_member_cache`] で正の容量を設定し、
    /// `GUILDS` と `GUILD_MEMBERS` Intent を指定してください。サーバーが利用可能になった時点で
    /// 1 ページにつき最大 1,000 件を取得します。サーバーごとの総取得上限は
    /// `per_guild_limit` とキャッシュ容量の小さい値です。取得はバックグラウンドで進みます。
    /// 保存済みのメンバーや取得中に更新・退出したメンバーは上書きしません。
    /// Gateway を再開できた場合は、最後に受け取ったページの次から取得を続けます。
    /// 空のリストを渡すと REST 取得を無効にします。
    #[must_use]
    pub fn with_member_cache_bootstrap(
        mut self,
        guilds: impl IntoIterator<Item = Id<GuildMarker>>,
        per_guild_limit: NonZeroU16,
    ) -> Self {
        self.member_cache_bootstrap = BootstrapConfig::new(guilds, per_guild_limit);
        self
    }

    /// ボイスチャンネルの参加や退出を要求できるハンドルを取得します。
    ///
    /// `GUILD_VOICE_STATES` Intent を明示的に追加してください。
    #[must_use]
    pub fn with_voice_gateway(mut self) -> (Self, VoiceGateway) {
        let voice = VoiceGateway::new();
        self.voice_gateway = Some(voice.clone());
        (self, voice)
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
    /// ハンドラーに渡しません。ハンドラーのエラーは `tracing` のログと `on_error` に報告して、処理を続けます。
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
            message_cache_capacity,
            member_cache_capacity,
            member_cache_handle,
            member_cache_bootstrap,
            presence,
            shutdown,
            recommended_shards,
            gateway_status,
            voice_gateway,
            #[cfg(test)]
            gateway_test_port,
            #[cfg(test)]
            http_test_port,
        } = self;
        let mut shutdown = shutdown.as_ref().map(ShutdownHandle::subscribe);
        if shutdown.as_ref().is_some_and(|request| *request.borrow()) {
            return Ok(());
        }
        let hooks = Hooks {
            reporter,
            info,
            messages,
            permissions: Arc::new(PermissionResolver::new(permission_cache)),
        };

        // Intent は自動では足さない。足りないときは、通信する前にエラーにする。
        let observe_messages = on_message.is_some() || events.has_message_handlers();
        let text_commands = text_commands.as_ref();
        let needs_messages = observe_messages || text_commands.is_some();
        let intents = intents.unwrap_or_else(Intents::empty) | additional_intents;
        check_intents(intents, needs_messages)?;
        let handler_events = handler_event_types(&events, intents)?;
        let voice_notifications = events.voice_notification_types(intents)?;
        let message_cache = MessageCacheRuntime::prepare(message_cache_capacity, &events, intents)?;
        let member_cache = MemberCacheRuntime::prepare(
            member_cache_capacity,
            &events,
            intents,
            member_cache_handle,
            member_cache_bootstrap,
        )?;
        check_voice_gateway_intents(intents, voice_gateway.is_some())?;
        limits.validate()?;

        let http = Client::new(token.clone());
        #[cfg(test)]
        let http = match http_test_port {
            Some(port) => Client::builder()
                .token(token.clone())
                .proxy(format!("127.0.0.1:{port}"), true)
                .build(),
            None => http,
        };
        let permissions = Arc::clone(&hooks.permissions);
        let event_types = gateway_event_types(
            needs_messages,
            slash.is_some() || events.interaction_handlers.has_handlers(),
            permission_cache_events(needs_messages && permissions.cache_enabled(), intents),
        ) | handler_events
            | message_cache
                .as_ref()
                .map_or_else(EventTypeFlags::empty, MessageCacheRuntime::event_types)
            | member_cache
                .as_ref()
                .map_or_else(EventTypeFlags::empty, MemberCacheRuntime::event_types);

        let prepare = async {
            let plan = GatewayPlan::discover(&http, recommended_shards).await?;
            let slash = match slash {
                Some(commands) => Some(commands.sync(&http, &hooks.info).await?),
                None => None,
            };
            Ok::<_, Error>((plan, slash))
        };
        let (plan, slash) = tokio::select! {
            biased;
            () = shutdown::wait_for_request(&mut shutdown) => return Ok(()),
            result = prepare => result?,
        };

        let config = ConfigBuilder::new(token, intents);
        #[cfg(test)]
        let config = match gateway_test_port {
            Some(port) => config.proxy_url(format!("ws://127.0.0.1:{port}")),
            None => config,
        };
        let mut gateways = plan.prepare(config, presence.as_ref())?;
        gateways.install_handles(gateway_status.as_ref(), voice_gateway.as_ref());
        let (event_tx, event_queue) = mpsc::channel(limits.queue_capacity);
        let (text_ingress, handle_text) =
            commands::text_workers(text_commands, limits, &http, &permissions, &hooks);
        let (interactions, interaction_queue) =
            mpsc::channel::<Pending<InteractionEvent>>(limits.queue_capacity);
        let (busy, busy_queue) = mpsc::channel(limits.queue_capacity);
        let slash = slash.as_ref();

        let handle_events = dispatch_events(
            event_queue,
            on_message,
            &events,
            &http,
            &permissions,
            &hooks,
        );

        let worker_hooks = &hooks;
        let interaction_overflow = busy.clone();
        let handle_interactions = work_queue::dispatch(
            interaction_queue,
            limits.interaction_workers,
            |pending| dispatch_interaction(pending, &events, slash, &http, &hooks),
            move |pending| queue_busy_response(&interaction_overflow, pending, worker_hooks),
        );
        let handle_busy = work_queue::respond(busy_queue, limits.notice_workers, |pending| {
            respond_interaction_busy(pending, &http, &hooks)
        });

        let receive = gateways.drive(
            event_types,
            GatewayDecodeOptions {
                capture_modal_radios: events.interaction_handlers.has_modals(),
                capture_member_fields: member_cache.is_some(),
                voice_notifications,
            },
            |shard| gateway_lost(shard, &permissions, member_cache.as_ref()),
            |gateway, event| {
                let shard = gateway.id();
                let cache = message_cache.as_ref().map(|cache| cache.shard(shard));
                let Some((event, radio)) = route_gateway_event(
                    event,
                    &events,
                    &permissions,
                    &event_tx,
                    &hooks.reporter,
                    shard,
                    (cache, member_cache.as_ref().map(|cache| cache.shard(shard))),
                ) else {
                    return;
                };
                let gateway_rtt = gateway.gateway_rtt();
                match event {
                    Event::Ready(ready) => {
                        gateway.ready(ready.user.id);
                        hooks.info.report(InfoEvent::GatewayReady);
                        queue_event(
                            &event_tx,
                            MessageEvent::Ready(Box::new(ready)),
                            "READY",
                            &hooks.reporter,
                        );
                    }
                    Event::Resumed => {
                        gateway.replay_presence();
                        hooks.info.report(InfoEvent::GatewayResumed);
                    }
                    Event::MessageCreate(message) => {
                        let accepted = accepts_message(
                            include_bots,
                            gateway.own_id(),
                            message.author.id,
                            message.author.bot,
                            message.webhook_id.is_some(),
                        );
                        let text_route = accepted
                            .then(|| text_ingress.classify(&message.content))
                            .flatten();
                        let observe = accepted && observe_messages;
                        if !observe && text_route.is_none() && cache.is_none() {
                            return;
                        }
                        let message = Arc::new(message.message);
                        if let Some(cache) = cache {
                            cache.store(Arc::clone(&message));
                        }
                        if observe {
                            queue_event(
                                &event_tx,
                                MessageEvent::Message(Pending::new(
                                    Arc::clone(&message),
                                    gateway_rtt,
                                )),
                                "MESSAGE_CREATE",
                                &hooks.reporter,
                            );
                        }
                        if let Some(route) = text_route {
                            text_ingress.submit(route, Pending::new(message, gateway_rtt), &hooks);
                        }
                    }
                    Event::InteractionCreate(interaction)
                        if events.interaction_handlers.accepts(&interaction.0)
                            || slash.is_some_and(|commands| commands.accepts(&interaction.0)) =>
                    {
                        let payload = InteractionEvent {
                            interaction: interaction.0,
                            radio,
                        };
                        if let Err(error) =
                            interactions.try_send(Pending::new(payload, gateway_rtt))
                        {
                            queue_busy_response(&busy, error.into_inner(), &hooks);
                        }
                    }
                    Event::GatewayClose(Some(frame)) => {
                        hooks.reporter.report(
                            ErrorSource::Gateway,
                            None,
                            &Error::GatewayClose(frame.code),
                        );
                    }
                    _ => {}
                }
            },
        );

        let workers = async {
            tokio::join!(handle_events, handle_text, handle_interactions, handle_busy);
        };
        tokio::pin!(workers);
        let stopped = BootstrapWorker::drive(
            member_cache
                .as_ref()
                .and_then(MemberCacheRuntime::bootstrap_worker),
            &http,
            &hooks.reporter,
            shutdown::drive(receive, workers.as_mut(), &mut shutdown),
        )
        .await;
        suspend_member_reads(member_cache.as_ref());
        if let Err(error) = stopped {
            if recommended_shards {
                shutdown::finish(gateways.close(), std::future::ready(())).await;
            }
            return Err(error);
        }

        drop((event_tx, text_ingress, interactions, busy));
        shutdown::finish(gateways.close(), workers).await;
        Ok(())
    }
}

async fn dispatch_interaction(
    pending: Pending<InteractionEvent>,
    events: &Events,
    slash: Option<&slash::ActiveSlashCommands>,
    http: &Client,
    hooks: &Hooks,
) {
    let (InteractionEvent { interaction, radio }, timing) = pending.begin();
    if matches!(
        interaction.kind,
        InteractionType::MessageComponent | InteractionType::ModalSubmit
    ) {
        events
            .interaction_handlers
            .dispatch(http, &interaction, timing, hooks, &radio)
            .await;
    } else if let Some(commands) = slash {
        commands.dispatch(http, &interaction, timing, hooks).await;
    }
}

async fn respond_interaction_busy(
    pending: Pending<InteractionEvent>,
    http: &Client,
    hooks: &Hooks,
) {
    let (incoming, timing) = pending.begin();
    let initial = slash::InitialResponder {
        http,
        interaction: &incoming.interaction,
        timing,
        info: &hooks.info,
        permissions: &hooks.permissions,
    };
    let response = if incoming.interaction.kind == InteractionType::ApplicationCommandAutocomplete {
        initial.autocomplete(Vec::new()).await
    } else if let Some(text) = &hooks.messages.interaction_busy {
        initial.message(text, true).await
    } else {
        return;
    };
    if let Err(error) = response {
        hooks
            .reporter
            .report(ErrorSource::Response, Some("混雑の通知"), &error);
    }
}

fn gateway_event_types(
    needs_messages: bool,
    needs_interactions: bool,
    cache_events: EventTypeFlags,
) -> EventTypeFlags {
    // 再開は、ログ（`tracing`）にも出すので、フックの有無にかかわらず受け取る。
    let mut events = EventTypeFlags::READY | EventTypeFlags::RESUMED | cache_events;
    if needs_messages {
        events |= EventTypeFlags::MESSAGE_CREATE;
    }
    if needs_interactions {
        events |= EventTypeFlags::INTERACTION_CREATE;
    }
    events
}

fn check_voice_gateway_intents(intents: Intents, voice_enabled: bool) -> Result<()> {
    if voice_enabled && !intents.contains(Intents::GUILD_VOICE_STATES) {
        return Err(Error::MissingEventIntents {
            any_of: Intents::GUILD_VOICE_STATES,
        });
    }
    Ok(())
}

fn gateway_lost(
    shard: ShardId,
    permissions: &PermissionResolver,
    members: Option<&MemberCacheRuntime>,
) {
    if let Some(members) = members {
        members.suspend_shard(shard);
    }
    permissions.clear_shard(shard);
}

fn suspend_member_reads(members: Option<&MemberCacheRuntime>) {
    if let Some(members) = members {
        members.suspend_all();
    }
}

fn permission_cache_events(enabled: bool, intents: Intents) -> EventTypeFlags {
    if !enabled {
        return EventTypeFlags::empty();
    }
    let mut events = EventTypeFlags::RESUMED;
    if intents.contains(Intents::GUILDS) {
        events |= EventTypeFlags::GUILD_UPDATE
            | EventTypeFlags::GUILD_DELETE
            | EventTypeFlags::ROLE_CREATE
            | EventTypeFlags::ROLE_UPDATE
            | EventTypeFlags::ROLE_DELETE
            | EventTypeFlags::CHANNEL_UPDATE
            | EventTypeFlags::CHANNEL_DELETE
            | EventTypeFlags::THREAD_UPDATE
            | EventTypeFlags::THREAD_DELETE;
    }
    events
}

fn update_permission_cache(permissions: &PermissionResolver, event: &Event, shard: ShardId) {
    match event {
        Event::Ready(_) | Event::Resumed | Event::GatewayClose(_) => permissions.clear_shard(shard),
        Event::GuildUpdate(guild) => permissions.invalidate_guild(guild.0.id),
        Event::GuildDelete(guild) => permissions.remove_guild(guild.id),
        Event::RoleCreate(role) => permissions.invalidate_guild(role.guild_id),
        Event::RoleUpdate(role) => permissions.invalidate_guild(role.guild_id),
        Event::RoleDelete(role) => permissions.invalidate_guild(role.guild_id),
        Event::ChannelUpdate(channel) => permissions.invalidate_channel(channel.0.id),
        Event::ChannelDelete(channel) => permissions.invalidate_channel(channel.0.id),
        Event::ThreadUpdate(thread) => permissions.invalidate_channel(thread.0.id),
        Event::ThreadDelete(thread) => permissions.invalidate_channel(thread.id),
        _ => {}
    }
}

enum MessageEvent {
    Ready(Box<Ready>),
    Message(Pending<Arc<Message>>),
    Update(Box<MessageUpdate>, Option<CachedMessageEdit>),
    Delete(MessageDelete, Option<Arc<Message>>),
    BulkDelete(MessageDeleteBulk, Vec<Arc<Message>>),
    Reaction(ReactionEvent),
    Activity(ActivityEvent),
    Gateway(Box<Event>),
    VoiceNotification(VoiceNotification),
    CachedMember(Box<CachedMemberEvent>, Option<Box<Event>>),
}

struct InteractionEvent {
    interaction: Interaction,
    radio: Vec<RadioSubmission>,
}

fn route_cached_member(
    event: Result<IncomingEvent>,
    cache: MemberCacheRoute<'_>,
    events: &Events,
    queue: &mpsc::Sender<MessageEvent>,
    reporter: &Reporter,
) -> Option<Result<IncomingEvent>> {
    let (event, cached) = match event {
        Ok(IncomingEvent::MemberUpdate(member, fields)) => {
            let cached = cache.update(&member, fields);
            (Event::MemberUpdate(member), cached)
        }
        Ok(IncomingEvent::Standard(event)) => {
            let cached = cache.observe(&event);
            (event, cached)
        }
        Err(error) => {
            cache.invalidate_on_decode_error(&error);
            return Some(Err(error));
        }
        other => return Some(other),
    };
    let Some(cached) = cached else {
        return Some(Ok(IncomingEvent::Standard(event)));
    };
    let name = match &cached {
        CachedMemberEvent::Update(_) => "GUILD_MEMBER_UPDATE",
        CachedMemberEvent::Remove(_) => "GUILD_MEMBER_REMOVE",
    };
    let raw = events.gateway_event_name(&event).map(|_| Box::new(event));
    queue_event(
        queue,
        MessageEvent::CachedMember(Box::new(cached), raw),
        name,
        reporter,
    );
    None
}

fn route_gateway_event(
    event: Result<IncomingEvent>,
    events: &Events,
    permissions: &PermissionResolver,
    queue: &mpsc::Sender<MessageEvent>,
    reporter: &Reporter,
    shard: ShardId,
    (cache, members): (Option<MessageCacheRoute<'_>>, Option<MemberCacheRoute<'_>>),
) -> Option<(Event, Vec<RadioSubmission>)> {
    let event = match members {
        Some(members) => route_cached_member(event, members, events, queue, reporter)?,
        None => event,
    };
    if let (Some(cache), Err(error)) = (cache, &event)
        && message_cache::invalidates_message_history(error)
    {
        // Twilight は型に変換する前にシーケンスを記録する。失敗した更新は再開しても再送されない。
        cache.reset();
    }
    if let Err(error) = &event
        && permissions.cache_enabled()
        && message_events::lost_gateway_event(
            error,
            permission_cache_events(true, Intents::GUILDS) | EventTypeFlags::READY,
        )
    {
        permissions.clear_shard(shard);
    }
    let (event, radio) = forward_lifecycle_event(event, queue, reporter, cache)?;
    if matches!(event, Event::Ready(_))
        && let Some(cache) = cache
    {
        cache.reset();
    }
    if permissions.cache_enabled() {
        update_permission_cache(permissions, &event, shard);
    }
    if let Some(name) = events.gateway_event_name(&event) {
        queue_event(
            queue,
            MessageEvent::Gateway(Box::new(event)),
            name,
            reporter,
        );
        return None;
    }
    Some((event, radio))
}

fn forward_lifecycle_event(
    event: Result<IncomingEvent>,
    queue: &mpsc::Sender<MessageEvent>,
    reporter: &hooks::Reporter,
    cache: Option<MessageCacheRoute<'_>>,
) -> Option<(Event, Vec<RadioSubmission>)> {
    let (event, name) = match event {
        Ok(IncomingEvent::MessageUpdate(update)) => {
            let cached = cache.and_then(|cache| cache.edit(&update));
            if cached.is_none()
                && cache.is_some_and(|cache| !cache.has_raw(EventTypeFlags::MESSAGE_UPDATE))
            {
                return None;
            }
            (
                MessageEvent::Update(Box::new(update), cached),
                "MESSAGE_UPDATE",
            )
        }
        Ok(IncomingEvent::Standard(Event::MessageDelete(delete))) => {
            let cached = cache.and_then(|cache| cache.delete(&delete));
            if cached.is_none()
                && cache.is_some_and(|cache| !cache.has_raw(EventTypeFlags::MESSAGE_DELETE))
            {
                return None;
            }
            (MessageEvent::Delete(delete, cached), "MESSAGE_DELETE")
        }
        Ok(IncomingEvent::Standard(Event::MessageDeleteBulk(delete))) => {
            let cached = cache.map_or_else(Vec::new, |cache| cache.bulk_delete(&delete));
            if cached.is_empty()
                && cache.is_some_and(|cache| !cache.has_raw(EventTypeFlags::MESSAGE_DELETE_BULK))
            {
                return None;
            }
            (
                MessageEvent::BulkDelete(delete, cached),
                "MESSAGE_DELETE_BULK",
            )
        }
        Ok(IncomingEvent::Standard(Event::ReactionAdd(reaction))) => (
            MessageEvent::Reaction(ReactionEvent::Add(reaction)),
            "MESSAGE_REACTION_ADD",
        ),
        Ok(IncomingEvent::Standard(Event::ReactionRemove(reaction))) => (
            MessageEvent::Reaction(ReactionEvent::Remove(reaction)),
            "MESSAGE_REACTION_REMOVE",
        ),
        Ok(IncomingEvent::Standard(Event::ReactionRemoveAll(reaction))) => (
            MessageEvent::Reaction(ReactionEvent::Clear(reaction)),
            "MESSAGE_REACTION_REMOVE_ALL",
        ),
        Ok(IncomingEvent::ReactionRemoveEmoji(reaction)) => (
            MessageEvent::Reaction(ReactionEvent::ClearEmoji(reaction)),
            "MESSAGE_REACTION_REMOVE_EMOJI",
        ),
        Ok(IncomingEvent::Standard(Event::MessagePollVoteAdd(vote))) => (
            MessageEvent::Activity(ActivityEvent::PollVoteAdd(vote)),
            "MESSAGE_POLL_VOTE_ADD",
        ),
        Ok(IncomingEvent::Standard(Event::MessagePollVoteRemove(vote))) => (
            MessageEvent::Activity(ActivityEvent::PollVoteRemove(vote)),
            "MESSAGE_POLL_VOTE_REMOVE",
        ),
        Ok(IncomingEvent::Standard(Event::TypingStart(typing))) => (
            MessageEvent::Activity(ActivityEvent::Typing(typing)),
            "TYPING_START",
        ),
        Ok(IncomingEvent::VoiceNotification(event)) => {
            let name = event.name();
            (MessageEvent::VoiceNotification(event), name)
        }
        Ok(IncomingEvent::Standard(event)) => return Some((event, Vec::new())),
        Ok(IncomingEvent::MemberUpdate(member, _)) => {
            return Some((Event::MemberUpdate(member), Vec::new()));
        }
        Ok(IncomingEvent::ModalWithRadio(interaction, radio)) => {
            return Some((Event::InteractionCreate(interaction), radio));
        }
        Err(error) => {
            reporter.report(ErrorSource::Gateway, None, &error);
            return None;
        }
    };
    queue_event(queue, event, name, reporter);
    None
}

fn queue_event(
    queue: &mpsc::Sender<MessageEvent>,
    event: MessageEvent,
    name: &str,
    reporter: &hooks::Reporter,
) {
    if queue.try_send(event).is_err() {
        reporter.report(
            ErrorSource::QueueFull,
            Some(name),
            &Error::QueueFull(QueueKind::Event),
        );
    }
}

async fn dispatch_events(
    mut queue: mpsc::Receiver<MessageEvent>,
    on_message: Option<impl for<'a> AsyncFn(Context<'a>) -> Result<()>>,
    events: &Events,
    http: &Client,
    permissions: &PermissionResolver,
    hooks: &Hooks,
) {
    while let Some(event) = queue.recv().await {
        match event {
            MessageEvent::Ready(ready) => events.ready(&ready, &hooks.reporter).await,
            MessageEvent::Update(update, cached) => {
                events.message_update(&update, &hooks.reporter).await;
                if let Some(cached) = cached {
                    events.cached_message_edit(&cached, &hooks.reporter).await;
                }
            }
            MessageEvent::Delete(delete, cached) => {
                events.message_delete(&delete, &hooks.reporter).await;
                if let Some(cached) = cached {
                    events.cached_message_delete(&cached, &hooks.reporter).await;
                }
            }
            MessageEvent::BulkDelete(delete, cached) => {
                events.message_delete_bulk(&delete, &hooks.reporter).await;
                if !cached.is_empty() {
                    events
                        .cached_bulk_message_delete(&cached, &hooks.reporter)
                        .await;
                }
            }
            MessageEvent::Reaction(reaction) => events.reaction(&reaction, &hooks.reporter).await,
            MessageEvent::Activity(activity) => events.activity(&activity, &hooks.reporter).await,
            MessageEvent::Gateway(event) => events.gateway(&event, &hooks.reporter).await,
            MessageEvent::VoiceNotification(event) => {
                events.voice_notification(&event, &hooks.reporter).await;
            }
            MessageEvent::CachedMember(cached, raw) => {
                if let Some(raw) = raw {
                    events.gateway(&raw, &hooks.reporter).await;
                }
                events
                    .dispatch_cached_member_event(&cached, &hooks.reporter)
                    .await;
            }
            MessageEvent::Message(pending) => {
                let (message, timing) = pending.begin();
                if let Some(handler) = &on_message {
                    let context = Context {
                        http,
                        message: &message,
                        timing,
                        permissions,
                    };
                    if let Err(error) = handler(context).await {
                        hooks.reporter.report(ErrorSource::Message, None, &error);
                    }
                }
                events.message(&message, &hooks.reporter).await;
            }
        }
    }
}

/// Interaction を処理できなかったことを報告し、文言が設定されていれば混雑を返す。
fn queue_busy_response(
    queue: &mpsc::Sender<Pending<InteractionEvent>>,
    interaction: Pending<InteractionEvent>,
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

fn check_lifecycle_intents(intents: Intents, events: EventTypeFlags) -> Result<()> {
    let mut any_of = Intents::GUILD_MESSAGES;
    if !events.contains(EventTypeFlags::MESSAGE_DELETE_BULK) {
        any_of |= Intents::DIRECT_MESSAGES;
    }
    check_event_intents(intents, events, any_of)
}

fn handler_event_types(events: &Events, intents: Intents) -> Result<EventTypeFlags> {
    let message_lifecycle = events.message_lifecycle_types();
    check_lifecycle_intents(intents, message_lifecycle)?;
    let reactions = events.reaction_types();
    check_reaction_intents(intents, reactions)?;
    let activity = events.activity_types();
    check_event_intents(
        intents,
        activity & EventTypeFlags::MESSAGE_POLLS,
        Intents::GUILD_MESSAGE_POLLS | Intents::DIRECT_MESSAGE_POLLS,
    )?;
    check_event_intents(
        intents,
        activity & EventTypeFlags::TYPING_START,
        Intents::GUILD_MESSAGE_TYPING | Intents::DIRECT_MESSAGE_TYPING,
    )?;
    let gateway = events.gateway_types(intents)?;
    Ok(message_lifecycle | reactions | activity | gateway)
}

fn check_reaction_intents(intents: Intents, events: EventTypeFlags) -> Result<()> {
    check_event_intents(
        intents,
        events,
        Intents::GUILD_MESSAGE_REACTIONS | Intents::DIRECT_MESSAGE_REACTIONS,
    )
}

fn check_event_intents(intents: Intents, events: EventTypeFlags, any_of: Intents) -> Result<()> {
    if events.is_empty() || intents.intersects(any_of) {
        return Ok(());
    }
    Err(Error::MissingEventIntents { any_of })
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

    /// 権限の確認や、ほかのメンバーの権限の取得に使う [`Access`] を返します。
    pub fn access(&self) -> Access<'a> {
        Access::for_message(self.http, self.permissions, self.message)
    }

    /// メンバー、ロール、VC の管理操作に使う [`Manage`] を返します。
    pub fn manage(&self) -> Manage<'a> {
        Manage::new(self.access())
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
        validation::message_content(content)?;
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
        validation::message_content(content)?;
        self.http
            .create_message(self.message.channel_id)
            .content(content)
            .reply(self.message.id)
            .allowed_mentions(Some(mentions))
            .await?;

        Ok(())
    }

    /// 元のメッセージにファイルを添付して返信します。添付だけで送るときは本文を空にします。
    ///
    /// # エラー
    /// 本文、添付の件数・サイズ・メタデータが無効な場合や、Discord が送信を拒否した場合に失敗します。
    pub async fn reply_files(
        &self,
        content: &str,
        files: &[twilight_model::http::attachment::Attachment],
        limits: validation::AttachmentLimits,
    ) -> Result<()> {
        validation::reply_files(content, files, limits, None)?;
        validation::channel_message_upload_size(files)?;
        let mentions = AllowedMentions::default();
        let mut request = self
            .http
            .create_message(self.message.channel_id)
            .reply(self.message.id)
            .allowed_mentions(Some(&mentions));
        if !content.is_empty() {
            request = request.content(content);
        }
        if !files.is_empty() {
            request = request.attachments(files);
        }
        channel_ui::send_create_message(self.http, request.try_into_request()?).await
    }

    /// 元のメッセージに Embed やボタンを付けて返信します。
    ///
    /// # エラー
    ///
    /// メッセージの構成が無効、または Discord が送信を受け付けない場合に失敗します。
    pub async fn reply_ui(&self, message: &ui::UiMessage) -> Result<()> {
        message.validate()?;
        if message.has_premium_button() {
            return channel_ui::reply_with_premium_button(
                self.http,
                self.message,
                message.content.as_deref(),
                &message.embeds,
                &message.components,
                None,
                &[],
            )
            .await;
        }
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
        message.validate_attachment_references(&[])?;
        validation::channel_message_upload_size(&message.attachments)?;
        if message.has_premium_button() {
            return channel_ui::reply_with_premium_button(
                self.http,
                self.message,
                None,
                &[],
                &message.components,
                Some(MessageFlags::IS_COMPONENTS_V2),
                &message.attachments,
            )
            .await;
        }
        let mentions = AllowedMentions::default();
        let mut request = self
            .http
            .create_message(self.message.channel_id)
            .reply(self.message.id)
            .allowed_mentions(Some(&mentions))
            .flags(MessageFlags::IS_COMPONENTS_V2)
            .components(&message.components);
        if !message.attachments.is_empty() {
            request = request.attachments(&message.attachments);
        }
        channel_ui::send_create_message(self.http, request.try_into_request()?).await
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
