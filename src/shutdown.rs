use std::{future::Future, pin::Pin, time::Duration};

use tokio::{sync::watch, time::timeout};
use twilight_gateway::{CloseFrame, Message, Shard, ShardState};

use crate::{Error, Result};

const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// 実行中の Bot に正常終了を要求します。[`Bot::with_shutdown`](crate::Bot::with_shutdown) で作れます。
#[derive(Clone, Debug)]
pub struct ShutdownHandle(watch::Sender<bool>);

impl ShutdownHandle {
    pub(crate) fn new() -> Self {
        Self(watch::Sender::new(false))
    }

    /// 正常終了を要求します。繰り返し呼んでも、終了の要求は一度だけ送られます。
    ///
    /// このメソッドは停止の完了を待ちません。Bot の起動メソッドの終了を待ってください。
    pub fn request(&self) {
        self.0.send_if_modified(|requested| {
            let changed = !*requested;
            *requested = true;
            changed
        });
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<bool> {
        self.0.subscribe()
    }
}

pub(crate) async fn wait_for_request(request: &mut Option<watch::Receiver<bool>>) {
    if let Some(request) = request
        && request.wait_for(|requested| *requested).await.is_ok()
    {
        return;
    }
    std::future::pending().await
}

pub(crate) async fn drive(
    receive: impl Future<Output = Error>,
    workers: Pin<&mut impl Future<Output = ()>>,
    request: &mut Option<watch::Receiver<bool>>,
) -> Result<()> {
    let runtime = async {
        tokio::select! {
            error = receive => Err(error),
            () = workers => Err(Error::GatewayEnded),
        }
    };
    tokio::select! {
        biased;
        () = wait_for_request(request) => Ok(()),
        result = runtime => result,
    }
}

pub(crate) async fn close_shard(shard: &mut Shard) {
    // 切断済みのシャードを poll すると、新しい Gateway 接続が始まる。
    if !matches!(
        shard.state(),
        ShardState::Active | ShardState::Identifying | ShardState::Resuming
    ) {
        return;
    }

    shard.close(CloseFrame::NORMAL);
    while let Some(item) = futures_util::StreamExt::next(shard).await {
        if !matches!(item, Ok(Message::Text(_))) {
            break;
        }
    }
}

pub(crate) async fn finish(close: impl Future<Output = ()>, drain: impl Future<Output = ()>) {
    // どちらかが遅くても、もう一方の終了処理は並行して進める。
    let _ = tokio::join!(timeout(CLOSE_TIMEOUT, close), timeout(DRAIN_TIMEOUT, drain));
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, future::pending};

    use tokio::{
        sync::{Notify, mpsc},
        time::{Instant, sleep},
    };

    use super::*;
    use crate::{Bot, Commands, Limits};

    #[tokio::test]
    async fn request_before_start_is_shared_and_skips_preparation() {
        let bot = Bot::new("test-token")
            .unwrap()
            .limits(Limits::default().queue_capacity(0));
        let (bot, first) = bot.with_shutdown();
        let (bot, second) = bot.with_shutdown();
        let mut requests = second.subscribe();

        first.request();
        requests.changed().await.unwrap();
        assert!(*requests.borrow_and_update());
        first.request();
        assert!(!requests.has_changed().unwrap());
        drop((first, second));

        // 準備処理に入ると Intent と Limits の検査で失敗するため、回帰時も通信しない。
        assert!(bot.run_commands(Commands::new("!").unwrap()).await.is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn an_unrequested_end_stays_distinct_from_a_requested_stop() {
        let handle = ShutdownHandle::new();
        let mut request = Some(handle.subscribe());
        drop(handle);
        let gateway_ended = async {
            sleep(Duration::from_secs(1)).await;
            Error::GatewayEnded
        };
        let mut workers = std::pin::pin!(pending());
        let start = Instant::now();
        assert!(matches!(
            drive(gateway_ended, workers.as_mut(), &mut request).await,
            Err(Error::GatewayEnded)
        ));
        assert_eq!(start.elapsed(), Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn waits_for_admitted_work_while_gateway_close_is_pending() {
        let handle = ShutdownHandle::new();
        let mut request = Some(handle.subscribe());
        let running = Notify::new();
        let completed = Cell::new(false);
        let (tx, mut rx) = mpsc::channel(1);
        tx.try_send(42).unwrap();
        let receive = async move {
            let _admission = tx;
            pending::<Error>().await
        };
        let workers = async {
            assert_eq!(rx.recv().await, Some(42));
            running.notify_one();
            sleep(Duration::from_millis(100)).await;
            assert_eq!(rx.recv().await, None);
            completed.set(true);
        };
        let trigger = async {
            running.notified().await;
            handle.request();
        };
        let runtime = async {
            let mut workers = std::pin::pin!(workers);
            assert!(drive(receive, workers.as_mut(), &mut request).await.is_ok());
            let start = Instant::now();
            finish(pending(), workers).await;
            assert_eq!(start.elapsed(), CLOSE_TIMEOUT);
        };
        tokio::join!(trigger, runtime);
        assert!(completed.get());
    }

    #[tokio::test(start_paused = true)]
    async fn close_and_drain_have_independent_deadlines() {
        struct RecordDrop<'a>(&'a Cell<Option<Instant>>);
        impl Drop for RecordDrop<'_> {
            fn drop(&mut self) {
                self.0.set(Some(Instant::now()));
            }
        }

        let close_end = Cell::new(None);
        let drain_end = Cell::new(None);
        let start = Instant::now();
        finish(
            async {
                let _close = RecordDrop(&close_end);
                pending().await
            },
            async {
                let _drain = RecordDrop(&drain_end);
                pending().await
            },
        )
        .await;
        assert_eq!(close_end.get(), Some(start + CLOSE_TIMEOUT));
        assert_eq!(drain_end.get(), Some(start + DRAIN_TIMEOUT));
    }
}
