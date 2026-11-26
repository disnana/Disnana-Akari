use std::sync::Arc;

use twilight_model::{
    application::{
        command::{CommandOption, CommandOptionType},
        interaction::application_command::CommandOptionValue,
    },
    id::{Id, marker::UserMarker},
};

use crate::{CommandContext, Commands, Result, SlashCommands, SlashContext, Timing};

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
pub fn register_hybrid(
    commands: &mut Commands,
    slash: Option<&mut SlashCommands>,
    name: &str,
    description: &str,
    options: Vec<CommandOption>,
    handler: impl AsyncFn(&mut HybridContext<'_>) -> Result<()> + Send + Sync + 'static,
) -> Result<()> {
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
                handler.as_ref()(&mut ctx).await
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
        handler.as_ref()(&mut ctx).await
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
    Text {
        ctx: CommandContext<'a>,
        option_name: Option<&'a str>,
    },
    Slash(SlashContext<'a>),
}

impl<'a> HybridContext<'a> {
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
