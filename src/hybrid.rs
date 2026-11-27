use std::sync::Arc;

use twilight_model::{
    application::{
        command::{CommandOption, CommandOptionType},
        interaction::application_command::CommandOptionValue,
    },
    guild::Permissions,
    id::{
        Id,
        marker::{GuildMarker, UserMarker},
    },
};

use crate::{
    CommandContext, Commands, Cooldown, Error, Result, SlashCommands, SlashContext, Timing,
    error::handler_error,
    ui::{ComponentsV2, UiMessage},
};

mod arguments;
mod groups;
use arguments::HybridSchema;
pub use arguments::{HybridArgumentError, HybridArguments};
pub use groups::{HybridCommandGroup, register_hybrid_group, register_hybrid_group_with_aliases};

/// 型付きの引数を、テキストとスラッシュで共有するコマンドを登録します。
///
/// `options` の順序でテキストの引数を読みます。空白を含む文字列はクォートで囲み、
/// 省略できる引数は必須の引数より後ろに置きます。テキストで省略できるのは末尾からです。
/// スラッシュでは Discord が送った名前で引数を照合します。
///
/// 文字列・整数・数値・真偽値・ユーザー・ロール・チャンネルに対応します。
/// 真偽値は `true/false` や `yes/no`、ID は数字または種類に合う Discord のメンションで
/// 入力できます。ID の変換に Discord との通信は発生しません。チャンネル種類の指定には対応していません。
///
/// `slash` が `None` のときはテキストだけに登録します。入力が不正な場合はハンドラーを実行せず、
/// [`Error::HybridArgument`] を既存のエラー処理へ渡します。引数は [`HybridArguments`] から取得します。
///
/// # エラー
///
/// 重複したコマンドや Discord の制約に合わない定義を拒否します。
/// 登録に失敗した場合は、どちらの一覧にも追加しません。
pub fn register_typed_hybrid<E>(
    commands: &mut Commands,
    slash: Option<&mut SlashCommands>,
    name: &str,
    description: &str,
    options: Vec<CommandOption>,
    handler: impl for<'a> AsyncFn(
        &mut HybridContext<'a>,
        HybridArguments<'a>,
    ) -> std::result::Result<(), E>
    + Send
    + Sync
    + 'static,
) -> Result<()>
where
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    commands.ensure_available(name)?;
    twilight_validate::command::chat_input_name(name)?;
    twilight_validate::command::description(description)?;
    let typed = Arc::new(TypedHandler::new(options, handler)?);

    if let Some(slash) = slash {
        let options = typed.schema.options().to_vec();
        let typed = Arc::clone(&typed);
        slash.register(
            name,
            description,
            options,
            async move |ctx: &mut SlashContext<'_>| typed.slash(ctx).await,
        )?;
    }

    commands.register(name, async move |ctx: CommandContext<'_>| {
        typed.text(ctx).await
    })
}

struct TypedHandler<F> {
    schema: HybridSchema,
    handler: F,
}

impl<F> TypedHandler<F> {
    fn new(options: Vec<CommandOption>, handler: F) -> Result<Self> {
        Ok(Self {
            schema: HybridSchema::new(options)?,
            handler,
        })
    }

    async fn text<E>(&self, ctx: CommandContext<'_>) -> Result<()>
    where
        F: for<'a> AsyncFn(
            &mut HybridContext<'a>,
            HybridArguments<'a>,
        ) -> std::result::Result<(), E>,
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let arguments = self.schema.bind_text(ctx.args())?;
        let mut ctx = HybridContext {
            inner: SourceContext::Text {
                ctx,
                option_name: None,
            },
        };
        (self.handler)(&mut ctx, arguments)
            .await
            .map_err(handler_error)
    }

    async fn slash<E>(&self, ctx: &mut SlashContext<'_>) -> Result<()>
    where
        F: for<'a> AsyncFn(
            &mut HybridContext<'a>,
            HybridArguments<'a>,
        ) -> std::result::Result<(), E>,
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let arguments = self.schema.bind_slash(ctx.leaf_options())?;
        let mut ctx = HybridContext {
            inner: SourceContext::Slash(ctx.reborrow()),
        };
        (self.handler)(&mut ctx, arguments)
            .await
            .map_err(handler_error)
    }
}

/// 同じ関数をテキストコマンドと、指定した場合はスラッシュコマンドに登録します。
///
/// `slash` が `None` のときは、テキストコマンドだけを登録します。
/// `description` と `options` は、スラッシュコマンドの表示と入力欄に使います。
/// テキスト側でも、最初の文字列オプション名を [`HybridContext::text_arg`] に使います。
///
/// # エラー
///
/// 既に使われている名前や登録対象の形式に合わない定義はエラーになります。
/// エラーの場合は、どちらの一覧にも新しいコマンドを追加しません。
pub fn register_hybrid<E>(
    commands: &mut Commands,
    slash: Option<&mut SlashCommands>,
    name: &str,
    description: &str,
    options: Vec<CommandOption>,
    handler: impl AsyncFn(&mut HybridContext<'_>) -> std::result::Result<(), E> + Send + Sync + 'static,
) -> Result<()>
where
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    commands.ensure_available(name)?;
    let text_option = first_text_option(&options).map(str::to_owned);
    let handler = Arc::new(handler);

    if let Some(slash) = slash {
        let handler = Arc::clone(&handler);
        slash.register(
            name,
            description,
            options,
            async move |ctx: &mut SlashContext<'_>| {
                let mut ctx = HybridContext {
                    inner: SourceContext::Slash(ctx.reborrow()),
                };
                handler.as_ref()(&mut ctx).await.map_err(handler_error)
            },
        )?;
    }

    // 先に確認してから登録する間、他の処理は `commands` を更新できない。
    commands.register(name, async move |ctx: CommandContext<'_>| {
        let mut ctx = HybridContext {
            inner: SourceContext::Text {
                ctx,
                option_name: text_option.as_deref(),
            },
        };
        handler.as_ref()(&mut ctx).await.map_err(handler_error)
    })
}

fn first_text_option(options: &[CommandOption]) -> Option<&str> {
    options
        .iter()
        .find(|option| option.kind == CommandOptionType::String)
        .map(|option| option.name.as_str())
}

fn bound_text<'a>(option_name: Option<&str>, requested: &str, args: &'a str) -> Option<&'a str> {
    (option_name == Some(requested) && !args.trim().is_empty()).then_some(args)
}

/// コマンドの呼び出し方です。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum CommandSource {
    /// `!ping` など、メッセージとして送信されたコマンドです。
    Text,
    /// `/ping` など、Discord のメニューから呼び出されたコマンドです。
    Slash,
}

/// テキストとスラッシュで同じコマンド関数を使うためのコンテキストです。
#[derive(Debug)]
pub struct HybridContext<'a> {
    inner: SourceContext<'a>,
}

#[derive(Debug)]
enum SourceContext<'a> {
    Text {
        ctx: CommandContext<'a>,
        option_name: Option<&'a str>,
    },
    Slash(SlashContext<'a>),
}

impl<'a> HybridContext<'a> {
    /// Akari が使用している Twilight の REST クライアントを借ります。
    pub fn http(&self) -> &'a crate::http::Client {
        match &self.inner {
            SourceContext::Text { ctx, .. } => ctx.http(),
            SourceContext::Slash(ctx) => ctx.http(),
        }
    }

    /// コマンドがメッセージとスラッシュのどちらで呼び出されたかを返します。
    pub fn source(&self) -> CommandSource {
        match &self.inner {
            SourceContext::Text { .. } => CommandSource::Text,
            SourceContext::Slash(_) => CommandSource::Slash,
        }
    }

    /// 呼び出したユーザーの Discord ID を返します。
    pub fn author_id(&self) -> Option<Id<UserMarker>> {
        match &self.inner {
            SourceContext::Text { ctx, .. } => Some(ctx.context().message().author.id),
            SourceContext::Slash(ctx) => ctx.author_id(),
        }
    }

    /// コマンドが呼ばれたサーバーの ID を返します。DM では `None` です。
    pub fn guild_id(&self) -> Option<Id<GuildMarker>> {
        match &self.inner {
            SourceContext::Text { ctx, .. } => ctx.guild_id(),
            SourceContext::Slash(ctx) => ctx.guild_id(),
        }
    }

    /// サーバーの中で呼ばれたことを確認し、その ID を返します。
    ///
    /// # エラー
    ///
    /// DM などでは [`CheckFailure::GuildOnly`](crate::CheckFailure::GuildOnly) を返します。
    pub fn require_guild(&self) -> Result<Id<GuildMarker>> {
        self.guild_id()
            .ok_or_else(|| crate::CheckFailure::GuildOnly.into())
    }

    /// 呼び出した人の、このチャンネルでの権限を返します。
    ///
    /// スラッシュでは Discord が送った値を、テキストでは REST で計算した値を使います
    /// （[`Bot::cache_permissions`](crate::Bot::cache_permissions) で、計算に使う情報を保存できます）。
    ///
    /// # エラー
    ///
    /// DM では [`CheckFailure::GuildOnly`](crate::CheckFailure::GuildOnly)、権限が分からないときは
    /// [`CheckFailure::PermissionsUnavailable`](crate::CheckFailure::PermissionsUnavailable) を返します。
    pub async fn permissions(&self) -> Result<Permissions> {
        match &self.inner {
            SourceContext::Text { ctx, .. } => ctx.permissions().await,
            SourceContext::Slash(ctx) => {
                ctx.require_guild()?;
                ctx.permissions()
                    .ok_or_else(|| crate::CheckFailure::PermissionsUnavailable.into())
            }
        }
    }

    /// 呼び出した人が、指定した権限をすべて持っていることを確認します。
    ///
    /// # エラー
    ///
    /// 足りない場合は、不足分を入れた
    /// [`CheckFailure::MissingPermissions`](crate::CheckFailure::MissingPermissions) を返します。
    pub async fn require_permissions(&self, required: Permissions) -> Result<()> {
        match &self.inner {
            SourceContext::Text { ctx, .. } => ctx.require_permissions(required).await,
            SourceContext::Slash(ctx) => ctx.require_permissions(required),
        }
    }

    /// コマンドを実行する前に、ユーザーごとの待ち時間を確認します。
    ///
    /// 実行できる場合は使用を記録して `true` を返します。待ち時間が残っている場合は、
    /// 秒単位で切り上げて返信したあと `false` を返します。スラッシュでは本人にだけ表示します。
    ///
    /// # エラー
    ///
    /// 実行者の ID を取得できない場合や、待ち時間の通知を送れない場合はエラーを返します。
    pub async fn check_cooldown(&mut self, cooldown: &Cooldown) -> Result<bool> {
        let user = self.author_id().ok_or(Error::MissingAuthor)?;
        let Err(remaining) = cooldown.check(user) else {
            return Ok(true);
        };

        let seconds = u128::from(remaining.as_secs()) + u128::from(remaining.subsec_nanos() > 0);
        let notice = format!("あと {seconds} 秒待ってから試してください。");
        match &mut self.inner {
            SourceContext::Text { ctx, .. } => ctx.reply(&notice).await?,
            SourceContext::Slash(ctx) => ctx.reply_ephemeral(&notice).await?,
        }
        Ok(false)
    }

    /// テキストコマンドの名前に続く本文です。スラッシュの場合は `None` になります。
    pub fn args(&self) -> Option<&'a str> {
        match &self.inner {
            SourceContext::Text { ctx, .. } => Some(ctx.args()),
            SourceContext::Slash(_) => None,
        }
    }

    /// スラッシュコマンドの名前付き引数です。テキストの場合は `None` になります。
    pub fn option(&self, name: &str) -> Option<&'a CommandOptionValue> {
        match &self.inner {
            SourceContext::Text { .. } => None,
            SourceContext::Slash(ctx) => ctx.option(name),
        }
    }

    /// 文字列として入力されたスラッシュコマンドの引数を借ります。
    pub fn string(&self, name: &str) -> Option<&'a str> {
        match &self.inner {
            SourceContext::Text { .. } => None,
            SourceContext::Slash(ctx) => ctx.string(name),
        }
    }

    /// 両方の呼び出し方で、名前を指定して文字列引数を借ります。
    ///
    /// テキストでは、最上位に定義した最初の文字列オプションにコマンド名以降の本文を割り当てます。
    /// スラッシュでは、指定した名前の文字列オプションを参照します。
    /// 引数がない場合や、値が空または空白だけの場合は `None` を返します。
    pub fn text_arg(&self, name: &str) -> Option<&'a str> {
        match &self.inner {
            SourceContext::Text { ctx, option_name } => bound_text(*option_name, name, ctx.args()),
            SourceContext::Slash(ctx) => ctx.string(name).filter(|value| !value.trim().is_empty()),
        }
    }

    /// [`text_arg`](Self::text_arg) の値を [`FromStr`](std::str::FromStr) で変換します。
    ///
    /// 引数がない場合は `None`、入力があって変換に失敗したら `Some(Err(..))` を返します。
    /// 文字列の引数だけを対象にします（スラッシュの整数や真偽値は、
    /// [`option`](Self::option) で受け取ってください）。
    pub fn parse<T: std::str::FromStr>(
        &self,
        name: &str,
    ) -> Option<std::result::Result<T, T::Err>> {
        self.text_arg(name).map(|value| value.trim().parse())
    }

    /// イベントの受信と、処理開始までの時間を調べる情報を返します。
    pub fn timing(&self) -> Timing {
        match &self.inner {
            SourceContext::Text { ctx, .. } => ctx.timing(),
            SourceContext::Slash(ctx) => ctx.timing(),
        }
    }

    /// メッセージに返信するか、スラッシュコマンドの初回応答を送ります。
    ///
    /// # エラー
    ///
    /// Discord が返信を受け付けなかった場合や、スラッシュコマンドに応答済みの場合に失敗します。
    pub async fn reply(&mut self, content: &str) -> Result<()> {
        match &mut self.inner {
            SourceContext::Text { ctx, .. } => ctx.reply(content).await,
            SourceContext::Slash(ctx) => ctx.reply(content).await,
        }
    }

    /// テキストとスラッシュの両方で、Embed やボタンを付けて返信します。
    ///
    /// # エラー
    ///
    /// 無効な構成、スラッシュコマンドに応答済み、または Discord が送信を受け付けない場合に失敗します。
    pub async fn reply_ui(&mut self, message: &UiMessage) -> Result<()> {
        match &mut self.inner {
            SourceContext::Text { ctx, .. } => ctx.reply_ui(message).await,
            SourceContext::Slash(ctx) => ctx.reply_ui(message).await,
        }
    }

    /// テキストとスラッシュの両方で Components V2 を使って返信します。
    ///
    /// # エラー
    /// 無効なコンポーネント、スラッシュ応答済み、または Discord が受け付けない場合に失敗します。
    pub async fn reply_v2(&mut self, message: &ComponentsV2) -> Result<()> {
        match &mut self.inner {
            SourceContext::Text { ctx, .. } => ctx.reply_v2(message).await,
            SourceContext::Slash(ctx) => ctx.reply_v2(message).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn handler(_: &mut HybridContext<'_>) -> Result<()> {
        Ok(())
    }

    #[test]
    fn binds_message_remainder_to_first_top_level_string_option() {
        let mut count = SlashCommands::string_option("count", "個数", true);
        count.kind = CommandOptionType::Integer;
        let text = SlashCommands::string_option("text", "本文", true);
        let footer = SlashCommands::string_option("footer", "末尾", false);
        let options = [count, text, footer];

        let name = first_text_option(&options);
        assert_eq!(name, Some("text"));
        assert_eq!(
            bound_text(name, "text", "こんにちは 世界"),
            Some("こんにちは 世界")
        );
        assert_eq!(bound_text(name, "footer", "こんにちは 世界"), None);
        assert_eq!(first_text_option(&options[..1]), None);
        assert_eq!(bound_text(None, "text", "こんにちは 世界"), None);
        assert_eq!(bound_text(name, "text", ""), None);
        assert_eq!(bound_text(name, "text", " 　 "), None);
    }

    #[test]
    fn existing_slash_command_leaves_text_registry_unchanged() -> Result<()> {
        let mut text = Commands::new("!")?;
        let mut slash = SlashCommands::for_guild(Id::new(1));
        slash.register("ping", "説明", vec![], async |_| -> Result<()> { Ok(()) })?;

        let result = register_hybrid(&mut text, Some(&mut slash), "ping", "説明", vec![], handler);
        assert!(result.is_err());
        text.ensure_available("ping")
    }

    #[test]
    fn existing_text_command_leaves_slash_registry_unchanged() -> Result<()> {
        let mut text = Commands::new("!")?;
        let mut slash = SlashCommands::for_guild(Id::new(1));
        register_hybrid(&mut text, None, "ping", "説明", vec![], handler)?;

        let result = register_hybrid(&mut text, Some(&mut slash), "ping", "説明", vec![], handler);
        assert!(result.is_err());
        slash.register("ping", "説明", vec![], async |_| -> Result<()> { Ok(()) })
    }

    #[test]
    fn typed_registration_does_not_partially_add_commands() -> Result<()> {
        let mut text = Commands::new("!")?;
        let mut slash = SlashCommands::for_guild(Id::new(1));
        let typed =
            async |_: &mut HybridContext<'_>, _: HybridArguments<'_>| -> Result<()> { Ok(()) };
        slash.register("reserved", "説明", vec![], async |_| -> Result<()> {
            Ok(())
        })?;
        assert!(
            register_typed_hybrid(
                &mut text,
                Some(&mut slash),
                "reserved",
                "説明",
                vec![],
                typed
            )
            .is_err()
        );
        text.ensure_available("reserved")?;

        text.register_with_aliases("name", &["alias"], async |_| -> Result<()> { Ok(()) })?;
        assert!(
            register_typed_hybrid(&mut text, Some(&mut slash), "alias", "説明", vec![], typed)
                .is_err()
        );
        slash.register("alias", "説明", vec![], async |_| -> Result<()> {
            Ok(())
        })?;

        let mut unsupported = SlashCommands::string_option("file", "添付", true);
        unsupported.kind = CommandOptionType::Attachment;
        assert!(
            register_typed_hybrid(
                &mut text,
                Some(&mut slash),
                "invalid",
                "説明",
                vec![unsupported],
                typed
            )
            .is_err()
        );
        text.ensure_available("invalid")?;
        slash.register("invalid", "説明", vec![], async |_| -> Result<()> {
            Ok(())
        })
    }
}
