use std::{collections::HashMap, future::Future, pin::Pin};

use crate::{Context, Result, Timing};

/// 共通のプレフィックスを使うテキストコマンドの登録先です。
pub struct Commands {
    prefix: String,
    handlers: HashMap<String, Box<dyn CommandHandler>>,
}

impl Commands {
    /// `!` などのプレフィックスを指定して、空のコマンド一覧を作ります。
    ///
    /// # エラー
    ///
    /// プレフィックスが空、または空白を含む場合に失敗します。
    pub fn new(prefix: &str) -> Result<Self> {
        if prefix.is_empty() || prefix.chars().any(char::is_whitespace) {
            return Err("プレフィックスには空白を含まない文字列を指定してください".into());
        }

        Ok(Self {
            prefix: prefix.into(),
            handlers: HashMap::new(),
        })
    }

    /// プレフィックスを除いた名前で `async fn` または async クロージャを登録します。
    ///
    /// # エラー
    ///
    /// 名前が空、空白を含む、または既に登録済みの場合に失敗します。
    pub fn register(
        &mut self,
        name: &str,
        handler: impl for<'a> AsyncFn(CommandContext<'a>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        self.ensure_available(name)?;
        self.handlers.insert(name.into(), Box::new(handler));
        Ok(())
    }

    pub(crate) fn ensure_available(&self, name: &str) -> Result<()> {
        if name.is_empty() || name.chars().any(char::is_whitespace) {
            return Err("コマンド名には空白を含まない文字列を指定してください".into());
        }
        if self.handlers.contains_key(name) {
            return Err(format!("コマンド `{name}` は登録済みです").into());
        }
        Ok(())
    }

    /// メッセージに一致するコマンドを実行します。未登録の名前や通常の投稿は無視します。
    ///
    /// # エラー
    ///
    /// コマンド関数が返したエラーを呼び出し元に伝えます。
    pub async fn dispatch(&self, ctx: Context<'_>) -> Result<()> {
        if let Some((handler, args)) = self.resolve(&ctx.message().content) {
            handler.call(CommandContext { ctx, args }).await?;
        }

        Ok(())
    }

    fn resolve<'a>(&self, content: &'a str) -> Option<(&dyn CommandHandler, &'a str)> {
        let input = content.strip_prefix(&self.prefix)?;
        let (name, args) = input.split_once(char::is_whitespace).unwrap_or((input, ""));
        let handler = self.handlers.get(name)?.as_ref();
        Some((handler, args.trim()))
    }
}

/// コマンド関数に渡す、引数と元のメッセージです。
pub struct CommandContext<'a> {
    ctx: Context<'a>,
    args: &'a str,
}

impl<'a> CommandContext<'a> {
    /// Gateway の往復時間と、このコマンドの処理時間を確認します。
    pub fn timing(&self) -> Timing {
        self.ctx.timing()
    }

    /// コマンド名より後ろの本文を返します。前後の空白は除去済みです。
    pub fn args(&self) -> &'a str {
        self.args
    }

    /// 元のメッセージコンテキストを借ります。
    pub fn context(&self) -> &Context<'a> {
        &self.ctx
    }

    /// 元のメッセージに返信します。詳細は [`Context::reply`] を参照してください。
    ///
    /// # エラー
    ///
    /// 返信を送信できなかった場合に失敗します。
    pub async fn reply(&self, content: &str) -> Result<()> {
        self.ctx.reply(content).await
    }
}

trait CommandHandler: Send + Sync {
    fn call<'a>(
        &'a self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>>;
}

impl<F> CommandHandler for F
where
    F: for<'a> AsyncFn(CommandContext<'a>) -> Result<()> + Send + Sync,
{
    fn call<'a>(
        &'a self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        Box::pin(self(ctx))
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandContext, Commands, Result};

    async fn command(_: CommandContext<'_>) -> Result<()> {
        Ok(())
    }

    #[test]
    fn resolves_only_registered_commands_and_borrows_arguments() -> Result<()> {
        let mut commands = Commands::new("!")?;
        commands.register("ping", command)?;
        commands.register("echo", command)?;

        assert_eq!(commands.resolve("!ping").map(|(_, args)| args), Some(""));
        assert_eq!(
            commands
                .resolve("!echo\t こんにちは  世界 ")
                .map(|(_, args)| args),
            Some("こんにちは  世界")
        );
        for input in ["ping", "!", "! ping", "!pinged", "!PING", "/ping"] {
            assert!(commands.resolve(input).is_none(), "{input}");
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
}
