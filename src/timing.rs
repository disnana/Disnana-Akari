use std::time::{Duration, Instant};

/// コマンドを受信した時点の Gateway RTT と、Bot 内での処理時間です。
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    gateway_rtt: Option<Duration>,
    queue_wait: Duration,
    started_at: Instant,
}

impl Timing {
    /// 最新のハートビート送信から Discord の ACK を受信するまでの時間です。
    /// サンプルを受信する前は `None` を返します。
    pub fn gateway_rtt(&self) -> Option<Duration> {
        self.gateway_rtt
    }

    /// Bot がイベントを受信してから処理を開始するまでの時間です。
    pub fn queue_wait(&self) -> Duration {
        self.queue_wait
    }

    /// 処理開始から呼び出し時点までに経過した時間です。
    pub fn handler_elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }
}

pub(crate) struct Pending<T> {
    event: T,
    received_at: Instant,
    gateway_rtt: Option<Duration>,
}

impl<T> Pending<T> {
    pub(crate) fn new(event: T, gateway_rtt: Option<Duration>) -> Self {
        Self {
            event,
            received_at: Instant::now(),
            gateway_rtt,
        }
    }

    pub(crate) fn begin(self) -> (T, Timing) {
        let started_at = Instant::now();
        let timing = Timing {
            gateway_rtt: self.gateway_rtt,
            queue_wait: started_at.saturating_duration_since(self.received_at),
            started_at,
        };
        (self.event, timing)
    }
}

#[cfg(test)]
mod tests {
    use super::{Duration, Instant, Pending};

    #[test]
    fn keeps_heartbeat_rtt_separate_from_local_wait() {
        let rtt = Duration::from_millis(35);
        let pending = Pending {
            event: "ping",
            received_at: Instant::now() - Duration::from_secs(1),
            gateway_rtt: Some(rtt),
        };
        let (event, timing) = pending.begin();
        assert_eq!(event, "ping");
        assert_eq!(timing.gateway_rtt(), Some(rtt));
        assert!(timing.queue_wait() >= Duration::from_secs(1));
    }

    #[test]
    fn unmeasured_heartbeat_stays_unavailable() {
        let (_, timing) = Pending::new((), None).begin();
        assert_eq!(timing.gateway_rtt(), None);
    }
}
