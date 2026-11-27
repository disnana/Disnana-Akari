use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use tokio::{
    sync::{Notify, mpsc, oneshot, watch},
    time::{Instant, timeout_at},
};
use twilight_model::{
    gateway::payload::outgoing::UpdateVoiceState,
    id::{
        Id,
        marker::{ChannelMarker, GuildMarker},
    },
};

mod managed;
pub(crate) use managed::VoiceHandshakes;
pub use managed::{VoiceConnection, VoiceConnectionInfo, VoiceConnectionStatus};

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

/// 音声操作を完了できなかった理由です。
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
    /// このサーバーには進行中の音声操作、または管理中の接続があります。
    #[error("このサーバーの音声接続は別の操作が管理しています")]
    Managed,
    /// Discord から必要なイベントが期限内に届きませんでした。
    #[error("音声状態の確定が時間切れになりました")]
    TimedOut,
    /// 直前の操作の結果を確定できません。新しい Gateway セッションまで参加を保留します。
    #[error("前の音声操作の結果が不明です。新しい Gateway セッションまで参加できません")]
    Indeterminate,
}

#[derive(Clone)]
struct VoiceRoute {
    sender: mpsc::Sender<VoiceRequest>,
    ready: Arc<AtomicU64>,
    cancellation: Arc<Notify>,
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
    /// 同じサーバーで、このメソッドと [`Self::join`]・[`Self::leave`] を併用しないでください。
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
        let route = self.route(guild)?;
        let (acknowledge, accepted) = oneshot::channel();
        route.send(
            UpdateVoiceState::new(guild, change.channel, change.self_deaf, change.self_mute),
            VoiceReply::Raw(acknowledge),
        )?;
        accepted.await.map_err(|_| VoiceGatewayError::Closed)?
    }

    /// ボイスチャンネルへの参加を要求し、自身の状態と音声サーバー情報の両方を待ちます。
    ///
    /// 同じサーバーでは操作を一件ずつ処理します。現在のチャンネルを指定すると、
    /// 同じ接続の購読を返します。別のチャンネルでは新しい割り当てを待ちます。
    /// 送信後に中断した操作の応答を次の参加と混ぜないため、新しい Gateway
    /// セッションを確立するまで、そのサーバーへの参加を保留することがあります。
    ///
    /// # エラー
    ///
    /// Gateway が利用できない場合、操作が競合した場合、または期限までに
    /// Discord の応答が揃わない場合はエラーになります。
    pub async fn join(
        &self,
        guild: Id<GuildMarker>,
        channel: Id<ChannelMarker>,
        timeout: Duration,
    ) -> Result<VoiceConnection, VoiceGatewayError> {
        let route = self.route(guild)?;
        let deadline = deadline(timeout)?;
        let (reply, result) = oneshot::channel();
        route.send(
            UpdateVoiceState::new(guild, Some(channel), false, false),
            VoiceReply::Join { deadline, reply },
        )?;
        await_reply(result, deadline, &route.cancellation).await
    }

    /// 現在のチャンネルから退出し、Discord が Bot 自身の退出を通知するまで待ちます。
    /// 以前の参加が時間切れなどで確定できなかった場合、このメソッドで退出を確認しても、
    /// 新しい Gateway セッションを確立するまで、同じサーバーへの参加は保留されます。
    ///
    /// # エラー
    ///
    /// Gateway が利用できない場合、操作が競合した場合、または期限までに
    /// Discord から退出通知が届かない場合はエラーになります。
    pub async fn leave(
        &self,
        guild: Id<GuildMarker>,
        timeout: Duration,
    ) -> Result<(), VoiceGatewayError> {
        let route = self.route(guild)?;
        let deadline = deadline(timeout)?;
        let (reply, result) = oneshot::channel();
        route.send(
            UpdateVoiceState::new(guild, None, false, false),
            VoiceReply::Leave { deadline, reply },
        )?;
        await_reply(result, deadline, &route.cancellation).await
    }

    fn route(&self, guild: Id<GuildMarker>) -> Result<VoiceRoute, VoiceGatewayError> {
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
        Ok(route)
    }

    pub(crate) fn initialize(&self, count: usize) -> Vec<VoiceIngress> {
        let mut routes = Vec::with_capacity(count);
        let mut receivers = Vec::with_capacity(count);
        for _ in 0..count {
            let (sender, receiver) = mpsc::channel(QUEUE_CAPACITY);
            let ready = Arc::new(AtomicU64::new(0));
            let cancellation = Arc::new(Notify::new());
            routes.push(VoiceRoute {
                sender,
                ready: Arc::clone(&ready),
                cancellation: Arc::clone(&cancellation),
            });
            receivers.push(VoiceIngress {
                receiver,
                ready,
                cancellation,
                managed: Mutex::default(),
            });
        }
        self.0.send_replace(routes);
        receivers
    }
}

fn deadline(timeout: Duration) -> Result<Instant, VoiceGatewayError> {
    if timeout.is_zero() {
        return Err(VoiceGatewayError::TimedOut);
    }
    Instant::now()
        .checked_add(timeout)
        .ok_or(VoiceGatewayError::TimedOut)
}

async fn await_reply<T>(
    reply: oneshot::Receiver<Result<T, VoiceGatewayError>>,
    deadline: Instant,
    notify: &Notify,
) -> Result<T, VoiceGatewayError> {
    let mut pending = NotifyOnDrop { reply, notify };
    timeout_at(deadline, &mut pending.reply)
        .await
        .map_err(|_| VoiceGatewayError::TimedOut)?
        .map_err(|_| VoiceGatewayError::Closed)?
}

struct NotifyOnDrop<'a, T> {
    reply: oneshot::Receiver<Result<T, VoiceGatewayError>>,
    notify: &'a Notify,
}

impl<T> Drop for NotifyOnDrop<'_, T> {
    fn drop(&mut self) {
        self.reply.close();
        self.notify.notify_one();
    }
}

impl VoiceRoute {
    fn send(&self, update: UpdateVoiceState, reply: VoiceReply) -> Result<(), VoiceGatewayError> {
        let generation = self.ready.load(Ordering::Acquire);
        if generation & 1 == 0 {
            return Err(VoiceGatewayError::NotReady);
        }
        self.sender
            .try_send(VoiceRequest {
                update,
                generation,
                reply,
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => VoiceGatewayError::Busy,
                mpsc::error::TrySendError::Closed(_) => VoiceGatewayError::Closed,
            })
    }
}

pub(crate) struct VoiceRequest {
    pub(crate) update: UpdateVoiceState,
    pub(crate) generation: u64,
    pub(crate) reply: VoiceReply,
}

pub(crate) enum VoiceReply {
    Raw(oneshot::Sender<Result<(), VoiceGatewayError>>),
    Join {
        deadline: Instant,
        reply: oneshot::Sender<Result<VoiceConnection, VoiceGatewayError>>,
    },
    Leave {
        deadline: Instant,
        reply: oneshot::Sender<Result<(), VoiceGatewayError>>,
    },
}

impl VoiceReply {
    pub(crate) fn fail(self, error: VoiceGatewayError) {
        match self {
            Self::Raw(reply) | Self::Leave { reply, .. } => {
                let _ = reply.send(Err(error));
            }
            Self::Join { reply, .. } => {
                let _ = reply.send(Err(error));
            }
        }
    }
}

pub(crate) struct VoiceIngress {
    receiver: mpsc::Receiver<VoiceRequest>,
    ready: Arc<AtomicU64>,
    pub(crate) cancellation: Arc<Notify>,
    pub(crate) managed: Mutex<VoiceHandshakes>,
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

    pub(crate) fn parts(&mut self) -> (&mut mpsc::Receiver<VoiceRequest>, VoiceControl<'_>) {
        (
            &mut self.receiver,
            VoiceControl {
                generation: &self.ready,
                cancellation: &self.cancellation,
                managed: &self.managed,
            },
        )
    }
}

#[derive(Clone, Copy)]
pub(crate) struct VoiceControl<'a> {
    pub(crate) generation: &'a AtomicU64,
    pub(crate) cancellation: &'a Notify,
    pub(crate) managed: &'a Mutex<VoiceHandshakes>,
}
