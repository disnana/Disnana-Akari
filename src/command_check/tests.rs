use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use twilight_model::application::command::CommandType;

use super::*;
use crate::{
    Error,
    access::tests::{offline_http, slash_interaction},
    permissions::PermissionResolver,
};

fn record_check(trace: &Arc<Mutex<Vec<&'static str>>>, name: &'static str) -> CommandCheck {
    let trace = Arc::clone(trace);
    CommandCheck::new(async move |ctx: CommandCheckContext<'_>| {
        assert_eq!(ctx.command_path().collect::<Vec<_>>(), ["system", "status"]);
        trace.lock().unwrap().push(name);
        Ok(())
    })
}

#[tokio::test]
async fn bot_checks_run_first_for_invocation_but_not_for_a_visibility_probe() -> Result<()> {
    let http = offline_http();
    let permissions = PermissionResolver::new(None);
    let interaction = slash_interaction(true, 0, 0);
    let access = Access::for_interaction(&http, &permissions, &interaction);
    let ctx = CommandCheckContext::application(
        &interaction,
        access,
        CommandCheckSource::Application(CommandType::ChatInput),
        "system",
        &["status"],
    );
    let trace = Arc::new(Mutex::new(Vec::new()));
    let once = [
        record_check(&trace, "once first"),
        record_check(&trace, "once second"),
    ];
    let checks = [record_check(&trace, "collection")];

    run_invocation_checks(&once, &checks, ctx).await?;
    assert_eq!(
        *trace.lock().unwrap(),
        ["once first", "once second", "collection"]
    );
    trace.lock().unwrap().clear();
    run_checks(&checks, ctx).await?;
    assert_eq!(*trace.lock().unwrap(), ["collection"]);
    Ok(())
}

#[tokio::test]
async fn bot_check_rejection_stops_both_layers() {
    let http = offline_http();
    let permissions = PermissionResolver::new(None);
    let interaction = slash_interaction(true, 0, 0);
    let access = Access::for_interaction(&http, &permissions, &interaction);
    let ctx = CommandCheckContext::application(
        &interaction,
        access,
        CommandCheckSource::Application(CommandType::ChatInput),
        "system",
        &[],
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let trace = Arc::clone(&calls);
    let skipped = CommandCheck::new(async move |_| {
        trace.fetch_add(1, Ordering::Relaxed);
        Ok(())
    });
    let once = [
        CommandCheck::new(async |_| Err(CheckFailure::Rejected.into())),
        skipped.clone(),
    ];
    assert!(matches!(
        run_invocation_checks(&once, &[skipped], ctx).await,
        Err(Error::Check(CheckFailure::Rejected))
    ));
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}
