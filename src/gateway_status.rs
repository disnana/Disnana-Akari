use std::time::Duration;

use tokio::sync::watch;
use twilight_model::gateway::ShardId;

/// Akari が観測した、一つの Gateway シャードの接続状態です。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GatewayShardState {
    /// 起動順を待っています。
    Queued,
    /// 初回接続を開始し、Ready を待っています。
    Connecting,
    /// Ready または Resumed を受信しました。
    Ready,
    /// 接続が失われ、再接続や再開を待っています。
    Reconnecting,
    /// Bot がシャードを終了しました。
    Closed,
}

/// 一つのシャードの状態と、その接続で受信したハートビートの計測値です。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct GatewayShardStatus {
    /// シャード番号と総数です。
    pub id: ShardId,
    /// 現在の接続状態です。
    pub state: GatewayShardState,
    /// 直近のハートビート往復時間です。まだ計測できていない場合は `None` です。
    pub gateway_rtt: Option<Duration>,
}

/// Bot の Gateway シャードの状態を参照します。
///
/// [`Bot::with_gateway_status`](crate::Bot::with_gateway_status) で受け取ります。
#[derive(Clone, Debug)]
pub struct GatewayStatusHandle(watch::Sender<Vec<GatewayShardStatus>>);

impl GatewayStatusHandle {
    pub(crate) fn new() -> Self {
        Self(watch::Sender::new(Vec::new()))
    }

    /// 現在の一覧をシャード番号順に取得します。シャード数が決まるまでは空です。
    pub fn shards(&self) -> Vec<GatewayShardStatus> {
        self.0.borrow().clone()
    }

    /// 全シャードの最新状態を購読します。複数の変化は最新の一覧にまとまることがあります。
    pub fn subscribe(&self) -> GatewayStatusUpdates {
        GatewayStatusUpdates(self.0.subscribe())
    }

    pub(crate) fn initialize(&self, shards: impl IntoIterator<Item = ShardId>) {
        self.0.send_replace(
            shards
                .into_iter()
                .map(|id| GatewayShardStatus {
                    id,
                    state: GatewayShardState::Queued,
                    gateway_rtt: None,
                })
                .collect(),
        );
    }

    pub(crate) fn transition(&self, id: ShardId, state: GatewayShardState) {
        self.update(id, |status| {
            if status.state == state {
                return false;
            }
            status.state = state;
            status.gateway_rtt = None;
            true
        });
    }

    pub(crate) fn heartbeat(&self, id: ShardId, rtt: Duration) {
        self.update(id, |status| {
            if status.state != GatewayShardState::Ready || status.gateway_rtt == Some(rtt) {
                return false;
            }
            status.gateway_rtt = Some(rtt);
            true
        });
    }

    fn update(&self, id: ShardId, update: impl FnOnce(&mut GatewayShardStatus) -> bool) {
        self.0.send_if_modified(|shards| {
            shards
                .get_mut(id.number() as usize)
                .filter(|status| status.id == id)
                .is_some_and(update)
        });
    }
}

/// Gateway の状態が変化したときに、最新の一覧を受け取ります。
#[derive(Debug)]
pub struct GatewayStatusUpdates(watch::Receiver<Vec<GatewayShardStatus>>);

impl GatewayStatusUpdates {
    /// 次の変化を待ち、シャード番号順の状態を取得します。
    /// ハンドルと Bot のすべての発行元がなくなると `None` です。
    pub async fn next(&mut self) -> Option<Vec<GatewayShardStatus>> {
        self.0.changed().await.ok()?;
        Some(self.0.borrow_and_update().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disconnected_shard_discards_its_latency_without_changing_other_shards() {
        let status = GatewayStatusHandle::new();
        let mut changes = status.subscribe();
        let first = ShardId::new(0, 2);
        let second = ShardId::new(1, 2);
        status.initialize([first, second]);
        assert_eq!(
            changes.next().await.unwrap()[0].state,
            GatewayShardState::Queued
        );
        status.transition(first, GatewayShardState::Ready);
        status.transition(second, GatewayShardState::Ready);
        let rtt = Duration::from_millis(30);
        status.heartbeat(first, rtt);
        status.heartbeat(second, rtt);
        status.transition(second, GatewayShardState::Reconnecting);
        status.heartbeat(second, rtt);
        let shards = changes.next().await.unwrap();
        assert_eq!(shards[0].gateway_rtt, Some(rtt));
        assert_eq!(shards[1].state, GatewayShardState::Reconnecting);
        assert_eq!(shards[1].gateway_rtt, None);
        drop(status);
        assert!(changes.next().await.is_none());
    }
}
