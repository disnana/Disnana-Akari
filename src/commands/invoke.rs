use std::{future::Future, panic::AssertUnwindSafe, pin::Pin};

use futures_util::FutureExt as _;

use super::{CommandContext, CommandHandler, PreparedTextHandler};
use crate::{
    Error, Result,
    error::{handler_error, panic_error},
};

type HookFuture<'a> = Pin<Box<dyn Future<Output = Result<()>> + 'a>>;

pub(super) trait BeforeInvoke: Send + Sync {
    fn call<'a>(&'a self, ctx: &'a CommandContext<'_>) -> HookFuture<'a>;
}

impl<F> BeforeInvoke for F
where
    F: AsyncFn(&CommandContext<'_>) -> Result<()> + Send + Sync,
{
    fn call<'a>(&'a self, ctx: &'a CommandContext<'_>) -> HookFuture<'a> {
        Box::pin(catch_result(self(ctx)))
    }
}

pub(super) trait AfterInvoke: Send + Sync {
    fn call<'a>(&'a self, ctx: &'a CommandContext<'_>, error: Option<&'a Error>) -> HookFuture<'a>;
}

impl<F> AfterInvoke for F
where
    F: AsyncFn(&CommandContext<'_>, Option<&Error>) -> Result<()> + Send + Sync,
{
    fn call<'a>(&'a self, ctx: &'a CommandContext<'_>, error: Option<&'a Error>) -> HookFuture<'a> {
        Box::pin(catch_result(self(ctx, error)))
    }
}

#[derive(Default)]
pub(super) struct InvocationHooks {
    pub(super) before: Option<Box<dyn BeforeInvoke>>,
    pub(super) after: Option<Box<dyn AfterInvoke>>,
}

impl<'a> CommandContext<'a> {
    pub(crate) async fn invoke_prepared(
        self,
        handler: impl AsyncFnOnce(Self) -> Result<()>,
    ) -> Result<()> {
        let hooks = &self.commands.invocation_hooks;
        if let Some(before) = &hooks.before {
            before.call(&self).await?;
        }
        let after_context = hooks.after.as_ref().map(|_| Self { ..self });
        let result = catch_result(handler(self)).await;
        if let (Some(after), Some(ctx)) = (&hooks.after, after_context) {
            let after_result = after.call(&ctx, result.as_ref().err()).await;
            return result.and(after_result);
        }
        result
    }
}

pub(super) fn parsed_handler<T, P, E>(
    parse: impl AsyncFn(&CommandContext<'_>) -> std::result::Result<T, P> + Send + Sync + 'static,
    handler: impl for<'a> AsyncFn(CommandContext<'a>, T) -> std::result::Result<(), E>
    + Send
    + Sync
    + 'static,
) -> Box<dyn CommandHandler>
where
    T: 'static,
    P: Into<Box<dyn std::error::Error + Send + Sync>>,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    Box::new(PreparedTextHandler(async move |ctx: CommandContext<'_>| {
        let parsed = catch_result(async { parse(&ctx).await.map_err(handler_error) }).await?;
        let handler = &handler;
        ctx.invoke_prepared(async move |ctx| handler(ctx, parsed).await.map_err(handler_error))
            .await
    }))
}

async fn catch_result<T>(future: impl Future<Output = Result<T>>) -> Result<T> {
    match AssertUnwindSafe(future).catch_unwind().await {
        Ok(result) => result,
        Err(payload) => Err(panic_error(payload)),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use twilight_http::Client;

    use super::*;
    use crate::{
        CheckFailure, CommandCheck, CommandCheckContext, Commands, Context, HybridArgumentError,
        HybridArguments, HybridContext, SlashCommands, commands::tests::command_message,
        permissions::PermissionResolver, register_hybrid_group_with_aliases, register_typed_hybrid,
        timing::Pending,
    };

    async fn dispatch(commands: &Commands, content: &str) -> Result<()> {
        let http = Client::new("test".into());
        let permissions = PermissionResolver::new(None);
        let message = command_message(content)?;
        let (_, timing) = Pending::new((), None).begin();
        commands
            .dispatch(Context {
                http: &http,
                message: &message,
                timing,
                permissions: &permissions,
            })
            .await
    }

    #[tokio::test]
    async fn checks_precede_hooks_and_aliases_use_the_canonical_command() -> Result<()> {
        let mut commands = Commands::new("!")?;
        let trace = Arc::new(Mutex::new(Vec::new()));
        let before = Arc::clone(&trace);
        commands.set_before_invoke(async move |ctx: &CommandContext<'_>| {
            assert_eq!(ctx.qualified_name(), "config set");
            assert_eq!(ctx.invoked_with(), "s");
            tokio::task::yield_now().await;
            before.lock().unwrap().push("before");
            Ok(())
        });
        let after = Arc::clone(&trace);
        commands.set_after_invoke(async move |ctx: &CommandContext<'_>, err: Option<&Error>| {
            assert_eq!(ctx.qualified_name(), "config set");
            assert!(err.is_none());
            after.lock().unwrap().push("after");
            Ok(())
        });
        let checks = Arc::clone(&trace);
        commands.add_check(CommandCheck::new(
            async move |ctx: CommandCheckContext<'_>| {
                if ctx.command_path().eq(["denied"]) {
                    return Err(CheckFailure::Rejected.into());
                }
                checks.lock().unwrap().push("check");
                Ok(())
            },
        ));
        let body = Arc::clone(&trace);
        commands
            .group_with_aliases("config", &["cfg"])?
            .subcommand_with_aliases(
                "set",
                &["s"],
                async move |_: CommandContext<'_>| -> Result<()> {
                    body.lock().unwrap().push("body");
                    Ok(())
                },
            )?;
        commands.register("denied", async |_| -> Result<()> {
            panic!("denied command ran")
        })?;
        dispatch(&commands, "!cfg missing").await?;
        assert!(matches!(
            dispatch(&commands, "!denied").await,
            Err(Error::Check(CheckFailure::Rejected))
        ));
        assert!(trace.lock().unwrap().is_empty());
        dispatch(&commands, "!cfg s 42").await?;
        assert_eq!(*trace.lock().unwrap(), ["check", "before", "body", "after"]);
        Ok(())
    }

    #[tokio::test]
    async fn failures_and_panics_keep_the_original_outcome_and_run_after_when_started() -> Result<()>
    {
        let mut commands = Commands::new("!")?;
        commands.set_before_invoke(async |ctx: &CommandContext<'_>| match ctx.args() {
            "before_error" => Err(Error::msg("before failure")),
            "before_panic" => panic!("before panic"),
            _ => Ok(()),
        });
        let seen = Arc::new(Mutex::new(Vec::new()));
        let after = Arc::clone(&seen);
        commands.set_after_invoke(
            async move |ctx: &CommandContext<'_>, failure: Option<&Error>| {
                after
                    .lock()
                    .unwrap()
                    .push((ctx.args().to_owned(), failure.is_some()));
                if ctx.args() == "after_panic" {
                    panic!("after panic");
                }
                Err(Error::msg("after failure"))
            },
        );
        commands.register("test", async |ctx: CommandContext<'_>| -> Result<()> {
            match ctx.args() {
                "body_error" => Err(Error::msg("body failure")),
                "body_panic" => panic!("body panic"),
                _ => Ok(()),
            }
        })?;
        for (input, expected) in [
            ("before_error", "before failure"),
            ("before_panic", "before panic"),
            ("body_error", "body failure"),
            ("body_panic", "body panic"),
            ("after_error", "after failure"),
            ("after_panic", "after panic"),
        ] {
            let error = dispatch(&commands, &format!("!test {input}"))
                .await
                .unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
        assert_eq!(
            *seen.lock().unwrap(),
            [
                ("body_error".into(), true),
                ("body_panic".into(), true),
                ("after_error".into(), false),
                ("after_panic".into(), false),
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn parsed_text_checks_and_converts_before_running_hooks() -> Result<()> {
        let mut commands = Commands::new("!")?;
        let trace = Arc::new(Mutex::new(Vec::new()));
        let checks = Arc::clone(&trace);
        commands.add_check(CommandCheck::new(
            async move |ctx: CommandCheckContext<'_>| {
                if ctx.command_path().eq(["stop"]) {
                    return Err(CheckFailure::Rejected.into());
                }
                checks.lock().unwrap().push("check");
                Ok(())
            },
        ));
        let before = Arc::clone(&trace);
        commands.set_before_invoke(async move |ctx: &CommandContext<'_>| {
            assert_eq!(ctx.qualified_name(), "計算 倍");
            assert_eq!(ctx.invoked_with(), "x");
            before.lock().unwrap().push("before");
            Ok(())
        });
        let after = Arc::clone(&trace);
        commands.set_after_invoke(async move |_: &CommandContext<'_>, error: Option<&Error>| {
            after.lock().unwrap().push(if error.is_some() {
                "after error"
            } else {
                "after success"
            });
            Ok(())
        });
        let parse = Arc::clone(&trace);
        let body = Arc::clone(&trace);
        commands
            .group_with_aliases("計算", &["calc"])?
            .subcommand_parsed_with_aliases(
                "倍",
                &["x"],
                async move |ctx: &CommandContext<'_>| {
                    tokio::task::yield_now().await;
                    parse.lock().unwrap().push("parse");
                    ctx.arguments().parse_required::<u32>()
                },
                async move |_: CommandContext<'_>, value| -> Result<()> {
                    body.lock().unwrap().push("body");
                    match value {
                        42 => Ok(()),
                        43 => Err(Error::msg("callback failure")),
                        _ => panic!("unexpected converted value"),
                    }
                },
            )?;
        commands.register_parsed(
            "stop",
            async |_: &CommandContext<'_>| -> Result<()> { panic!("denied parser ran") },
            async |_: CommandContext<'_>, ()| -> Result<()> { panic!("denied callback ran") },
        )?;

        assert!(matches!(
            dispatch(&commands, "!stop").await,
            Err(Error::Check(CheckFailure::Rejected))
        ));
        assert!(trace.lock().unwrap().is_empty());
        assert!(matches!(
            dispatch(&commands, "!calc x nope").await,
            Err(Error::Handler(_))
        ));
        assert_eq!(*trace.lock().unwrap(), ["check", "parse"]);
        trace.lock().unwrap().clear();
        dispatch(&commands, "!calc x 「42」").await?;
        assert_eq!(
            *trace.lock().unwrap(),
            ["check", "parse", "before", "body", "after success"]
        );
        trace.lock().unwrap().clear();
        assert!(matches!(
            dispatch(&commands, "!calc x 43").await,
            Err(Error::Handler(_))
        ));
        assert_eq!(
            *trace.lock().unwrap(),
            ["check", "parse", "before", "body", "after error"]
        );
        Ok(())
    }

    #[test]
    fn parsed_text_does_not_register_any_name_when_an_alias_conflicts() -> Result<()> {
        let mut commands = Commands::new("!")?;
        let parse = async |ctx: &CommandContext<'_>| ctx.arguments().parse_required::<u32>();
        let handler = async |_: CommandContext<'_>, _: u32| -> Result<()> { Ok(()) };
        commands.register_parsed_with_aliases("first", &["n"], parse, handler)?;
        let conflict =
            commands.register_parsed_with_aliases("second", &["other", "n"], parse, handler);
        assert!(conflict.is_err());
        assert_eq!(commands.command_info("n").unwrap().name(), "first");
        assert!(commands.command_info("second").is_none());
        assert!(commands.command_info("other").is_none());
        Ok(())
    }

    #[tokio::test]
    async fn typed_hybrid_root_group_and_fallback_run_hooks_only_after_binding() -> Result<()> {
        let mut commands = Commands::new("!")?;
        let trace = Arc::new(Mutex::new(Vec::new()));
        let before = Arc::clone(&trace);
        commands.set_before_invoke(async move |ctx: &CommandContext<'_>| {
            before
                .lock()
                .unwrap()
                .push(format!("before {}", ctx.qualified_name()));
            Ok(())
        });
        let after = Arc::clone(&trace);
        commands.set_after_invoke(
            async move |ctx: &CommandContext<'_>, failure: Option<&Error>| {
                assert!(failure.is_none());
                after
                    .lock()
                    .unwrap()
                    .push(format!("after {}", ctx.qualified_name()));
                Ok(())
            },
        );
        let count = || SlashCommands::integer_option("count", "件数", true);
        let callback =
            async |_: &mut HybridContext<'_>, _: HybridArguments<'_>| -> Result<()> { Ok(()) };
        register_typed_hybrid(
            &mut commands,
            None,
            "sum",
            "足し算",
            vec![count()],
            callback,
        )?;
        register_hybrid_group_with_aliases(
            &mut commands,
            None,
            "files",
            "ファイル",
            &["f"],
            |group| {
                group.subcommand(
                    "add",
                    "追加",
                    vec![
                        SlashCommands::attachment_option("file", "添付", true),
                        count(),
                    ],
                    callback,
                )?;
                group.subcommand_with_aliases("count", "件数", &["n"], vec![count()], callback)?;
                group.fallback("show", "表示", callback)
            },
        )?;
        assert!(matches!(
            dispatch(&commands, "!sum invalid").await,
            Err(Error::HybridArgument(
                HybridArgumentError::InvalidValue { .. }
            ))
        ));
        assert!(
            matches!(dispatch(&commands, "!f add 2").await, Err(Error::HybridArgument(HybridArgumentError::Missing { name })) if name == "file")
        );
        assert!(trace.lock().unwrap().is_empty());
        for input in ["!sum 2", "!f n 2", "!f"] {
            dispatch(&commands, input).await?;
        }
        assert_eq!(
            *trace.lock().unwrap(),
            [
                "before sum",
                "after sum",
                "before files count",
                "after files count",
                "before files",
                "after files",
            ]
        );
        Ok(())
    }
}
