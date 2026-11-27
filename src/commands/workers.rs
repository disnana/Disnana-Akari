use std::{future::Future, sync::Arc};

use tokio::sync::mpsc;
use twilight_http::Client;
use twilight_model::channel::Message;

use super::{Commands, PrefixPolicy};
use crate::{
    Context, Error, ErrorSource, Limits, QueueKind, hooks::Hooks, permissions::PermissionResolver,
    timing::Pending, work_queue,
};

type Candidate = Pending<Arc<Message>>;

struct Matched {
    pending: Candidate,
    prefix_len: usize,
}

pub(crate) enum TextRoute {
    Fixed(usize),
    Discover,
}

pub(crate) struct TextIngress<'a> {
    commands: Option<&'a Commands>,
    matched: mpsc::Sender<Matched>,
    discovery: Option<mpsc::Sender<Candidate>>,
    busy: mpsc::Sender<Candidate>,
}

impl TextIngress<'_> {
    pub(crate) fn classify(&self, content: &str) -> Option<TextRoute> {
        let commands = self.commands?;
        if self.discovery.is_some() {
            Some(TextRoute::Discover)
        } else {
            commands
                .matching_static_prefix(content)
                .map(TextRoute::Fixed)
        }
    }

    pub(crate) fn submit(&self, route: TextRoute, pending: Candidate, hooks: &Hooks) {
        match route {
            TextRoute::Fixed(prefix_len) => submit_match(
                &self.matched,
                &self.busy,
                Matched {
                    pending,
                    prefix_len,
                },
                hooks,
            ),
            TextRoute::Discover => {
                if let Some(queue) = &self.discovery
                    && queue.try_send(pending).is_err()
                {
                    hooks.reporter.report(
                        ErrorSource::QueueFull,
                        None,
                        &Error::QueueFull(QueueKind::PrefixDiscovery),
                    );
                }
            }
        }
    }
}

pub(crate) fn text_workers<'a>(
    commands: Option<&'a Commands>,
    limits: Limits,
    http: &'a Client,
    permissions: &'a PermissionResolver,
    hooks: &'a Hooks,
) -> (TextIngress<'a>, impl Future<Output = ()> + 'a) {
    let (matched, matched_rx) = mpsc::channel(limits.queue_capacity);
    let (busy, busy_rx) = mpsc::channel(limits.queue_capacity);
    let (discovery, discovery_work) = if let Some(commands) = commands
        && matches!(commands.prefixes, PrefixPolicy::Dynamic(_))
    {
        let (tx, rx) = mpsc::channel(limits.queue_capacity);
        (
            Some(tx),
            Some((commands, rx, matched.clone(), busy.clone())),
        )
    } else {
        (None, None)
    };

    let resolve = async move {
        if let Some((commands, rx, matched, busy)) = discovery_work {
            let (matched, busy) = (&matched, &busy);
            work_queue::respond(
                rx,
                limits.text_workers,
                move |pending: Candidate| async move {
                    match commands.matching_prefix(pending.event()).await {
                        Ok(Some(prefix_len)) => submit_match(
                            matched,
                            busy,
                            Matched {
                                pending,
                                prefix_len,
                            },
                            hooks,
                        ),
                        Ok(None) => {}
                        Err(error) => {
                            hooks
                                .reporter
                                .report(ErrorSource::PrefixResolver, None, &error)
                        }
                    }
                },
            )
            .await;
        }
    };
    let busy_overflow = busy.clone();
    let run = work_queue::dispatch(
        matched_rx,
        limits.text_workers,
        move |matched: Matched| async move {
            let (message, timing) = matched.pending.begin();
            if let Some(commands) = commands {
                commands
                    .dispatch_with_feedback(
                        Context {
                            http,
                            message: &message,
                            timing,
                            permissions,
                        },
                        matched.prefix_len,
                        hooks,
                    )
                    .await;
            }
        },
        move |matched| queue_text_busy(&busy_overflow, matched.pending, hooks),
    );
    let notices = work_queue::respond(
        busy_rx,
        limits.notice_workers,
        move |pending: Candidate| async move {
            let (message, timing) = pending.begin();
            let context = Context {
                http,
                message: &message,
                timing,
                permissions,
            };
            if let Some(text) = &hooks.messages.text_command_busy
                && let Err(error) = context.reply(text).await
            {
                hooks
                    .reporter
                    .report(ErrorSource::Response, Some("混雑の通知"), &error);
            }
        },
    );
    let workers = async move {
        tokio::join!(resolve, run, notices);
    };
    (
        TextIngress {
            commands,
            matched,
            discovery,
            busy,
        },
        workers,
    )
}

fn submit_match(
    queue: &mpsc::Sender<Matched>,
    busy: &mpsc::Sender<Candidate>,
    matched: Matched,
    hooks: &Hooks,
) {
    if let Err(error) = queue.try_send(matched) {
        queue_text_busy(busy, error.into_inner().pending, hooks);
    }
}

fn queue_text_busy(queue: &mpsc::Sender<Candidate>, pending: Candidate, hooks: &Hooks) {
    hooks.reporter.report(
        ErrorSource::QueueFull,
        Some("テキストコマンド"),
        &Error::QueueFull(QueueKind::TextCommand),
    );
    if hooks.messages.text_command_busy.is_some() && queue.try_send(pending).is_err() {
        hooks.reporter.report(
            ErrorSource::QueueFull,
            Some("混雑の通知"),
            &Error::QueueFull(QueueKind::BusyNotice),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use tokio::sync::Notify;
    use twilight_model::id::Id;

    use super::*;
    use crate::{CommandContext, CommandPrefixes, Result, hooks::Reporter};

    fn submit(ingress: &TextIngress<'_>, hooks: &Hooks, id: u64, content: &str) {
        let mut message: Message = serde_json::from_str(
            r#"{
            "id": "1", "channel_id": "2", "type": 0,
            "author": {"id": "3", "username": "member", "discriminator": "0"},
            "content": "", "timestamp": "2026-01-01T00:00:00.000000+00:00",
            "attachments": [], "embeds": [], "mention_everyone": false, "mention_roles": [],
            "mentions": [], "pinned": false, "tts": false
        }"#,
        )
        .unwrap();
        message.id = Id::new(id);
        message.content = content.into();
        let route = ingress.classify(content).unwrap();
        ingress.submit(
            route,
            Pending::new(Arc::new(message), Some(Duration::from_millis(4))),
            hooks,
        );
    }

    fn no_discord() -> Client {
        Client::builder().proxy("127.0.0.1:1".into(), true).build()
    }

    #[tokio::test]
    async fn dynamic_prefix_is_evaluated_once_and_drain_ignores_unresolved_overflow() -> Result<()>
    {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let resolutions = Arc::new(Mutex::new(Vec::new()));
        let resolver_started = Arc::clone(&started);
        let resolver_release = Arc::clone(&release);
        let resolved = Arc::clone(&resolutions);
        let mut commands = Commands::with_prefix_resolver(
            async move |message: &Message| -> Result<CommandPrefixes> {
                resolved.lock().unwrap().push(message.id.get());
                if message.id.get() == 1 {
                    resolver_started.notify_one();
                    resolver_release.notified().await;
                    return Ok(CommandPrefixes::default());
                }
                CommandPrefixes::new(&["☀", "☀!"])
            },
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let handler = Arc::clone(&seen);
        commands.register("ping", async move |ctx: CommandContext<'_>| -> Result<()> {
            assert_eq!(ctx.timing().gateway_rtt(), Some(Duration::from_millis(4)));
            handler
                .lock()
                .unwrap()
                .push((ctx.prefix().to_owned(), ctx.args().to_owned()));
            Ok(())
        })?;
        // The shorter prefix also has a matching command; dynamic and static both choose the longest.
        commands.register("!ping", async |_| -> Result<()> {
            panic!("shorter prefix must not win")
        })?;

        let reports = Arc::new(Mutex::new(Vec::new()));
        let reported = Arc::clone(&reports);
        let mut hooks = Hooks {
            reporter: Reporter::new(move |report| {
                assert_eq!(report.source, ErrorSource::QueueFull);
                assert!(matches!(
                    report.error,
                    Error::QueueFull(QueueKind::PrefixDiscovery)
                ));
                reported.lock().unwrap().push(report.source);
            }),
            ..Hooks::default()
        };
        hooks.messages.text_command_busy = Some("busy".into());
        let http = no_discord();
        let permissions = PermissionResolver::new(None);
        let limits = Limits::default().text_workers(1).queue_capacity(1);
        let (ingress, workers) = text_workers(Some(&commands), limits, &http, &permissions, &hooks);
        submit(&ingress, &hooks, 1, "ordinary chat");
        let arrivals = async {
            started.notified().await;
            submit(&ingress, &hooks, 2, "☀!ping ok");
            submit(&ingress, &hooks, 3, "another ordinary message");
            assert_eq!(*resolutions.lock().unwrap(), [1]);
            assert_eq!(reports.lock().unwrap().len(), 1);
            drop(ingress);
            release.notify_one();
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(workers, arrivals);
        })
        .await
        .unwrap();
        assert_eq!(*resolutions.lock().unwrap(), [1, 2]);
        assert_eq!(*seen.lock().unwrap(), [("☀!".into(), "ok".into())]);
        assert_eq!(reports.lock().unwrap().len(), 1);
        Ok(())
    }

    struct Cancellation(Arc<AtomicBool>);

    impl Drop for Cancellation {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn dropping_workers_cancels_stuck_prefix_resolution() -> Result<()> {
        let started = Arc::new(Notify::new());
        let cancelled = Arc::new(AtomicBool::new(false));
        let resolver_started = Arc::clone(&started);
        let resolver_cancelled = Arc::clone(&cancelled);
        let mut commands =
            Commands::with_prefix_resolver(async move |_: &Message| -> Result<CommandPrefixes> {
                let _cancelled = Cancellation(Arc::clone(&resolver_cancelled));
                resolver_started.notify_one();
                std::future::pending().await
            });
        commands.register("ping", async |_| -> Result<()> {
            panic!("cancelled resolver produced a command")
        })?;
        let http = no_discord();
        let permissions = PermissionResolver::new(None);
        let hooks = Hooks::default();
        let (ingress, workers) = text_workers(
            Some(&commands),
            Limits::default(),
            &http,
            &permissions,
            &hooks,
        );
        submit(&ingress, &hooks, 1, "!ping");
        let mut workers = Box::pin(workers);
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::select! {
                () = &mut workers => panic!("resolver unexpectedly exited"),
                () = started.notified() => {}
            }
        })
        .await
        .unwrap();
        drop(ingress);
        drop(workers);
        assert!(cancelled.load(Ordering::SeqCst));
        Ok(())
    }

    #[tokio::test]
    async fn dynamic_errors_use_the_error_hook_without_replying_to_chat() -> Result<()> {
        let mut commands =
            Commands::with_prefix_resolver(async |_: &Message| -> Result<CommandPrefixes> {
                Err(Error::msg("prefix provider unavailable"))
            });
        commands.register("ping", async |_| -> Result<()> {
            panic!("failed resolver produced a command")
        })?;
        let reports = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&reports);
        let mut hooks = Hooks {
            reporter: Reporter::new(move |report| {
                assert_eq!(report.name, None);
                assert_eq!(report.error.to_string(), "prefix provider unavailable");
                seen.lock().unwrap().push(report.source);
            }),
            ..Hooks::default()
        };
        hooks.messages.text_command_busy = Some("busy".into());
        hooks.messages.text_command_failed = Some("failed".into());
        let http = no_discord();
        let permissions = PermissionResolver::new(None);
        let (ingress, workers) = text_workers(
            Some(&commands),
            Limits::default(),
            &http,
            &permissions,
            &hooks,
        );
        submit(&ingress, &hooks, 1, "ordinary chat");
        drop(ingress);
        tokio::time::timeout(Duration::from_secs(2), workers)
            .await
            .unwrap();
        assert_eq!(*reports.lock().unwrap(), [ErrorSource::PrefixResolver]);
        Ok(())
    }
}
