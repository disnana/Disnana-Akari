use std::{future::Future, panic::AssertUnwindSafe, pin::Pin};

use futures_util::FutureExt as _;

use super::{SlashContext, SlashHandler};
use crate::{
    Error, Result,
    error::{handler_error, panic_error},
};

pub(super) type HookFuture<'a> = Pin<Box<dyn Future<Output = Result<()>> + 'a>>;

trait BeforeInvoke: Send + Sync {
    fn call<'a>(&'a self, ctx: &'a SlashContext<'_>) -> HookFuture<'a>;
}

impl<F> BeforeInvoke for F
where
    F: AsyncFn(&SlashContext<'_>) -> Result<()> + Send + Sync,
{
    fn call<'a>(&'a self, ctx: &'a SlashContext<'_>) -> HookFuture<'a> {
        Box::pin(catch_result(self(ctx)))
    }
}

trait AfterInvoke: Send + Sync {
    fn call<'a>(&'a self, ctx: &'a SlashContext<'_>, error: Option<&'a Error>) -> HookFuture<'a>;
}

impl<F> AfterInvoke for F
where
    F: AsyncFn(&SlashContext<'_>, Option<&Error>) -> Result<()> + Send + Sync,
{
    fn call<'a>(&'a self, ctx: &'a SlashContext<'_>, error: Option<&'a Error>) -> HookFuture<'a> {
        Box::pin(catch_result(self(ctx, error)))
    }
}

#[derive(Default)]
pub(crate) struct InvocationHooks {
    before: Option<Box<dyn BeforeInvoke>>,
    after: Option<Box<dyn AfterInvoke>>,
}

impl InvocationHooks {
    pub(super) fn set_before(
        &mut self,
        hook: impl AsyncFn(&SlashContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) {
        self.before = Some(Box::new(hook));
    }

    pub(super) fn set_after(
        &mut self,
        hook: impl AsyncFn(&SlashContext<'_>, Option<&Error>) -> Result<()> + Send + Sync + 'static,
    ) {
        self.after = Some(Box::new(hook));
    }

    pub(crate) async fn run<'ctx>(
        &self,
        ctx: &mut SlashContext<'ctx>,
        handler: impl AsyncFnOnce(&mut SlashContext<'ctx>) -> Result<()>,
    ) -> Result<()> {
        if let Some(before) = &self.before {
            before.call(ctx).await?;
        }
        let result = catch_result(handler(ctx)).await;
        if let Some(after) = &self.after {
            let after_result = after.call(ctx, result.as_ref().err()).await;
            return result.and(after_result);
        }
        result
    }
}

pub(super) fn slash_handler<E>(
    handler: impl AsyncFn(&mut SlashContext<'_>) -> std::result::Result<(), E> + Send + Sync + 'static,
) -> Box<dyn SlashHandler>
where
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    Box::new(async move |ctx: &mut SlashContext<'_>| handler(ctx).await.map_err(handler_error))
}

// Framework adapters bind the inputs before they call the shared hook runner.
pub(super) struct PreparedSlashHandler<F>(pub(super) F);

impl<F> SlashHandler for PreparedSlashHandler<F>
where
    F: AsyncFn(&mut SlashContext<'_>, &InvocationHooks) -> Result<()> + Send + Sync,
{
    fn call<'a, 'ctx>(
        &'a self,
        ctx: &'a mut SlashContext<'ctx>,
        hooks: &'a InvocationHooks,
    ) -> HookFuture<'a> {
        Box::pin(self.0(ctx, hooks))
    }
}

async fn catch_result(future: impl Future<Output = Result<()>>) -> Result<()> {
    match AssertUnwindSafe(future).catch_unwind().await {
        Ok(result) => result,
        Err(payload) => Err(panic_error(payload)),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use twilight_http::Client;
    use twilight_model::{
        application::{
            command::CommandType,
            interaction::{
                Interaction, InteractionData,
                application_command::{CommandDataOption, CommandOptionValue},
            },
        },
        id::Id,
    };

    use super::*;
    use crate::{
        CheckFailure, CommandCheck, CommandCheckContext, Commands, HybridArgumentError,
        HybridArguments, HybridContext, SlashCommands, UserMenuTarget,
        hooks::{Hooks, Reporter},
        register_typed_hybrid,
        slash::{ActiveSlashCommands, tests::command_interaction},
        timing::Pending,
    };

    async fn dispatch(commands: &ActiveSlashCommands, input: &Interaction, hooks: &Hooks) {
        let http = Client::builder().proxy("127.0.0.1:1".into(), true).build();
        let (_, timing) = Pending::new((), None).begin();
        commands.dispatch(&http, input, timing, hooks).await;
    }

    fn quiet_hooks() -> Hooks {
        let mut hooks = Hooks::default();
        hooks.messages.slash_command_failed = None;
        hooks.messages.slash_command_unavailable = None;
        hooks.messages.check_failed = None;
        hooks
    }

    fn chat_input(
        id: u64,
        name: &str,
        parameter: Option<(&str, CommandOptionValue)>,
    ) -> Result<Interaction> {
        let mut input = command_interaction(None, None, id);
        let Some(InteractionData::ApplicationCommand(data)) = &mut input.data else {
            return Err(Error::msg(
                "試験用の Interaction にコマンドデータがありません",
            ));
        };
        data.name = name.into();
        if let Some((name, value)) = parameter {
            data.options.push(CommandDataOption {
                name: name.into(),
                value,
            });
        }
        Ok(input)
    }

    #[tokio::test]
    async fn hooks_observe_checks_and_bound_arguments_in_order() -> Result<()> {
        let mut slash = SlashCommands::global();
        let mut text = Commands::new("!")?;
        let trace = Arc::new(Mutex::new(Vec::new()));
        let check = Arc::clone(&trace);
        slash.add_check(CommandCheck::new(
            async move |ctx: CommandCheckContext<'_>| {
                if ctx.command_path().eq(["denied"]) {
                    return Err(CheckFailure::Rejected.into());
                }
                check.lock().unwrap().push("check");
                Ok(())
            },
        ));
        let before = Arc::clone(&trace);
        slash.set_before_invoke(async move |ctx: &SlashContext<'_>| {
            assert_eq!(ctx.command_path(), ["sum"]);
            tokio::task::yield_now().await;
            before.lock().unwrap().push("before");
            Ok(())
        });
        let after = Arc::clone(&trace);
        slash.set_after_invoke(async move |_: &SlashContext<'_>, error: Option<&Error>| {
            assert!(error.is_none());
            after.lock().unwrap().push("after");
            Ok(())
        });
        let body = Arc::clone(&trace);
        register_typed_hybrid(
            &mut text,
            Some(&mut slash),
            "sum",
            "計算",
            vec![SlashCommands::integer_option("n", "個数", true)],
            async move |_: &mut HybridContext<'_>, args: HybridArguments<'_>| -> Result<()> {
                assert_eq!(args.integer("n"), Some(2));
                body.lock().unwrap().push("body");
                Ok(())
            },
        )?;
        slash.register("denied", "拒否", vec![], async |_| -> Result<()> {
            panic!("denied callback ran")
        })?;
        let mut models = slash.replacement_commands()?;
        for command in &mut models {
            command.id = Some(Id::new(if command.name == "sum" { 5 } else { 6 }));
        }
        let commands = slash.bind(models)?;
        let invalid = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&invalid);
        let mut hooks = quiet_hooks();
        hooks.reporter = Reporter::new(move |report| {
            observed.lock().unwrap().push(matches!(
                report.error,
                Error::HybridArgument(HybridArgumentError::Missing { .. })
                    | Error::Check(CheckFailure::Rejected)
            ));
        });

        dispatch(&commands, &chat_input(6, "denied", None)?, &hooks).await;
        dispatch(&commands, &chat_input(5, "sum", None)?, &hooks).await;
        assert_eq!(*trace.lock().unwrap(), ["check"]);
        assert_eq!(*invalid.lock().unwrap(), [true, true]);
        trace.lock().unwrap().clear();
        dispatch(
            &commands,
            &chat_input(5, "sum", Some(("n", CommandOptionValue::Integer(2))))?,
            &hooks,
        )
        .await;
        assert_eq!(*trace.lock().unwrap(), ["check", "before", "body", "after"]);
        Ok(())
    }

    #[tokio::test]
    async fn menu_target_validation_precedes_hooks_and_callback_panic_reaches_after() -> Result<()>
    {
        let mut slash = SlashCommands::global();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let before = Arc::clone(&seen);
        slash.set_before_invoke(async move |ctx: &SlashContext<'_>| {
            assert_eq!(ctx.command_path(), ["Inspect"]);
            before.lock().unwrap().push("before");
            Ok(())
        });
        let after = Arc::clone(&seen);
        slash.set_after_invoke(async move |_: &SlashContext<'_>, error: Option<&Error>| {
            assert!(matches!(error, Some(Error::HandlerPanic { .. })));
            after.lock().unwrap().push("after");
            Err(Error::msg("after failed"))
        });
        let body = Arc::clone(&seen);
        slash.register_user_menu(
            "Inspect",
            async move |_: &mut SlashContext<'_>, user: UserMenuTarget<'_>| -> Result<()> {
                assert_eq!(user.user.id, Id::new(71));
                body.lock().unwrap().push("body");
                panic!("body panic")
            },
        )?;
        let mut models = slash.replacement_commands()?;
        models[0].id = Some(Id::new(5));
        let commands = slash.bind(models)?;
        let errors = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&errors);
        let mut hooks = quiet_hooks();
        hooks.reporter = Reporter::new(move |report| {
            observed.lock().unwrap().push(report.error.to_string());
        });
        let mut input = chat_input(5, "Inspect", None)?;
        let Some(InteractionData::ApplicationCommand(data)) = &mut input.data else {
            return Err(Error::msg(
                "試験用の Interaction にコマンドデータがありません",
            ));
        };
        data.kind = CommandType::User;
        data.target_id = Some(Id::new(71));
        dispatch(&commands, &input, &hooks).await;
        assert!(seen.lock().unwrap().is_empty());

        let Some(InteractionData::ApplicationCommand(data)) = &mut input.data else {
            return Err(Error::msg(
                "試験用の Interaction にコマンドデータがありません",
            ));
        };
        data.resolved = Some(
            serde_json::from_value(serde_json::json!({
                "users": {"71": {"id":"71", "username":"member", "discriminator":"0"}}
            }))
            .map_err(Error::handler)?,
        );
        dispatch(&commands, &input, &hooks).await;
        assert_eq!(*seen.lock().unwrap(), ["before", "body", "after"]);
        let errors = errors.lock().unwrap();
        assert_eq!(errors[0], Error::CommandTargetUnavailable.to_string());
        assert!(errors[1].contains("body panic"));
        Ok(())
    }
}
