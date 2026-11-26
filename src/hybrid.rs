use std::sync::Arc;

use twilight_model::{
    application::{command::CommandOption, interaction::application_command::CommandOptionValue},
    id::{Id, marker::UserMarker},
};

use crate::{CommandContext, Commands, Result, SlashCommands, SlashContext, Timing};

/// 同じ関数をテキストコマンドと、指定した場合はスラッシュコマンドに登録します。
///
/// `slash` が `None` のときは、テキストコマンドだけを登録します。
/// `description` と `options` は、スラッシュコマンドの表示と入力欄に使います。
///
/// # エラー
///
/// 既に使われている名前や登録対象の形式に合わない定義はエラーになります。
/// エラーの場合は、どちらの一覧にも新しいコマンドを追加しません。
pub fn register_hybrid(
    commands: &mut Commands,
    slash: Option<&mut SlashCommands>,
    name: &str,
    description: &str,
    options: Vec<CommandOption>,
    handler: impl AsyncFn(&mut HybridContext<'_>) -> Result<()> + Send + Sync + 'static,
) -> Result<()> {
    commands.ensure_available(name)?;
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
                handler.as_ref()(&mut ctx).await
            },
        )?;
    }

    // 先に確認してから登録する間、他の処理は `commands` を更新できない。
    commands.register(name, async move |ctx: CommandContext<'_>| {
        let mut ctx = HybridContext {
            inner: SourceContext::Text(ctx),
        };
        handler.as_ref()(&mut ctx).await
    })
}

/// コマンドの呼び出し方です。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CommandSource {
    /// `!ping` など、メッセージとして送信されたコマンドです。
    Text,
    /// `/ping` など、Discord のメニューから呼び出されたコマンドです。
    Slash,
}

/// テキストとスラッシュで同じコマンド関数を使うためのコンテキストです。
pub struct HybridContext<'a> {
    inner: SourceContext<'a>,
}

enum SourceContext<'a> {
    Text(CommandContext<'a>),
    Slash(SlashContext<'a>),
}

impl<'a> HybridContext<'a> {
    /// コマンドがメッセージとスラッシュのどちらで呼び出されたかを返します。
    pub fn source(&self) -> CommandSource {
        match &self.inner {
            SourceContext::Text(_) => CommandSource::Text,
            SourceContext::Slash(_) => CommandSource::Slash,
        }
    }

    /// 呼び出したユーザーの Discord ID を返します。
    pub fn author_id(&self) -> Option<Id<UserMarker>> {
        match &self.inner {
            SourceContext::Text(ctx) => Some(ctx.context().message().author.id),
            SourceContext::Slash(ctx) => ctx.author_id(),
        }
    }

    /// テキストコマンドの名前に続く本文です。スラッシュの場合は `None` になります。
    pub fn args(&self) -> Option<&'a str> {
        match &self.inner {
            SourceContext::Text(ctx) => Some(ctx.args()),
            SourceContext::Slash(_) => None,
        }
    }

    /// スラッシュコマンドの名前付き引数です。テキストの場合は `None` になります。
    pub fn option(&self, name: &str) -> Option<&'a CommandOptionValue> {
        match &self.inner {
            SourceContext::Text(_) => None,
            SourceContext::Slash(ctx) => ctx.option(name),
        }
    }

    /// 文字列として入力されたスラッシュコマンドの引数を借ります。
    pub fn string(&self, name: &str) -> Option<&'a str> {
        match &self.inner {
            SourceContext::Text(_) => None,
            SourceContext::Slash(ctx) => ctx.string(name),
        }
    }

    /// イベントの受信と、処理開始までの時間を調べる情報を返します。
    pub fn timing(&self) -> Timing {
        match &self.inner {
            SourceContext::Text(ctx) => ctx.timing(),
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
            SourceContext::Text(ctx) => ctx.reply(content).await,
            SourceContext::Slash(ctx) => ctx.reply(content).await,
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
    fn existing_slash_command_leaves_text_registry_unchanged() -> Result<()> {
        let mut text = Commands::new("!")?;
        let mut slash = SlashCommands::for_guild(Id::new(1));
        slash.register("ping", "説明", vec![], async |_| Ok(()))?;

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
        slash.register("ping", "説明", vec![], async |_| Ok(()))
    }
}
