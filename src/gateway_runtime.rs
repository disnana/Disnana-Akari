use std::{future::pending, sync::atomic::Ordering, time::Duration};

use futures_util::{StreamExt, stream::FuturesUnordered};
use tokio::{sync::mpsc, time::timeout};
use twilight_gateway::{ConfigBuilder, Event, EventTypeFlags, Shard, ShardId, ShardState};
use twilight_model::id::{Id, marker::UserMarker};

use crate::{
    Error, GatewayShardState, GatewayStatusHandle, PresenceHandle, Result,
    message_events::{self, GatewayDecodeOptions, IncomingEvent},
    presence::PresenceUpdates,
    shutdown,
    voice_gateway::{VoiceGateway, VoiceGatewayError, VoiceIngress},
};

mod plan;
pub(crate) use plan::GatewayPlan;

const WAVE_READY_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) struct GatewaySet {
    sessions: Vec<GatewaySession>,
    wave_size: Option<usize>,
}

impl GatewaySet {
    pub(crate) fn observe(&mut self, status: &GatewayStatusHandle) {
        status.initialize(self.sessions.iter().map(GatewaySession::id));
        for session in &mut self.sessions {
            session.status = Some(status.clone());
        }
    }

    pub(crate) fn install_voice(&mut self, handle: &VoiceGateway) {
        let receivers = handle.initialize(self.sessions.len());
        for (session, ingress) in self.sessions.iter_mut().zip(receivers) {
            session.voice = Some(ingress);
        }
    }

    pub(crate) async fn drive(
        &mut self,
        event_types: EventTypeFlags,
        decode: GatewayDecodeOptions,
        invalidate_permissions: impl Fn(ShardId),
        route: impl Fn(&mut GatewaySession, Result<IncomingEvent>),
    ) -> Error {
        let Some(wave_size) = self.wave_size else {
            let Some(session) = self.sessions.first_mut() else {
                return Error::GatewayEnded;
            };
            return session
                .drive(event_types, decode, &invalidate_permissions, &route, None)
                .await;
        };

        let (ready_tx, mut ready_rx) = mpsc::channel(wave_size);
        let mut running = FuturesUnordered::new();
        let mut waiting = self.sessions.iter_mut();
        loop {
            let mut count = 0;
            for session in waiting.by_ref().take(wave_size) {
                running.push(session.drive(
                    event_types,
                    decode,
                    &invalidate_permissions,
                    &route,
                    Some(&ready_tx),
                ));
                count += 1;
            }
            if count == 0 {
                break;
            }
            // 前の組の Identify がすべて完了するまで、次の組の接続を開始しない。
            let wave_ready = async {
                for _ in 0..count {
                    if ready_rx.recv().await.is_none() {
                        return;
                    }
                }
            };
            tokio::select! {
                biased;
                error = running.next() => return error.unwrap_or(Error::GatewayEnded),
                ready = timeout(WAVE_READY_TIMEOUT, wave_ready) => {
                    if ready.is_err() {
                        return Error::GatewayWaveTimedOut;
                    }
                }
            }
        }
        running.next().await.unwrap_or(Error::GatewayEnded)
    }

    pub(crate) async fn close(&mut self) {
        let mut closing: FuturesUnordered<_> = self
            .sessions
            .iter_mut()
            .map(GatewaySession::close)
            .collect();
        while closing.next().await.is_some() {}
    }
}

pub(crate) struct GatewaySession {
    shard: Shard,
    presence: Option<PresenceUpdates>,
    own_id: Option<Id<UserMarker>>,
    status: Option<GatewayStatusHandle>,
    voice: Option<VoiceIngress>,
}

impl GatewaySession {
    pub(crate) fn new(
        id: ShardId,
        mut config: ConfigBuilder,
        presence: Option<&PresenceHandle>,
    ) -> Self {
        let mut presence = presence.map(PresenceHandle::subscribe);
        if let Some(updates) = &mut presence {
            config = config.presence(updates.initial());
        }
        Self {
            shard: Shard::with_config(id, config.build()),
            presence,
            own_id: None,
            status: None,
            voice: None,
        }
    }

    async fn next_event(
        &mut self,
        event_types: EventTypeFlags,
        decode: GatewayDecodeOptions,
        invalidate_permissions: &impl Fn(ShardId),
    ) -> Option<Result<IncomingEvent>> {
        let had_session = self.own_id.is_some();
        let id = self.id();
        let (mut voice_rx, voice_generation) = self.voice.as_mut().map(VoiceIngress::parts).unzip();
        let observe_state = |previous, state| {
            if lost_gateway_session(previous, state) {
                invalidate_permissions(id);
                if let Some(generation) = voice_generation {
                    VoiceIngress::transition(generation, false);
                }
            }
            if let Some(status) = &self.status {
                match state {
                    ShardState::FatallyClosed => {
                        status.transition(id, GatewayShardState::Closed);
                    }
                    ShardState::Disconnected { .. }
                    | ShardState::Identifying
                    | ShardState::Resuming
                        if had_session =>
                    {
                        status.transition(id, GatewayShardState::Reconnecting);
                    }
                    _ => {}
                }
            }
        };
        loop {
            let active = self.shard.state() == ShardState::Active;
            let presence_ready = active && self.presence.is_some();
            if !presence_ready && voice_rx.is_none() {
                return message_events::next_event(
                    &mut self.shard,
                    event_types,
                    decode,
                    observe_state,
                )
                .await;
            }
            tokio::select! {
                event = message_events::next_event(&mut self.shard, event_types, decode, observe_state) => return event,
                update = async {
                    match &mut self.presence {
                        Some(updates) => updates.next().await,
                        None => pending().await,
                    }
                }, if presence_ready => self.shard.command(&update),
                request = async {
                    match &mut voice_rx {
                        Some(receiver) => receiver.recv().await,
                        None => pending().await,
                    }
                }, if voice_rx.is_some() => {
                    let Some(request) = request else {
                        voice_rx = None;
                        continue;
                    };
                    if request.acknowledge.is_closed() {
                        continue;
                    }
                    let current = voice_generation.map(|generation| generation.load(Ordering::Acquire));
                    let result = if self.shard.state() == ShardState::Active
                        && current == Some(request.generation)
                    {
                        self.shard.command(&request.update);
                        Ok(())
                    } else {
                        Err(VoiceGatewayError::NotReady)
                    };
                    let _ = request.acknowledge.send(result);
                }
            }
        }
    }

    async fn drive(
        &mut self,
        mut event_types: EventTypeFlags,
        decode: GatewayDecodeOptions,
        invalidate_permissions: &impl Fn(ShardId),
        route: &impl Fn(&mut Self, Result<IncomingEvent>),
        mut ready: Option<&mpsc::Sender<()>>,
    ) -> Error {
        if let Some(status) = &self.status {
            status.transition(self.id(), GatewayShardState::Connecting);
            event_types |= EventTypeFlags::GATEWAY_HEARTBEAT_ACK;
        }
        while let Some(event) = self
            .next_event(event_types, decode, invalidate_permissions)
            .await
        {
            let is_ready = matches!(&event, Ok(IncomingEvent::Standard(Event::Ready(_))));
            if let Some(voice) = &self.voice
                && matches!(
                    &event,
                    Ok(IncomingEvent::Standard(Event::Ready(_) | Event::Resumed))
                )
            {
                voice.set_ready(true);
            }
            if let Some(status) = &self.status {
                self.observe_event(status, &event);
            }
            route(self, event);
            if is_ready
                && let Some(sender) = ready.take()
                && sender.try_send(()).is_err()
            {
                return Error::GatewayEnded;
            }
        }
        Error::GatewayEnded
    }

    fn observe_event(&self, status: &GatewayStatusHandle, event: &Result<IncomingEvent>) {
        let id = self.id();
        match event {
            Ok(IncomingEvent::Standard(Event::Ready(_) | Event::Resumed)) => {
                status.transition(id, GatewayShardState::Ready);
                if let Some(rtt) = self.gateway_rtt() {
                    status.heartbeat(id, rtt);
                }
            }
            Ok(IncomingEvent::Standard(Event::GatewayHeartbeatAck)) => {
                if let Some(rtt) = self.gateway_rtt() {
                    status.heartbeat(id, rtt);
                }
            }
            _ => {}
        }
    }

    pub(crate) const fn id(&self) -> ShardId {
        self.shard.id()
    }

    pub(crate) const fn own_id(&self) -> Option<Id<UserMarker>> {
        self.own_id
    }

    pub(crate) fn ready(&mut self, user_id: Id<UserMarker>) {
        if self.own_id.replace(user_id).is_some() {
            self.replay_presence();
        }
    }

    pub(crate) fn replay_presence(&mut self) {
        if let Some(updates) = &mut self.presence {
            updates.replay();
        }
    }

    pub(crate) fn gateway_rtt(&self) -> Option<Duration> {
        self.shard.latency().recent().first().copied()
    }

    pub(crate) async fn close(&mut self) {
        if let Some(voice) = &self.voice {
            voice.set_ready(false);
            self.voice = None;
        }
        shutdown::close_shard(&mut self.shard).await;
        if let Some(status) = &self.status {
            status.transition(self.id(), GatewayShardState::Closed);
        }
    }
}

fn lost_gateway_session(previous: ShardState, current: ShardState) -> bool {
    match previous {
        ShardState::Active => current != ShardState::Active,
        ShardState::Resuming => !current.is_identified(),
        _ => false,
    }
}

impl Drop for GatewaySession {
    fn drop(&mut self) {
        if let Some(status) = &self.status {
            status.transition(self.id(), GatewayShardState::Closed);
        }
    }
}

#[cfg(test)]
mod tests;
