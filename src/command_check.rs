use std::{future::Future, pin::Pin, sync::Arc};

use twilight_model::{
    application::{command::CommandType, interaction::Interaction},
    guild::Permissions,
    id::{
        Id,
        marker::{ChannelMarker, GuildMarker, UserMarker},
    },
};

use crate::{
    Access, CheckFailure, Context, Result,
    error::adapt_handler,
    permissions::check_permissions,
    slash::{interaction_channel_id, interaction_permissions},
};

#[cfg(test)]
mod tests;

/// コマンドの実行前に使う、共有可能な非同期チェックです。
#[derive(Clone)]
pub struct CommandCheck(Arc<dyn CheckHandler>);

impl CommandCheck {
    /// `Ok(())` で実行を許可し、拒否には [`CheckFailure::Rejected`] などを返します。
    /// アプリコマンドと補完では初回応答の前に実行するため、応答期限内に完了する処理を登録してください。
    pub fn new(
        check: impl for<'a> AsyncFn(CommandCheckContext<'a>) -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(
            adapt_handler!(check, ctx: CommandCheckContext<'_>),
        ))
    }
}

impl std::fmt::Debug for CommandCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandCheck").finish_non_exhaustive()
    }
}

trait CheckHandler: Send + Sync {
    fn call<'a>(
        &'a self,
        ctx: CommandCheckContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>>;
}

impl<F> CheckHandler for F
where
    F: for<'a> AsyncFn(CommandCheckContext<'a>) -> Result<()> + Send + Sync + 'static,
{
    fn call<'a>(
        &'a self,
        ctx: CommandCheckContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        Box::pin(self(ctx))
    }
}

pub(crate) async fn run_checks(
    checks: &[CommandCheck],
    ctx: CommandCheckContext<'_>,
) -> Result<()> {
    run_invocation_checks(&[], checks, ctx).await
}

/// 通常のコマンド実行では、Bot 全体の一度限りのチェックを先に行う。
/// 補完やヘルプの表示候補には `run_checks` を使う。
pub(crate) async fn run_invocation_checks(
    once: &[CommandCheck],
    checks: &[CommandCheck],
    ctx: CommandCheckContext<'_>,
) -> Result<()> {
    for check in once.iter().chain(checks) {
        check.0.call(ctx).await?;
    }
    Ok(())
}

/// コマンドや補完の呼び出し元です。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CommandCheckSource {
    /// プレフィックス付きのメッセージです。
    Text,
    /// スラッシュコマンド、ユーザーメニュー、メッセージメニューのいずれかです。
    Application(CommandType),
    /// スラッシュコマンドの引数補完です。
    Autocomplete,
}

/// コマンドの呼び出し元と権限を、応答する前に確認するための情報です。
#[derive(Clone, Copy)]
pub struct CommandCheckContext<'a> {
    origin: Origin<'a>,
}

#[derive(Clone, Copy)]
enum Origin<'a> {
    Text {
        ctx: &'a Context<'a>,
        qualified_name: &'a str,
    },
    Application {
        interaction: &'a Interaction,
        access: Access<'a>,
        kind: CommandCheckSource,
        root: &'a str,
        path: &'a [&'a str],
    },
}

impl<'a> CommandCheckContext<'a> {
    pub(crate) const fn text(ctx: &'a Context<'a>, qualified_name: &'a str) -> Self {
        Self {
            origin: Origin::Text {
                ctx,
                qualified_name,
            },
        }
    }

    pub(crate) const fn application(
        interaction: &'a Interaction,
        access: Access<'a>,
        kind: CommandCheckSource,
        root: &'a str,
        path: &'a [&'a str],
    ) -> Self {
        Self {
            origin: Origin::Application {
                interaction,
                access,
                kind,
                root,
                path,
            },
        }
    }

    /// 呼び出しの種類です。
    pub fn source(&self) -> CommandCheckSource {
        match self.origin {
            Origin::Text { .. } => CommandCheckSource::Text,
            Origin::Application { kind, .. } => kind,
        }
    }

    /// コマンドの正式名を、親から順に返します。コンテキストメニュー名は空白を含んでも一つの要素です。
    /// テキストコマンドで別名が使われた場合も、登録時の正式名を返します。
    pub fn command_path(&self) -> impl Iterator<Item = &'a str> {
        let (text, root, path) = match self.origin {
            Origin::Text { qualified_name, .. } => (Some(qualified_name), None, &[][..]),
            Origin::Application { root, path, .. } => (None, Some(root), path),
        };
        text.into_iter()
            .flat_map(str::split_whitespace)
            .chain(root)
            .chain(path.iter().copied())
    }

    /// 権限の確認や、Bot とほかのメンバーの権限の取得に使う [`Access`] を返します。
    pub fn access(&self) -> Access<'a> {
        match self.origin {
            Origin::Text { ctx, .. } => ctx.access(),
            Origin::Application { access, .. } => access,
        }
    }

    /// 呼び出したユーザーです。Interaction にユーザー情報がなければ `None` です。
    pub fn author_id(&self) -> Option<Id<UserMarker>> {
        match self.origin {
            Origin::Text { ctx, .. } => Some(ctx.message().author.id),
            Origin::Application { interaction, .. } => interaction.author_id(),
        }
    }

    /// 呼び出されたサーバーです。DM では `None` です。
    pub fn guild_id(&self) -> Option<Id<GuildMarker>> {
        match self.origin {
            Origin::Text { ctx, .. } => ctx.guild_id(),
            Origin::Application { interaction, .. } => interaction.guild_id,
        }
    }

    /// 呼び出されたチャンネルです。Interaction にチャンネル情報がなければ `None` です。
    pub fn channel_id(&self) -> Option<Id<ChannelMarker>> {
        match self.origin {
            Origin::Text { ctx, .. } => Some(ctx.message().channel_id),
            Origin::Application { interaction, .. } => interaction_channel_id(interaction),
        }
    }

    /// サーバーで呼び出されたことを確認します。
    ///
    /// # エラー
    /// DM などでは [`CheckFailure::GuildOnly`] を返します。
    pub fn require_guild(&self) -> Result<Id<GuildMarker>> {
        self.guild_id()
            .ok_or_else(|| CheckFailure::GuildOnly.into())
    }

    /// 呼び出した人が、チャンネルで必要な権限を持つことを確認します。
    /// テキストコマンドでは [`Context::require_permissions`] と同じ権限計算を使います。
    ///
    /// # エラー
    /// 権限が足りない場合や、確認できない場合に失敗します。
    pub async fn require_permissions(&self, required: Permissions) -> Result<()> {
        match self.origin {
            Origin::Text { ctx, .. } => ctx.require_permissions(required).await,
            Origin::Application { interaction, .. } => {
                check_permissions(
                    interaction.guild_id,
                    interaction_permissions(interaction),
                    required,
                )?;
                Ok(())
            }
        }
    }
}
