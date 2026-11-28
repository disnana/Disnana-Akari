use std::{future::pending, sync::atomic::Ordering, time::Duration};

use futures_util::{StreamExt, stream::FuturesUnordered};
use tokio::{
    sync::mpsc,
    time::{sleep_until, timeout},
};
use twilight_gateway::{ConfigBuilder, Event, EventTypeFlags, Intents, Shard, ShardId, ShardState};
use twilight_model::{
    gateway::payload::incoming::rate_limited::RateLimitMetadata,
    id::{Id, marker::UserMarker},
};

use crate::{
    DerivedGatewayEvent, Error, Events, GatewayShardState, GatewayStatusHandle, MemberQuery,
    PresenceHandle, Result,
    guild_lifecycle::{GuildLifecycleTracker, LIFECYCLE_EVENTS},
    member_query::MemberQueryCommand,
    message_events::{self, GatewayDecodeOptions, IncomingEvent},
    presence::PresenceUpdates,
    shutdown,
    voice_gateway::{VoiceGateway, VoiceGatewayError, VoiceIngress},
    voice_state_cache::{VOICE_STATE_CACHE_EVENTS, VoiceStateCache},
};

mod plan;
pub(crate) use plan::GatewayPlan;

const WAVE_READY_TIMEOUT: Duration = Duration::from_secs(60);
// Twilight の送信キューは接続をまたいで残る。排出を確認するまで渡す件数を制限する。
const MEMBER_QUERY_TRANSPORT_LIMIT: u8 = 4;

pub(crate) struct MemberQueryRuntime(pub(crate) Option<MemberQuery>);

impl MemberQueryRuntime {
    pub(crate) fn new(handle: Option<MemberQuery>, intents: Intents) -> Self {
        if let Some(query) = &handle {
            query.configure_intents(intents);
        }
        Self(handle)
    }

    pub(crate) fn close(&self) {
        if let Some(handle) = &self.0 {
            handle.close();
        }
    }
}

impl Drop for MemberQueryRuntime {
    fn drop(&mut self) {
        self.close();
    }
}

struct MemberQueryIngress {
    handle: MemberQuery,
    commands: mpsc::Receiver<MemberQueryCommand>,
    transport_pending: u8,
}

impl MemberQueryIngress {
    fn observe(
        &mut self,
        shard: ShardId,
        event: Result<IncomingEvent>,
        transport_progress: bool,
    ) -> Option<Result<IncomingEvent>> {
        if transport_progress
            && matches!(
                event,
                Ok(IncomingEvent::Standard(
                    Event::GatewayHeartbeatAck | Event::MemberChunk(_) | Event::RateLimited(_)
                ))
            )
        {
            // Twilight は受信フレームを返す前に、送信可能なキューを排出する。
            self.transport_pending = 0;
        }
        if let Ok(IncomingEvent::Standard(Event::MemberChunk(chunk))) = event {
            self.handle.observe_chunk(shard, chunk);
            return None;
        }
        match &event {
            Ok(IncomingEvent::Standard(Event::Ready(_) | Event::Resumed)) => {
                self.handle.ready(shard)
            }
            Ok(IncomingEvent::Standard(Event::RateLimited(event))) => {
                if let RateLimitMetadata::RequestGuildMembers {
                    guild_id,
                    nonce: Some(nonce),
                } = &event.meta
                    && event.opcode == twilight_model::gateway::OpCode::RequestGuildMembers
                    && let Ok(retry_after) = Duration::try_from_secs_f32(event.retry_after)
                {
                    self.handle
                        .observe_rate_limit(shard, *guild_id, nonce, retry_after);
                }
            }
            _ => {}
        }
        Some(event)
    }
}

pub(crate) struct GatewaySet {
    sessions: Vec<GatewaySession>,
    wave_size: Option<usize>,
}

impl GatewaySet {
    pub(crate) fn configure_event_trackers(&mut self, events: &Events) {
        let lifecycle = events.has_guild_lifecycle_handlers();
        let voice_states = events.voice_state_capacity();
        for session in &mut self.sessions {
            let shard = session.id();
            session.guild_lifecycle = lifecycle.then(|| GuildLifecycleTracker::new(shard));
            session.voice_state_cache =
                voice_states.map(|capacity| VoiceStateCache::new(shard, capacity));
        }
    }

    pub(crate) fn install_handles(
        &mut self,
        status: Option<&GatewayStatusHandle>,
        voice: Option<&VoiceGateway>,
        member_query: Option<&MemberQuery>,
    ) {
        if let Some(status) = status {
            status.initialize(self.sessions.iter().map(GatewaySession::id));
            for session in &mut self.sessions {
                session.status = Some(status.clone());
            }
        }
        if let Some(voice) = voice {
            let receivers = voice.initialize(self.sessions.len());
            for (session, ingress) in self.sessions.iter_mut().zip(receivers) {
                session.voice = Some(ingress);
            }
        }
        if let Some(handle) = member_query {
            let receivers = handle.initialize(self.sessions.iter().map(GatewaySession::id));
            for (session, commands) in self.sessions.iter_mut().zip(receivers) {
                session.member_query = Some(MemberQueryIngress {
                    handle: handle.clone(),
                    commands,
                    transport_pending: 0,
                });
            }
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
    member_query: Option<MemberQueryIngress>,
    guild_lifecycle: Option<GuildLifecycleTracker>,
    voice_state_cache: Option<VoiceStateCache>,
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
            member_query: None,
            guild_lifecycle: None,
            voice_state_cache: None,
        }
    }

    pub(crate) fn observe_derived_gateway_event(
        &mut self,
        incoming: &Result<IncomingEvent>,
    ) -> Option<DerivedGatewayEvent> {
        match incoming {
            Ok(IncomingEvent::Standard(event)) => {
                let lifecycle = self
                    .guild_lifecycle
                    .as_mut()
                    .and_then(|tracker| tracker.observe(event));
                let voice_state = self
                    .voice_state_cache
                    .as_mut()
                    .and_then(|cache| cache.observe(event));
                lifecycle
                    .map(DerivedGatewayEvent::GuildLifecycle)
                    .or_else(|| voice_state.map(DerivedGatewayEvent::VoiceState))
            }
            Err(error) => {
                if let Some(tracker) = &mut self.guild_lifecycle
                    && message_events::lost_gateway_event(error, LIFECYCLE_EVENTS)
                {
                    tracker.reset();
                }
                if let Some(cache) = &mut self.voice_state_cache
                    && message_events::lost_gateway_event(error, VOICE_STATE_CACHE_EVENTS)
                {
                    cache.reset();
                }
                None
            }
            _ => None,
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
        let (mut voice_rx, voice) = self.voice.as_mut().map(VoiceIngress::parts).unzip();
        let (mut query_rx, query) = self
            .member_query
            .as_mut()
            .map(|ingress| {
                (
                    &mut ingress.commands,
                    (&ingress.handle, &mut ingress.transport_pending),
                )
            })
            .unzip();
        let (query, mut query_pending) = query.unzip();
        let observe_state = |previous, state| {
            if lost_gateway_session(previous, state) {
                invalidate_permissions(id);
                if let Some(query) = query {
                    query.disconnect(id);
                }
                if let Some(voice) = voice {
                    VoiceIngress::transition(voice.generation, false);
                    if let Ok(mut managed) = voice.managed.lock() {
                        managed.lost_gateway();
                    }
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
            let deadline = voice.and_then(|voice| voice.managed.lock().ok()?.next_deadline());
            let query_capacity = query_pending
                .as_deref()
                .is_some_and(|pending| *pending < MEMBER_QUERY_TRANSPORT_LIMIT);
            if !presence_ready && voice_rx.is_none() && query_rx.is_none() {
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
                command = async {
                    match &mut query_rx {
                        Some(commands) => commands.recv().await,
                        None => pending().await,
                    }
                }, if query_capacity && query_rx.is_some() => {
                    let Some(command) = command else {
                        query_rx = None;
                        continue;
                    };
                    if let (Some(query), Some(pending)) = (query, query_pending.as_deref_mut())
                        && let Some(request) = query.request(id, &command)
                    {
                        if self.shard.state() != ShardState::Active
                            || self.shard.ratelimiter().is_some_and(|limiter| limiter.available() == 0)
                            || self.shard.sender().command(request).is_err()
                        {
                            query.send_failed(id, &command);
                        } else {
                            *pending += 1;
                        }
                    }
                }
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
                    let Some(voice) = voice else {
                        request.reply.fail(VoiceGatewayError::Closed);
                        continue;
                    };
                    let current = voice.generation.load(Ordering::Acquire);
                    if self.shard.state() != ShardState::Active || current != request.generation {
                        request.reply.fail(VoiceGatewayError::NotReady);
                    } else if let Ok(mut managed) = voice.managed.lock() {
                        managed.request(request, |update| self.shard.command(update));
                    } else {
                        request.reply.fail(VoiceGatewayError::Closed);
                    }
                }
                () = async {
                    match (voice, deadline) {
                        (Some(voice), Some(deadline)) => tokio::select! {
                            () = voice.cancellation.notified() => {},
                            () = sleep_until(deadline) => {},
                        },
                        (Some(voice), None) => voice.cancellation.notified().await,
                        (None, _) => pending().await,
                    }
                }, if voice.is_some() => {
                    if let Some(voice) = voice
                        && let Ok(mut managed) = voice.managed.lock()
                    {
                        managed.expire();
                    }
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
        if self.voice.is_some() {
            event_types |= EventTypeFlags::VOICE_STATE_UPDATE | EventTypeFlags::VOICE_SERVER_UPDATE;
        }
        if self.member_query.is_some() {
            event_types |= EventTypeFlags::MEMBER_CHUNK
                | EventTypeFlags::GATEWAY_HEARTBEAT_ACK
                | EventTypeFlags::RATE_LIMITED;
        }
        while let Some(event) = self
            .next_event(event_types, decode, invalidate_permissions)
            .await
        {
            let is_ready = matches!(&event, Ok(IncomingEvent::Standard(Event::Ready(_))));
            let shard = self.id();
            let event = if let Some(query) = &mut self.member_query {
                let transport_progress = self.shard.state() == ShardState::Active
                    && self
                        .shard
                        .ratelimiter()
                        .is_none_or(|limiter| limiter.available() > 0);
                let Some(event) = query.observe(shard, event, transport_progress) else {
                    continue;
                };
                event
            } else {
                event
            };
            if let Some(voice) = &self.voice
                && let Ok(IncomingEvent::Standard(event)) = &event
            {
                if let Ok(mut managed) = voice.managed.lock() {
                    managed.observe(event, self.own_id);
                }
                if matches!(event, Event::Ready(_) | Event::Resumed) {
                    voice.set_ready(true);
                }
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
