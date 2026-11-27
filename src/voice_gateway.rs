use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use tokio::sync::{mpsc, oneshot, watch};
use twilight_model::{
    gateway::payload::outgoing::UpdateVoiceState,
    id::{
        Id,
        marker::{ChannelMarker, GuildMarker},
    },
};

const QUEUE_CAPACITY: usize = 32;

/// Bot 自身のボイスチャンネルとミュート状態を変更します。
#[derive(Clone, Copy, Debug)]
pub struct VoiceStateChange {
    channel: Option<Id<ChannelMarker>>,
    self_mute: bool,
    self_deaf: bool,
}

impl VoiceStateChange {
    /// チャンネルへ参加するか、今の接続から移動します。
    pub const fn join(channel: Id<ChannelMarker>) -> Self {
        Self {
            channel: Some(channel),
            self_mute: false,
            self_deaf: false,
        }
    }

    /// 現在のチャンネルから退出します。
    pub const fn leave() -> Self {
        Self {
            channel: None,
            self_mute: false,
            self_deaf: false,
        }
    }

    /// Bot の送話をミュートするか指定します。
    #[must_use]
    pub const fn self_mute(mut self, mute: bool) -> Self {
        self.self_mute = mute;
        self
    }

    /// Bot の受話をミュートするか指定します。
    #[must_use]
    pub const fn self_deaf(mut self, deaf: bool) -> Self {
        self.self_deaf = deaf;
        self
    }
}

/// 音声状態変更を Gateway へ渡せなかった理由です。
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum VoiceGatewayError {
    /// Bot が起動していないか、対象シャードが準備中です。
    #[error("音声状態を更新する Gateway シャードが準備できていません")]
    NotReady,
    /// シャードの音声状態変更キューが満杯です。
    #[error("音声状態変更のキューが満杯です")]
    Busy,
    /// 変更を処理する前に Gateway が終了しました。
    #[error("音声状態の変更を処理する前に Gateway が終了しました")]
    Closed,
}

#[derive(Clone)]
struct VoiceRoute {
    sender: mpsc::Sender<VoiceRequest>,
    ready: Arc<AtomicU64>,
}

/// ボイスチャンネルの参加、移動、退出の要求を担当シャードへ渡します。
///
/// [`Bot::with_voice_gateway`](crate::Bot::with_voice_gateway) で取得します。
#[derive(Clone)]
pub struct VoiceGateway(watch::Sender<Vec<VoiceRoute>>);

impl std::fmt::Debug for VoiceGateway {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VoiceGateway")
            .finish_non_exhaustive()
    }
}

impl VoiceGateway {
    pub(crate) fn new() -> Self {
        Self(watch::Sender::new(Vec::new()))
    }

    /// 指定の音声状態を担当シャードの送信キューに入れます。
    ///
    /// 成功は Gateway 内で要求を受理したことを示します。Bot 自身の
    /// VoiceStateUpdate と VoiceServerUpdate を使って音声接続の進行を確認できます。
    ///
    /// # エラー
    ///
    /// 対象シャードが準備中または終了済みの場合、処理中に終了した場合、
    /// キューが満杯の場合はエラーを返します。
    pub async fn set_state(
        &self,
        guild: Id<GuildMarker>,
        change: VoiceStateChange,
    ) -> Result<(), VoiceGatewayError> {
        let route = {
            let routes = self.0.borrow();
            let total = u64::try_from(routes.len()).map_err(|_| VoiceGatewayError::NotReady)?;
            let index = (guild.get() >> 22)
                .checked_rem(total)
                .ok_or(VoiceGatewayError::NotReady)?;
            let index = usize::try_from(index).map_err(|_| VoiceGatewayError::NotReady)?;
            routes
                .get(index)
                .cloned()
                .ok_or(VoiceGatewayError::NotReady)?
        };
        if route.sender.is_closed() {
            return Err(VoiceGatewayError::Closed);
        }
        let generation = route.ready.load(Ordering::Acquire);
        if generation & 1 == 0 {
            return Err(VoiceGatewayError::NotReady);
        }
        let (acknowledge, accepted) = oneshot::channel();
        route
            .sender
            .try_send(VoiceRequest {
                update: UpdateVoiceState::new(
                    guild,
                    change.channel,
                    change.self_deaf,
                    change.self_mute,
                ),
                generation,
                acknowledge,
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => VoiceGatewayError::Busy,
                mpsc::error::TrySendError::Closed(_) => VoiceGatewayError::Closed,
            })?;
        accepted.await.map_err(|_| VoiceGatewayError::Closed)?
    }

    pub(crate) fn initialize(&self, count: usize) -> Vec<VoiceIngress> {
        let mut routes = Vec::with_capacity(count);
        let mut receivers = Vec::with_capacity(count);
        for _ in 0..count {
            let (sender, receiver) = mpsc::channel(QUEUE_CAPACITY);
            let ready = Arc::new(AtomicU64::new(0));
            routes.push(VoiceRoute {
                sender,
                ready: Arc::clone(&ready),
            });
            receivers.push(VoiceIngress { receiver, ready });
        }
        self.0.send_replace(routes);
        receivers
    }
}

pub(crate) struct VoiceRequest {
    pub(crate) update: UpdateVoiceState,
    pub(crate) generation: u64,
    pub(crate) acknowledge: oneshot::Sender<Result<(), VoiceGatewayError>>,
}

pub(crate) struct VoiceIngress {
    receiver: mpsc::Receiver<VoiceRequest>,
    ready: Arc<AtomicU64>,
}

impl VoiceIngress {
    pub(crate) fn set_ready(&self, ready: bool) {
        Self::transition(&self.ready, ready);
    }

    pub(crate) fn transition(generation: &AtomicU64, ready: bool) {
        if (generation.load(Ordering::Relaxed) & 1 == 1) != ready {
            generation.fetch_add(1, Ordering::Release);
        }
    }

    pub(crate) fn parts(&mut self) -> (&mut mpsc::Receiver<VoiceRequest>, &AtomicU64) {
        (&mut self.receiver, &self.ready)
    }
}
