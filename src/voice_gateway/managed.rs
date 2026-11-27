use std::{collections::HashMap, fmt, sync::Arc};

use tokio::{
    sync::{oneshot, watch},
    time::Instant,
};
use twilight_gateway::Event;
use twilight_model::{
    gateway::payload::incoming::{VoiceServerUpdate, VoiceStateUpdate},
    id::{
        Id,
        marker::{ChannelMarker, GuildMarker, UserMarker},
    },
};

use super::{QUEUE_CAPACITY, VoiceGatewayError, VoiceReply, VoiceRequest};

/// 音声処理ライブラリに渡す、一時的な接続情報です。
///
/// 接続先が変わることがあるため、ファイル等に保存せず、最新の通知を使ってください。
pub struct VoiceConnectionInfo {
    /// サーバー ID。
    pub guild_id: Id<GuildMarker>,
    /// 接続中のチャンネル ID。
    pub channel_id: Id<ChannelMarker>,
    /// Bot 自身のユーザー ID。
    pub user_id: Id<UserMarker>,
    /// プロトコル指定を含まない音声サーバーの接続先。ポートを含むことがあります。
    pub endpoint: String,
    session_id: String,
    token: String,
}

impl VoiceConnectionInfo {
    /// 音声接続のセッション ID。ログに出さないでください。
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Discord が発行した一時トークン。ログに出さないでください。
    pub fn token(&self) -> &str {
        &self.token
    }
}

impl fmt::Debug for VoiceConnectionInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VoiceConnectionInfo")
            .field("guild_id", &self.guild_id)
            .field("channel_id", &self.channel_id)
            .field("user_id", &self.user_id)
            .finish_non_exhaustive()
    }
}

/// 音声サーバーの割り当て状況です。
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum VoiceConnectionStatus {
    /// 使用できる音声サーバーの情報が揃いました。
    Ready(Arc<VoiceConnectionInfo>),
    /// 現在の情報を破棄し、Discord から新しい音声サーバーが割り当てられるのを待ちます。
    AwaitingServer,
    /// 退出、外部からの移動、Gateway のセッション交換または停止によって管理を終了しました。
    Disconnected,
}

/// 接続先の割り当てを受け取り、更新を購読するハンドルです。
///
/// 音声データは送受信しません。ハンドルを破棄しても Bot は退出しません。
#[derive(Clone, Debug)]
pub struct VoiceConnection(watch::Receiver<VoiceConnectionStatus>);

impl VoiceConnection {
    /// 現在の割り当て状況を返します。
    pub fn status(&self) -> VoiceConnectionStatus {
        self.0.borrow().clone()
    }

    /// 次の割り当て状況を待ちます。短時間に複数の変更があれば最新の状態を返します。
    /// 新しい [`VoiceConnectionStatus::Ready`] では、前の接続情報を破棄して接続し直してください。
    ///
    /// # エラー
    ///
    /// 最後の状態を受け取ったあとに接続管理が終了した場合、`Closed` を返します。
    pub async fn changed(&mut self) -> Result<VoiceConnectionStatus, VoiceGatewayError> {
        self.0
            .changed()
            .await
            .map_err(|_| VoiceGatewayError::Closed)?;
        Ok(self.0.borrow_and_update().clone())
    }
}

type Reply<T> = oneshot::Sender<Result<T, VoiceGatewayError>>;

#[derive(Default)]
pub(crate) struct VoiceHandshakes {
    guilds: HashMap<Id<GuildMarker>, Entry>,
}

enum Entry {
    Joining(Joining),
    Connected(Connected),
    Leaving(Leaving),
    Indeterminate,
}

struct Joining {
    deadline: Instant,
    reply: Reply<VoiceConnection>,
    channel: Id<ChannelMarker>,
    session_id: Option<String>,
    server: Option<Server>,
    previous_token: Option<String>,
}

struct Leaving {
    deadline: Instant,
    reply: Reply<()>,
    fenced: bool,
}

struct Connected {
    channel: Id<ChannelMarker>,
    session_id: String,
    status: watch::Sender<VoiceConnectionStatus>,
}

struct Server {
    endpoint: String,
    token: String,
}

impl VoiceHandshakes {
    /// Gateway にコマンドを渡す直前に呼ぶ。送信済みの待機は必ず一件ずつ保持する。
    pub(crate) fn request(
        &mut self,
        request: VoiceRequest,
        send: impl FnOnce(&twilight_model::gateway::payload::outgoing::UpdateVoiceState),
    ) {
        self.expire();
        let guild = request.update.d.guild_id;
        match request.reply {
            VoiceReply::Raw(reply) if !reply.is_closed() => {
                if self.guilds.contains_key(&guild) {
                    let _ = reply.send(Err(VoiceGatewayError::Managed));
                } else {
                    send(&request.update);
                    let _ = reply.send(Ok(()));
                }
            }
            VoiceReply::Join { deadline, reply } if !reply.is_closed() => {
                if let Some(channel) = request.update.d.channel_id
                    && self.start_join(guild, channel, deadline, reply)
                {
                    send(&request.update);
                }
            }
            VoiceReply::Leave { deadline, reply } if !reply.is_closed() => {
                if self.start_leave(guild, deadline, reply) {
                    send(&request.update);
                }
            }
            _ => {}
        }
    }

    fn start_join(
        &mut self,
        guild: Id<GuildMarker>,
        channel: Id<ChannelMarker>,
        deadline: Instant,
        reply: Reply<VoiceConnection>,
    ) -> bool {
        if deadline <= Instant::now() {
            let _ = reply.send(Err(VoiceGatewayError::TimedOut));
            return false;
        }
        let previous_token = match self.guilds.get(&guild) {
            Some(Entry::Indeterminate) => {
                let _ = reply.send(Err(VoiceGatewayError::Indeterminate));
                return false;
            }
            Some(Entry::Connected(connected)) if connected.channel == channel => {
                let _ = reply.send(Ok(VoiceConnection(connected.status.subscribe())));
                return false;
            }
            Some(Entry::Connected(connected)) => {
                let status = connected.status.borrow();
                if let VoiceConnectionStatus::Ready(info) = &*status {
                    Some(info.token.clone())
                } else {
                    let _ = reply.send(Err(VoiceGatewayError::NotReady));
                    return false;
                }
            }
            Some(_) => {
                let _ = reply.send(Err(VoiceGatewayError::Managed));
                return false;
            }
            None if self.guilds.len() >= QUEUE_CAPACITY => {
                let _ = reply.send(Err(VoiceGatewayError::Busy));
                return false;
            }
            None => None,
        };
        self.replace(
            guild,
            Entry::Joining(Joining {
                channel,
                deadline,
                reply,
                session_id: None,
                server: None,
                previous_token,
            }),
        );
        true
    }

    fn start_leave(&mut self, guild: Id<GuildMarker>, deadline: Instant, reply: Reply<()>) -> bool {
        if deadline <= Instant::now() {
            let _ = reply.send(Err(VoiceGatewayError::TimedOut));
            return false;
        }
        let fenced = match self.guilds.get(&guild) {
            Some(Entry::Connected(_)) => false,
            Some(Entry::Indeterminate) => true,
            Some(_) => {
                let _ = reply.send(Err(VoiceGatewayError::Managed));
                return false;
            }
            None if self.guilds.len() >= QUEUE_CAPACITY => {
                let _ = reply.send(Err(VoiceGatewayError::Busy));
                return false;
            }
            None => false,
        };
        self.replace(
            guild,
            Entry::Leaving(Leaving {
                deadline,
                reply,
                fenced,
            }),
        );
        true
    }

    fn replace(&mut self, guild: Id<GuildMarker>, new: Entry) {
        if let Some(Entry::Connected(old)) = self.guilds.insert(guild, new) {
            old.status.send_replace(VoiceConnectionStatus::Disconnected);
        }
    }

    pub(crate) fn expire(&mut self) {
        let now = Instant::now();
        for entry in self.guilds.values_mut() {
            if match entry {
                Entry::Joining(join) => join.deadline <= now || join.reply.is_closed(),
                Entry::Leaving(leave) => leave.deadline <= now || leave.reply.is_closed(),
                _ => false,
            } {
                std::mem::replace(entry, Entry::Indeterminate).fail(VoiceGatewayError::TimedOut);
            }
        }
    }

    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.guilds
            .values()
            .filter_map(|entry| match entry {
                Entry::Joining(join) => Some(join.deadline),
                Entry::Leaving(leave) => Some(leave.deadline),
                _ => None,
            })
            .min()
    }

    pub(crate) fn lost_gateway(&mut self) {
        for entry in self.guilds.values_mut() {
            match entry {
                Entry::Connected(connected) => {
                    connected
                        .status
                        .send_replace(VoiceConnectionStatus::AwaitingServer);
                }
                Entry::Joining(_) | Entry::Leaving(_) => {
                    std::mem::replace(entry, Entry::Indeterminate)
                        .fail(VoiceGatewayError::NotReady);
                }
                Entry::Indeterminate => {}
            }
        }
    }

    pub(crate) fn observe(&mut self, event: &Event, own_id: Option<Id<UserMarker>>) {
        match event {
            Event::Ready(_) => self.reset(),
            Event::VoiceStateUpdate(state) if own_id == Some(state.user_id) => {
                self.expire();
                if let Some(guild) = state.guild_id {
                    self.observe_state(guild, state);
                    self.finish_join(guild, state.user_id);
                }
            }
            Event::VoiceServerUpdate(server) => {
                if let Some(own_id) = own_id {
                    self.expire();
                    self.observe_server(server, own_id);
                    self.finish_join(server.guild_id, own_id);
                }
            }
            _ => {}
        }
    }

    fn observe_state(&mut self, guild: Id<GuildMarker>, state: &VoiceStateUpdate) {
        let Some(entry) = self.guilds.get_mut(&guild) else {
            return;
        };
        match entry {
            Entry::Joining(join) if state.channel_id == Some(join.channel) => {
                if !state.session_id.is_empty() {
                    join.session_id = Some(state.session_id.clone());
                }
            }
            Entry::Leaving(_) if state.channel_id.is_none() => {
                if let Some(Entry::Leaving(leave)) = self.guilds.remove(&guild) {
                    if leave.fenced {
                        self.guilds.insert(guild, Entry::Indeterminate);
                    }
                    let _ = leave.reply.send(Ok(()));
                }
            }
            Entry::Connected(conn) if state.channel_id != Some(conn.channel) => {
                conn.status
                    .send_replace(VoiceConnectionStatus::Disconnected);
                if state.channel_id.is_none() {
                    self.guilds.remove(&guild);
                } else {
                    // 外部から移動された場合も、前の接続に対する遅延応答との区別ができない。
                    self.guilds.insert(guild, Entry::Indeterminate);
                }
            }
            Entry::Connected(conn) if state.session_id != conn.session_id => {
                conn.session_id.clone_from(&state.session_id);
                conn.status
                    .send_replace(VoiceConnectionStatus::AwaitingServer);
            }
            _ => {}
        }
    }

    fn observe_server(&mut self, server: &VoiceServerUpdate, user: Id<UserMarker>) {
        let Some(entry) = self.guilds.get_mut(&server.guild_id) else {
            return;
        };
        let server_info = || {
            server
                .endpoint
                .as_ref()
                .filter(|endpoint| !endpoint.is_empty() && !server.token.is_empty())
                .map(|endpoint| Server {
                    endpoint: endpoint.clone(),
                    token: server.token.clone(),
                })
        };
        match entry {
            Entry::Joining(join) => {
                if join.previous_token.as_deref() != Some(server.token.as_str())
                    || server.endpoint.is_none()
                {
                    join.server = server_info();
                }
            }
            Entry::Connected(conn) => {
                let status =
                    server_info().map_or(VoiceConnectionStatus::AwaitingServer, |server_info| {
                        VoiceConnectionStatus::Ready(Arc::new(server_info.info(
                            server.guild_id,
                            conn.channel,
                            user,
                            conn.session_id.clone(),
                        )))
                    });
                conn.status.send_replace(status);
            }
            _ => {}
        }
    }

    fn finish_join(&mut self, guild: Id<GuildMarker>, user: Id<UserMarker>) {
        let Some(Entry::Joining(join)) = self.guilds.get_mut(&guild) else {
            return;
        };
        let (Some(session), Some(server)) = (&join.session_id, &join.server) else {
            return;
        };
        let info = Arc::new(VoiceConnectionInfo {
            guild_id: guild,
            channel_id: join.channel,
            user_id: user,
            endpoint: server.endpoint.clone(),
            session_id: session.clone(),
            token: server.token.clone(),
        });
        let (status, receiver) = watch::channel(VoiceConnectionStatus::Ready(Arc::clone(&info)));
        let connected = Connected {
            channel: info.channel_id,
            session_id: info.session_id.clone(),
            status,
        };
        if let Some(Entry::Joining(join)) = self.guilds.insert(guild, Entry::Connected(connected))
            && join.reply.send(Ok(VoiceConnection(receiver))).is_err()
        {
            self.guilds.insert(guild, Entry::Indeterminate);
        }
    }

    fn reset(&mut self) {
        for (_, entry) in self.guilds.drain() {
            entry.fail(VoiceGatewayError::Closed);
        }
    }
}

impl Drop for VoiceHandshakes {
    fn drop(&mut self) {
        self.reset();
    }
}

impl Entry {
    fn fail(self, error: VoiceGatewayError) {
        match self {
            Self::Joining(join) => {
                let _ = join.reply.send(Err(error));
            }
            Self::Leaving(leave) => {
                let _ = leave.reply.send(Err(error));
            }
            Self::Connected(conn) => {
                conn.status
                    .send_replace(VoiceConnectionStatus::Disconnected);
            }
            Self::Indeterminate => {}
        }
    }
}

impl Server {
    fn info(
        self,
        guild: Id<GuildMarker>,
        channel: Id<ChannelMarker>,
        user: Id<UserMarker>,
        session_id: String,
    ) -> VoiceConnectionInfo {
        VoiceConnectionInfo {
            guild_id: guild,
            channel_id: channel,
            user_id: user,
            endpoint: self.endpoint,
            token: self.token,
            session_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use twilight_model::gateway::payload::outgoing::UpdateVoiceState;

    use super::*;

    #[test]
    fn cancellation_before_dispatch_leaves_the_guild_available() {
        let mut managed = VoiceHandshakes::default();
        let guild = Id::new(7);
        let channel = Id::new(8);
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        let (cancelled, result) = oneshot::channel();
        drop(result);
        let mut make_request = |reply, sent: &mut bool| {
            managed.request(
                VoiceRequest {
                    update: UpdateVoiceState::new(guild, Some(channel), false, false),
                    generation: 1,
                    reply: VoiceReply::Join { deadline, reply },
                },
                |_| *sent = true,
            );
        };
        let mut sent = false;
        make_request(cancelled, &mut sent);
        assert!(!sent);
        let (retry, _result) = oneshot::channel();
        make_request(retry, &mut sent);
        assert!(sent);
        assert!(matches!(
            managed.guilds.get(&guild),
            Some(Entry::Joining(_))
        ));
    }
}
