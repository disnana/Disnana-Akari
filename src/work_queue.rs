use std::future::Future;

use futures_util::{
    StreamExt,
    stream::{self, FuturesUnordered},
};
use tokio::sync::mpsc;

pub(crate) async fn dispatch<T, F>(
    mut queue: mpsc::Receiver<T>,
    capacity: usize,
    handle: impl Fn(T) -> F,
    reject: impl Fn(T),
) where
    F: Future<Output = ()>,
{
    let mut in_flight = FuturesUnordered::new();

    loop {
        tokio::select! {
            // 先に完了した枠を回収し、処理可能な呼び出しを混雑扱いしない。
            biased;
            _ = in_flight.next(), if !in_flight.is_empty() => {}
            event = queue.recv() => match event {
                Some(event) if in_flight.len() < capacity => in_flight.push(handle(event)),
                Some(event) => reject(event),
                None => break,
            },
        }
    }

    while in_flight.next().await.is_some() {}
}

pub(crate) async fn respond<T, F>(queue: mpsc::Receiver<T>, capacity: usize, reply: impl Fn(T) -> F)
where
    F: Future<Output = ()>,
{
    stream::unfold(queue, async |mut queue| {
        queue.recv().await.map(|event| (event, queue))
    })
    .for_each_concurrent(capacity, reply)
    .await;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::{
        sync::{Notify, mpsc},
        time::timeout,
    };

    use super::dispatch;

    #[tokio::test]
    async fn second_handler_progresses_and_excess_is_rejected_before_first_finishes() {
        let (incoming, queue) = mpsc::channel(3);
        for event in 1..=3 {
            incoming.try_send(event).unwrap();
        }
        drop(incoming);

        let finish = Notify::new();
        let (started, mut starts) = mpsc::channel(3);
        let (rejected, mut busy) = mpsc::channel(3);
        let schedule = dispatch(
            queue,
            2,
            |event| {
                let started = &started;
                let finish = &finish;
                async move {
                    started.try_send(event).unwrap();
                    finish.notified().await;
                }
            },
            |event| rejected.try_send(event).unwrap(),
        );
        let check = async {
            assert_eq!(starts.recv().await, Some(1));
            assert_eq!(starts.recv().await, Some(2));
            assert_eq!(busy.recv().await, Some(3));
            finish.notify_waiters();
        };
        timeout(Duration::from_secs(2), async {
            tokio::join!(schedule, check);
        })
        .await
        .unwrap();
    }
}
