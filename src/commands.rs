use std::{future::Future, pin::Pin};

mod arguments;
mod groups;
mod help;
mod invoke;
mod prefix;
mod workers;
pub use arguments::{ArgumentError, ArgumentValueError, Arguments};
use groups::CommandTree;
pub use groups::TextCommandGroup;
pub use help::{CommandInfo, CommandMetadata};
use invoke::InvocationHooks;
pub use prefix::CommandPrefixes;
use prefix::PrefixPolicy;
pub(crate) use workers::text_workers;

use twilight_model::{
    channel::Message,
    guild::Permissions,
    id::{Id, marker::GuildMarker},
};

use crate::{
    CommandCheck, CommandCheckContext, Context, Error, ErrorSource, HandlerKind, RegistrationError,
    Result, Timing,
    command_check::run_checks,
    hooks::Hooks,
    ui::{ComponentsV2, UiMessage},
};

/// 共通のプレフィックス候補を使うテキストコマンドの登録先です。
pub struct Commands {
    prefixes: PrefixPolicy,
    entries: CommandTree,
    checks: Vec<CommandCheck>,
    invocation_hooks: InvocationHooks,
}

struct Invocation<'a> {
    handler: &'a dyn CommandHandler,
    command_name: &'a str,
    qualified_name: &'a str,
    invoked_with: &'a str,
    prefix: &'a str,
    args: &'a str,
}

impl<'a> Invocation<'a> {
    async fn call(&self, ctx: Context<'a>, commands: &'a Commands) -> Result<()> {
        if !commands.checks.is_empty() {
            run_checks(
                &commands.checks,
                CommandCheckContext::text(&ctx, self.qualified_name),
            )
            .await?;
        }
        self.handler
            .call(CommandContext {
                ctx,
                commands,
                command_name: self.command_name,
                qualified_name: self.qualified_name,
                invoked_with: self.invoked_with,
                prefix: self.prefix,
                args: self.args,
            })
            .await
    }
}

impl std::fmt::Debug for Commands {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Commands")
            .field("prefixes", &self.prefixes)
            .field("registered", &self.entries.names().collect::<Vec<_>>())
            .finish()
    }
}

impl Commands {
    /// コマンド名、別名、グループ名の照合で ASCII の英字の大文字・小文字を同一視します。
    /// プレフィックスと ASCII 以外の文字は入力どおりに比較します。
    /// 登録済みの正式名や別名の表記はそのまま保持します。
    ///
    /// # エラー
    /// どこかの階層で名前または別名が衝突する場合に失敗し、照合の設定は変わりません。
    pub fn enable_case_insensitive(&mut self) -> Result<()> {
        self.entries.enable_case_insensitive()
    }

    /// コマンドやグループのヘルプ用設定を置き換えます。
    ///
    /// `path` はプレフィックスを付けず、正式名を空白でつなぎます。
    ///
    /// # エラー
    /// 指定した正式名のコマンドやグループが見つからない場合に失敗します。
    pub fn set_metadata(&mut self, path: &str, metadata: CommandMetadata) -> Result<()> {
        self.entries.set_metadata(path, metadata).ok_or_else(|| {
            RegistrationError::UnknownCommand {
                name: path.to_owned(),
            }
            .into()
        })
    }

    /// 空白区切りの正式名または別名から、該当する登録情報を取得します。非表示のコマンドも取得できます。
    pub fn command_info(&self, path: &str) -> Option<CommandInfo<'_>> {
        self.entries.info(path)
    }

    /// 最上位のコマンドやグループを正式名順で取得します。非表示のコマンドも取得できます。
    pub fn command_infos(&self) -> Vec<CommandInfo<'_>> {
        self.entries.infos(false)
    }

    /// 一覧と個別の使い方を返す、標準のヘルプコマンドを指定の名前で登録します。
    ///
    /// 例えば `help` を登録すると `!help` や `!help config set` で使えます。
    /// 非表示に設定したコマンドとその配下は出しません。ヘルプを実行する際は通常のチェックを行い、
    /// 一覧に表示する各コマンドのチェックは呼びません。利用者に表示したくないコマンドは
    /// [`CommandMetadata::hidden`] で隠せます。
    ///
    /// # エラー
    /// 名前が無効、またはすでに使用中の場合に失敗します。
    pub fn register_help(&mut self, name: &str) -> Result<()> {
        self.register(name, help::reply_help)?;
        self.set_metadata(
            name,
            CommandMetadata::new()
                .description("コマンド一覧や使い方を表示します")
                .usage("[コマンド名]"),
        )
    }

    /// `!` などのプレフィックスを指定して、空のコマンド一覧を作ります。
    ///
    /// # エラー
    ///
    /// プレフィックスが空、または空白を含む場合に失敗します。
    pub fn new(prefix: &str) -> Result<Self> {
        Self::with_prefixes(&[prefix])
    }

    /// 同じコマンド一覧を呼び出せるプレフィックスをまとめて指定します。
    ///
    /// 複数の候補が同じ投稿に一致する場合は、最も長いものを使います。
    ///
    /// # エラー
    /// 候補がない場合や、空または空白を含む候補がある場合に失敗します。
    pub fn with_prefixes(prefixes: &[&str]) -> Result<Self> {
        if prefixes.is_empty() {
            return Err(RegistrationError::InvalidPrefix.into());
        }
        Ok(Self {
            prefixes: PrefixPolicy::Fixed(CommandPrefixes::new(prefixes)?),
            entries: CommandTree::default(),
            checks: Vec::new(),
            invocation_hooks: InvocationHooks::default(),
        })
    }

    /// 投稿ごとにプレフィックスの候補を返す非同期関数を指定します。
    ///
    /// [`Bot::run_commands`](crate::Bot::run_commands) で条件を満たした投稿は、
    /// コマンド実行と別の待ち行列で判定します。待ち行列に入った投稿ごとに最大 1 回関数を呼びます。
    /// 判定の同時数と待ち行列の長さは [`Limits`](crate::Limits) の
    /// `text_workers` と `queue_capacity` です。判定が混雑した場合は投稿に返信しません。
    /// 関数内の待機には非同期処理を使ってください。
    pub fn with_prefix_resolver<E>(
        resolver: impl AsyncFn(&Message) -> std::result::Result<CommandPrefixes, E>
        + Send
        + Sync
        + 'static,
    ) -> Self
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        Self {
            prefixes: PrefixPolicy::dynamic(resolver),
            entries: CommandTree::default(),
            checks: Vec::new(),
            invocation_hooks: InvocationHooks::default(),
        }
    }

    /// この一覧で実行するコマンドにチェックを追加します。
    /// コマンドを特定した後に登録順に実行し、最初のエラーで実行を止めます。
    pub fn add_check(&mut self, check: CommandCheck) {
        self.checks.push(check);
    }

    /// テキストコマンドの関数の直前に呼ぶ非同期フックを設定します。既存の設定は置き換えます。
    /// 権限などのチェックと、型付きハイブリッドの引数変換に成功した場合に呼びます。
    /// フックが失敗した場合は、コマンド関数と実行後フックを呼びません。
    pub fn set_before_invoke(
        &mut self,
        hook: impl AsyncFn(&CommandContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) {
        self.invocation_hooks.before = Some(Box::new(hook));
    }

    /// テキストコマンドの関数の直後に呼ぶ非同期フックを設定します。既存の設定は置き換えます。
    /// 関数がエラーや panic で終わった場合も呼び、そのエラーを渡します。
    /// 関数とフックが両方失敗した場合は、関数のエラーを返します。
    /// 処理がキャンセルされて Future が破棄される場合の後始末には `Drop` を使ってください。
    pub fn set_after_invoke(
        &mut self,
        hook: impl AsyncFn(&CommandContext<'_>, Option<&Error>) -> Result<()> + Send + Sync + 'static,
    ) {
        self.invocation_hooks.after = Some(Box::new(hook));
    }

    /// プレフィックスを除いた名前で `async fn` または async クロージャを登録します。
    ///
    /// # エラー
    ///
    /// 名前が空、空白を含む、または既に登録済みの場合に失敗します。
    pub fn register<E>(
        &mut self,
        name: &str,
        handler: impl for<'a> AsyncFn(CommandContext<'a>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.register_with_aliases(name, &[], handler)
    }

    /// 正式名と別名をまとめて登録し、どの名前からも同じ関数を実行します。
    ///
    /// 既定では大文字・小文字を区別します。[`enable_case_insensitive`](Self::enable_case_insensitive) で変更できます。
    ///
    /// # エラー
    ///
    /// 空の名前、空白を含む名前、重複する名前がある場合に失敗します。
    /// 失敗した場合は、正式名と別名のどちらも登録しません。
    pub fn register_with_aliases<E>(
        &mut self,
        name: &str,
        aliases: &[&str],
        handler: impl for<'a> AsyncFn(CommandContext<'a>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.register_handler(name, aliases, plain_handler!(handler))
    }

    pub(crate) fn register_handler(
        &mut self,
        name: &str,
        aliases: &[&str],
        handler: Box<dyn CommandHandler>,
    ) -> Result<()> {
        self.entries.register(name, aliases, handler)
    }

    /// サブコマンドやグループを登録するための、最上位グループを作ります。
    ///
    /// # エラー
    ///
    /// 名前が空、空白を含む、または登録済みの場合に失敗します。
    pub fn group(&mut self, name: &str) -> Result<TextCommandGroup<'_>> {
        self.group_with_aliases(name, &[])
    }

    /// 別名でも呼び出せる最上位グループを作ります。
    ///
    /// # エラー
    ///
    /// 無効な名前や同じ階層の重複を拒否します。失敗すると、グループも別名も登録しません。
    pub fn group_with_aliases(
        &mut self,
        name: &str,
        aliases: &[&str],
    ) -> Result<TextCommandGroup<'_>> {
        self.entries.group(name, aliases)
    }

    pub(crate) fn ensure_available(&self, name: &str) -> Result<()> {
        self.entries.ensure_available(name)
    }

    pub(crate) fn ensure_available_with_aliases(&self, name: &str, aliases: &[&str]) -> Result<()> {
        self.entries.validate_names(name, aliases)
    }

    pub(crate) fn stage_group(&self) -> Self {
        Self {
            prefixes: self.prefixes.clone(),
            entries: self.entries.empty_like(),
            checks: Vec::new(),
            invocation_hooks: InvocationHooks::default(),
        }
    }

    pub(crate) fn append_staged_group(&mut self, staged: Self) {
        self.entries.append(staged.entries);
    }

    /// メッセージに一致するコマンドを実行します。未登録の名前や通常の投稿は無視します。
    ///
    /// # エラー
    ///
    /// プレフィックスの判定、チェック、コマンド関数のエラーを呼び出し元に伝えます。
    pub async fn dispatch(&self, ctx: Context<'_>) -> Result<()> {
        if let Some(invocation) = self.resolve_message(ctx.message()).await? {
            invocation.call(ctx, self).await?;
        }

        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn matches_message(&self, content: &str) -> bool {
        self.resolve(content).is_some()
    }

    fn matching_static_prefix(&self, content: &str) -> Option<usize> {
        self.resolve(content)
            .map(|invocation| invocation.prefix.len())
    }

    async fn matching_prefix(&self, message: &Message) -> Result<Option<usize>> {
        Ok(self
            .resolve_message(message)
            .await?
            .map(|invocation| invocation.prefix.len()))
    }

    async fn resolve_message<'a>(&'a self, message: &'a Message) -> Result<Option<Invocation<'a>>> {
        let PrefixPolicy::Dynamic(resolver) = &self.prefixes else {
            return Ok(self.resolve(&message.content));
        };
        let prefixes = resolver.resolve(message).await?;
        Ok(prefixes
            .match_len(&message.content)
            .and_then(|prefix_len| self.resolve_selected(&message.content, prefix_len)))
    }

    /// コマンドを実行する。失敗はエラーの報告先へ渡し、文言が設定されていれば元の投稿へ返信する。
    async fn dispatch_with_feedback(&self, ctx: Context<'_>, prefix_len: usize, hooks: &Hooks) {
        let Some(invocation) = self.resolve_selected(&ctx.message().content, prefix_len) else {
            return;
        };
        let Err(error) = invocation.call(ctx, self).await else {
            return;
        };
        let name = Some(invocation.qualified_name);
        hooks
            .reporter
            .report(ErrorSource::TextCommand, name, &error);
        let text = if matches!(error, Error::Check(_)) {
            &hooks.messages.check_failed
        } else {
            &hooks.messages.text_command_failed
        };
        if let Some(text) = text
            && let Err(reply_error) = ctx.reply(text).await
        {
            hooks
                .reporter
                .report(ErrorSource::Response, name, &reply_error);
        }
    }

    fn resolve<'a>(&'a self, content: &'a str) -> Option<Invocation<'a>> {
        let PrefixPolicy::Fixed(prefixes) = &self.prefixes else {
            return None;
        };
        self.resolve_selected(content, prefixes.match_len(content)?)
    }

    fn resolve_selected<'a>(
        &'a self,
        content: &'a str,
        prefix_len: usize,
    ) -> Option<Invocation<'a>> {
        let (prefix, input) = content.split_at_checked(prefix_len)?;
        self.entries.resolve(prefix, input)
    }
}

fn duplicate_command(name: &str) -> Error {
    RegistrationError::Duplicate {
        kind: HandlerKind::TextCommand,
        name: name.to_owned(),
    }
    .into()
}

/// コマンド関数に渡す、引数と元のメッセージです。
#[derive(Debug)]
pub struct CommandContext<'a> {
    ctx: Context<'a>,
    commands: &'a Commands,
    command_name: &'a str,
    qualified_name: &'a str,
    invoked_with: &'a str,
    prefix: &'a str,
    args: &'a str,
}

impl<'a> CommandContext<'a> {
    /// 正式名または別名で、同じ一覧に登録済みのコマンド情報を取得します。
    /// 独自のヘルプで使えます。非表示の設定や権限を確認してから表示してください。
    pub fn command_info(&self, path: &str) -> Option<CommandInfo<'a>> {
        self.commands.command_info(path)
    }

    /// 同じ一覧の最上位コマンドを正式名順で取得します。非表示のコマンドも取得できます。
    pub fn command_infos(&self) -> Vec<CommandInfo<'a>> {
        self.commands.command_infos()
    }

    /// Akari が使用している Twilight の REST クライアントを借ります。
    pub fn http(&self) -> &'a crate::http::Client {
        self.ctx.http()
    }

    /// Gateway の往復時間と、このコマンドの処理時間を確認します。
    pub fn timing(&self) -> Timing {
        self.ctx.timing()
    }

    /// コマンド名より後ろの本文を返します。前後の空白は除去済みです。
    pub fn args(&self) -> &'a str {
        self.args
    }

    /// 最後に選ばれたコマンドの正式名を、親グループや別名を含めず返します。
    pub fn command_name(&self) -> &'a str {
        self.command_name
    }

    /// 親グループからコマンドまで、正式名を空白でつないだ名前を返します。
    ///
    /// たとえば `!c set` の `c` が `config` の別名なら `config set` です。
    pub fn qualified_name(&self) -> &'a str {
        self.qualified_name
    }

    /// メッセージで最後に使われたコマンド名を、登録した別名も含め入力どおりに返します。
    pub fn invoked_with(&self) -> &'a str {
        self.invoked_with
    }

    /// 今回の呼び出しに使われたプレフィックスを取得します。
    pub fn prefix(&self) -> &'a str {
        self.prefix
    }

    /// コマンド名の後ろから、空白やクォートを解釈しながら引数を読み取ります。
    pub fn arguments(&self) -> Arguments<'a> {
        Arguments::new(self.args)
    }

    /// 元のメッセージコンテキストを借ります。
    pub fn context(&self) -> &Context<'a> {
        &self.ctx
    }

    /// メッセージが送られたサーバーの ID を返します。DM では `None` です。
    pub fn guild_id(&self) -> Option<Id<GuildMarker>> {
        self.ctx.guild_id()
    }

    /// サーバーの中で送られたことを確認します。詳細は [`Context::require_guild`] を参照してください。
    ///
    /// # エラー
    ///
    /// DM などでは [`CheckFailure::GuildOnly`](crate::CheckFailure::GuildOnly) を返します。
    pub fn require_guild(&self) -> Result<Id<GuildMarker>> {
        self.ctx.require_guild()
    }

    /// 権限の確認や、Bot とほかのメンバーの権限の取得に使う [`Access`](crate::Access) を返します。
    pub fn access(&self) -> crate::Access<'a> {
        self.ctx.access()
    }

    /// 送り主の、このチャンネルでの権限を返します。詳細は [`Context::permissions`] を参照してください。
    ///
    /// # エラー
    ///
    /// 権限を計算できなかった場合に失敗します。
    pub async fn permissions(&self) -> Result<Permissions> {
        self.ctx.permissions().await
    }

    /// 送り主が、指定した権限をすべて持っていることを確認します。
    /// 詳細は [`Context::require_permissions`] を参照してください。
    ///
    /// # エラー
    ///
    /// 足りない場合は [`CheckFailure::MissingPermissions`](crate::CheckFailure::MissingPermissions) を返します。
    pub async fn require_permissions(&self, required: Permissions) -> Result<()> {
        self.ctx.require_permissions(required).await
    }

    /// 元のメッセージに返信します。詳細は [`Context::reply`] を参照してください。
    ///
    /// # エラー
    ///
    /// 返信を送信できなかった場合に失敗します。
    pub async fn reply(&self, content: &str) -> Result<()> {
        self.ctx.reply(content).await
    }

    /// 通知する相手を指定して返信します。詳細は [`Context::reply_with`] を参照してください。
    ///
    /// # エラー
    ///
    /// 返信を送信できなかった場合に失敗します。
    pub async fn reply_with(
        &self,
        content: &str,
        mentions: &twilight_model::channel::message::AllowedMentions,
    ) -> Result<()> {
        self.ctx.reply_with(content, mentions).await
    }

    /// 元のコマンドに Embed やボタンを付けて返信します。
    ///
    /// # エラー
    ///
    /// 詳細は [`Context::reply_ui`] を参照してください。
    pub async fn reply_ui(&self, message: &UiMessage) -> Result<()> {
        self.ctx.reply_ui(message).await
    }

    /// 元のコマンドに Components V2 で返信します。
    /// 詳細は [`Context::reply_v2`] を参照してください。
    pub async fn reply_v2(&self, message: &ComponentsV2) -> Result<()> {
        self.ctx.reply_v2(message).await
    }

    /// 添付ファイルを含む返信を送ります。詳細は [`Context::reply_files`] を参照してください。
    ///
    /// # エラー
    /// メッセージやファイルが無効、または Discord への送信に失敗した場合にエラーになります。
    pub async fn reply_files(
        &self,
        content: &str,
        files: &[twilight_model::http::attachment::Attachment],
        limits: crate::validation::AttachmentLimits,
    ) -> Result<()> {
        self.ctx.reply_files(content, files, limits).await
    }
}

pub(crate) trait CommandHandler: Send + Sync {
    fn call<'a>(
        &'a self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>>;
}

struct PlainHandler<F>(F);

impl<F> CommandHandler for PlainHandler<F>
where
    F: for<'a> AsyncFn(CommandContext<'a>) -> Result<()> + Send + Sync,
{
    fn call<'a>(
        &'a self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        Box::pin(ctx.invoke_prepared(&self.0))
    }
}

macro_rules! plain_handler {
    ($handler:ident) => {
        Box::new($crate::commands::PlainHandler(
            async move |ctx: $crate::CommandContext<'_>| {
                $handler(ctx).await.map_err($crate::error::handler_error)
            },
        ))
    };
}
use plain_handler;

// 型付きの引数を変換する登録は、その変換が済んでからフックを呼ぶ。
pub(crate) struct PreparedTextHandler<F>(pub(crate) F);

impl<F> CommandHandler for PreparedTextHandler<F>
where
    F: for<'a> AsyncFn(CommandContext<'a>) -> Result<()> + Send + Sync,
{
    fn call<'a>(
        &'a self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        Box::pin(self.0(ctx))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use twilight_http::Client;
    use twilight_model::channel::Message;

    use super::{
        CommandContext, Commands, Context, Error, ErrorSource, HandlerKind, RegistrationError,
        Result,
    };
    use crate::{
        hooks::{Hooks, Reporter},
        permissions::PermissionResolver,
        timing::Pending,
    };

    async fn command(_: CommandContext<'_>) -> Result<()> {
        Ok(())
    }

    #[test]
    fn resolves_only_registered_commands_and_borrows_arguments() -> Result<()> {
        let mut commands = Commands::new("!")?;
        commands.register("ping", command)?;
        commands.register("echo", command)?;

        assert_eq!(commands.resolve("!ping").map(|call| call.args), Some(""));
        assert!(commands.matches_message("!ping"));
        assert_eq!(
            commands
                .resolve("!echo\t こんにちは  世界 ")
                .map(|call| call.args),
            Some("こんにちは  世界")
        );
        for input in ["ping", "!", "! ping", "!pinged", "!PING", "/ping"] {
            assert!(commands.resolve(input).is_none(), "{input}");
            assert!(!commands.matches_message(input), "{input}");
        }
        Ok(())
    }

    #[test]
    fn rejects_invalid_names_and_duplicate_registrations() -> Result<()> {
        assert!(Commands::new("").is_err());
        assert!(Commands::new("! ").is_err());
        let mut commands = Commands::new("!")?;
        assert!(commands.register("", command).is_err());
        assert!(commands.register("two words", command).is_err());
        commands.register("ping", command)?;
        assert!(commands.register("ping", command).is_err());
        assert!(commands.resolve("!ping").is_some());
        Ok(())
    }

    #[test]
    fn aliases_share_the_command_namespace_and_register_atomically() -> Result<()> {
        let mut commands = Commands::new("!")?;
        commands.register_with_aliases("ping", &["p", "ポン"], command)?;

        for aliases in [
            &["greet", "ping"][..],
            &["greet", "p"],
            &["greet", "greet"],
            &["greet", "hello"],
        ] {
            let conflicting_name = aliases[1];
            assert!(matches!(
                commands.register_with_aliases("hello", aliases, command),
                Err(Error::Registration(RegistrationError::Duplicate {
                    kind: HandlerKind::TextCommand, name,
                })) if name == conflicting_name
            ));
            assert!(!commands.matches_message("!hello"));
            assert!(!commands.matches_message("!greet"));
        }
        assert!(matches!(
            commands.register_with_aliases("hello", &["greet", "two words"], command),
            Err(Error::Registration(RegistrationError::InvalidCommandName))
        ));
        assert!(!commands.matches_message("!hello"));
        assert!(!commands.matches_message("!greet"));
        assert!(matches!(
            commands.register("p", command),
            Err(Error::Registration(RegistrationError::Duplicate {
                kind: HandlerKind::TextCommand, name,
            })) if name == "p"
        ));
        assert!(commands.matches_message("!ping"));
        assert!(commands.matches_message("!p"));
        assert!(commands.matches_message("!ポン"));
        assert!(!commands.matches_message("!P"));
        Ok(())
    }

    #[test]
    fn ascii_case_insensitive_lookup_keeps_original_names_and_prefixes() -> Result<()> {
        let mut commands = Commands::new("Akari.")?;
        let mut root = commands.group_with_aliases("Config", &["Cfg"])?;
        root.group_with_aliases("Advanced", &["Adv"])?
            .subcommand_with_aliases("Set", &["Put"], command)?;
        commands.register("Café", command)?;
        assert!(commands.resolve("Akari.CFG adv PUT 変更").is_none());

        commands.enable_case_insensitive()?;
        commands.enable_case_insensitive()?;
        let resolved = commands.resolve("Akari.CFG adv PUT 変更").unwrap();
        assert_eq!(resolved.command_name, "Set");
        assert_eq!(resolved.qualified_name, "Config Advanced Set");
        assert_eq!(resolved.invoked_with, "PUT");
        assert_eq!(resolved.args, "変更");
        assert_eq!(
            commands.command_info("CFG adv put").unwrap().aliases(),
            ["Put"]
        );
        assert!(commands.resolve("akari.Cfg Adv Set").is_none());
        assert!(commands.resolve("Akari.CAFé").is_some());
        assert!(commands.resolve("Akari.CAFÉ").is_none());
        Ok(())
    }

    #[test]
    fn case_insensitive_activation_is_atomic_when_a_nested_alias_conflicts() -> Result<()> {
        let mut commands = Commands::new("!")?;
        commands.register("Ping", command)?;
        let mut group = commands.group("group")?;
        group.subcommand_with_aliases("first", &["SAME"], command)?;
        group.subcommand_with_aliases("second", &["same"], command)?;

        assert!(matches!(
            commands.enable_case_insensitive(),
            Err(Error::Registration(RegistrationError::Duplicate {
                kind: HandlerKind::TextCommand,
                ..
            }))
        ));
        assert!(commands.resolve("!PING").is_none());
        assert!(commands.resolve("!group SaMe").is_none());
        assert_eq!(
            commands.resolve("!group SAME").unwrap().command_name,
            "first"
        );
        assert_eq!(
            commands.resolve("!group same").unwrap().command_name,
            "second"
        );
        commands.register("PING", command)?;
        Ok(())
    }

    #[test]
    fn enabled_case_insensitivity_applies_to_new_and_staged_hybrid_groups() -> Result<()> {
        use crate::{HybridArguments, HybridContext, register_hybrid_group_with_aliases};

        let mut commands = Commands::new("!")?;
        commands.enable_case_insensitive()?;
        commands.register_with_aliases("Ping", &["P"], command)?;
        assert!(commands.register("p", command).is_err());
        assert!(
            commands
                .register_with_aliases("Echo", &["e", "E"], command)
                .is_err()
        );
        assert!(
            commands
                .register_with_aliases("Echo", &["ECHO"], command)
                .is_err()
        );
        assert!(commands.resolve("!echo").is_none());
        assert!(commands.resolve("!E").is_none());
        register_hybrid_group_with_aliases(
            &mut commands,
            None,
            "config",
            "設定",
            &["CFG"],
            |group| {
                let callback = async |_: &mut HybridContext<'_>,
                                      _: HybridArguments<'_>|
                       -> Result<()> { Ok(()) };
                group.subcommand_with_aliases("first", "最初", &["Set"], vec![], callback)?;
                assert!(
                    group
                        .subcommand_with_aliases("second", "二番目", &["SET"], vec![], callback)
                        .is_err()
                );
                group.subcommand_with_aliases("second", "二番目", &["Write"], vec![], callback)
            },
        )?;
        assert_eq!(
            commands.resolve("!cfg SET").unwrap().qualified_name,
            "config first"
        );
        assert_eq!(
            commands.resolve("!CfG WRITE").unwrap().qualified_name,
            "config second"
        );
        assert!(commands.register("CFG", command).is_err());
        Ok(())
    }

    #[test]
    fn longest_prefix_wins_for_canonical_names_and_aliases() -> Result<()> {
        assert!(Commands::with_prefixes(&[]).is_err());
        assert!(Commands::with_prefixes(&["!", " "]).is_err());
        let mut commands = Commands::with_prefixes(&["!", "akari.", "!!"])?;
        commands.register_with_aliases("echo", &["e"], async |_| -> Result<()> { Ok(()) })?;
        commands.register("!echo", async |_| -> Result<()> { Ok(()) })?;

        let invocation = commands.resolve("!!echo こんにちは").unwrap();
        assert_eq!(invocation.prefix, "!!");
        assert_eq!(invocation.command_name, "echo");
        assert_eq!(invocation.args, "こんにちは");
        let invocation = commands.resolve("akari.e").unwrap();
        assert_eq!(invocation.prefix, "akari.");
        assert_eq!(invocation.invoked_with, "e");
        assert!(!commands.matches_message("/echo"));
        Ok(())
    }

    #[test]
    fn groups_route_exact_tokens_greedily_and_do_not_fall_back_on_unknown_children() -> Result<()> {
        let mut commands = Commands::with_prefixes(&["!", "!!"])?;
        let mut config = commands.group_with_aliases("config", &["c"])?;
        config.fallback(command)?;
        config.subcommand("list", command)?;
        let mut advanced = config.group_with_aliases("advanced", &["a"])?;
        advanced.subcommand_with_aliases("set", &["s"], command)?;

        let bare = commands.resolve("!c \t").unwrap();
        assert_eq!(
            (
                bare.command_name,
                bare.qualified_name,
                bare.invoked_with,
                bare.args
            ),
            ("config", "config", "c", "")
        );
        let list = commands.resolve("!config list one two").unwrap();
        assert_eq!((list.qualified_name, list.args), ("config list", "one two"));
        let leaf = commands.resolve(r#"!!c　a s 「二つの 値」 2"#).unwrap();
        assert_eq!(
            (
                leaf.prefix,
                leaf.command_name,
                leaf.qualified_name,
                leaf.invoked_with,
                leaf.args
            ),
            ("!!", "set", "config advanced set", "s", "「二つの 値」 2"),
        );
        for input in [
            "!c a",
            "!c x",
            "!c lists",
            "!c a setting",
            "!c a SET",
            "! c",
            "!cc list",
            "!c a#s",
        ] {
            assert!(!commands.matches_message(input), "{input}");
        }
        Ok(())
    }

    #[test]
    fn group_aliases_share_only_the_local_namespace_with_leaf_commands() -> Result<()> {
        let mut commands = Commands::new("!")?;
        let mut first = commands.group("first")?;
        first.subcommand_with_aliases("echo", &["e"], command)?;
        assert!(matches!(
            first.group_with_aliases("other", &["o", "e"]),
            Err(Error::Registration(RegistrationError::Duplicate {
                kind: HandlerKind::TextCommand, name
            })) if name == "first e"
        ));
        first.group_with_aliases("other", &["o"])?;
        let mut second = commands.group_with_aliases("second", &["s"])?;
        second.subcommand_with_aliases("echo", &["e"], command)?;
        second.fallback(command)?;
        assert!(matches!(
            second.fallback(command),
            Err(Error::Registration(RegistrationError::Duplicate {
                kind: HandlerKind::TextCommand, name
            })) if name == "second"
        ));
        assert!(commands.register("s", command).is_err());
        assert!(commands.matches_message("!first e"));
        assert!(commands.matches_message("!s e"));
        assert!(commands.matches_message("!s"));
        Ok(())
    }

    #[tokio::test]
    async fn dispatch_reports_canonical_name_and_passes_the_invoked_alias() -> Result<()> {
        let mut commands = Commands::new("!")?;
        let mut config = commands.group_with_aliases("config", &["c"])?;
        config.subcommand_with_aliases(
            "echo",
            &["e"],
            async |ctx: CommandContext<'_>| -> Result<()> {
                assert_eq!(ctx.command_name(), "echo");
                assert_eq!(ctx.qualified_name(), "config echo");
                assert_eq!(ctx.invoked_with(), "e");
                assert_eq!(ctx.args(), "こんにちは  世界");
                assert_eq!(
                    ctx.command_info("c e").unwrap().qualified_name(),
                    "config echo"
                );
                assert_eq!(ctx.command_infos()[0].name(), "config");
                Err(Error::msg("test command failure"))
            },
        )?;

        let message = command_message("!c e  こんにちは  世界 ")?;
        let http = Client::new("test".into());
        let permissions = PermissionResolver::new(None);
        let (message, timing) = Pending::new(&message, None).begin();
        let ctx = Context {
            http: &http,
            message,
            timing,
            permissions: &permissions,
        };
        let names = Arc::new(Mutex::new(Vec::new()));
        let collected = Arc::clone(&names);
        let hooks = Hooks {
            reporter: Reporter::new(move |report| {
                assert_eq!(report.source, ErrorSource::TextCommand);
                assert_eq!(report.error.to_string(), "test command failure");
                collected
                    .lock()
                    .unwrap()
                    .push(report.name.map(str::to_owned));
            }),
            ..Hooks::default()
        };
        assert!(commands.dispatch(ctx).await.is_err());
        commands
            .dispatch_with_feedback(ctx, "!".len(), &hooks)
            .await;
        assert_eq!(*names.lock().unwrap(), [Some("config echo".to_owned())]);
        Ok(())
    }

    pub(super) fn command_message(content: &str) -> Result<Message> {
        let mut message: Message = serde_json::from_str(
            r#"{
            "id": "1", "channel_id": "2", "type": 0,
            "author": {"id": "3", "username": "member", "discriminator": "0"},
            "content": "", "timestamp": "2026-01-01T00:00:00.000000+00:00",
            "attachments": [], "embeds": [], "mention_everyone": false, "mention_roles": [],
            "mentions": [], "pinned": false, "tts": false
        }"#,
        )
        .map_err(Error::handler)?;
        message.content = content.into();
        Ok(message)
    }

    #[tokio::test]
    async fn checks_use_canonical_route_and_stop_on_first_denial() -> Result<()> {
        use crate::{CheckFailure, CommandCheck, CommandCheckContext, CommandCheckSource};

        let mut commands = Commands::new("!")?;
        commands
            .group_with_aliases("config", &["c"])?
            .subcommand_with_aliases(
                "echo",
                &["e"],
                async |_: CommandContext<'_>| -> Result<()> { panic!("denied handler ran") },
            )?;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let first = Arc::clone(&calls);
        commands.add_check(CommandCheck::new(
            async move |ctx: CommandCheckContext<'_>| {
                assert_eq!(ctx.source(), CommandCheckSource::Text);
                assert!(ctx.command_path().eq(["config", "echo"]));
                assert_eq!(ctx.author_id().map(twilight_model::id::Id::get), Some(3));
                assert_eq!(ctx.channel_id().map(twilight_model::id::Id::get), Some(2));
                first.lock().unwrap().push(1);
                Ok(())
            },
        ));
        let second = Arc::clone(&calls);
        commands.add_check(CommandCheck::new(async move |_| {
            second.lock().unwrap().push(2);
            Err(CheckFailure::Rejected.into())
        }));
        commands.add_check(CommandCheck::new(async |_| panic!("later check ran")));

        let http = Client::new("test".into());
        let permissions = PermissionResolver::new(None);
        let message = command_message("!c e")?;
        let unknown = command_message("!c missing")?;
        let (_, timing) = Pending::new((), None).begin();
        let ctx = Context {
            http: &http,
            message: &message,
            timing,
            permissions: &permissions,
        };
        commands
            .dispatch(Context {
                message: &unknown,
                ..ctx
            })
            .await?;
        assert!(calls.lock().unwrap().is_empty());
        assert!(matches!(
            commands.dispatch(ctx).await,
            Err(Error::Check(CheckFailure::Rejected))
        ));
        assert_eq!(*calls.lock().unwrap(), [1, 2]);

        commands.checks.remove(1);
        assert!(matches!(
            commands.dispatch(ctx).await,
            Err(Error::HandlerPanic { .. })
        ));
        Ok(())
    }
}
