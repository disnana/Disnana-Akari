use std::{
    collections::{BTreeMap, HashMap, hash_map},
    num::NonZeroUsize,
};

use twilight_gateway::{Event, EventTypeFlags};
use twilight_model::{
    gateway::{ShardId, payload::incoming::GuildCreate},
    id::{
        Id,
        marker::{ChannelMarker, GuildMarker, UserMarker},
    },
    util::Timestamp,
    voice::VoiceState,
};

/// サーバー内で観測したユーザーの音声状態です。
///
/// 音声接続用のセッション ID や、メンバーの詳細は保持しません。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct VoiceStateSnapshot {
    channel_id: Option<Id<ChannelMarker>>,
    deaf: bool,
    mute: bool,
    self_deaf: bool,
    self_mute: bool,
    self_stream: bool,
    self_video: bool,
    suppress: bool,
    request_to_speak_timestamp: Option<Timestamp>,
}

impl VoiceStateSnapshot {
    /// 接続しているボイスまたはステージチャンネルです。切断中は [None] です。
    pub const fn channel_id(&self) -> Option<Id<ChannelMarker>> {
        self.channel_id
    }

    /// サーバーによるスピーカーミュートが有効かを返します。
    pub const fn deaf(&self) -> bool {
        self.deaf
    }

    /// サーバーによるマイクミュートが有効かを返します。
    pub const fn mute(&self) -> bool {
        self.mute
    }

    /// 本人のスピーカーミュートが有効かを返します。
    pub const fn self_deaf(&self) -> bool {
        self.self_deaf
    }

    /// 本人のマイクミュートが有効かを返します。
    pub const fn self_mute(&self) -> bool {
        self.self_mute
    }

    /// 画面配信中かを返します。
    pub const fn self_stream(&self) -> bool {
        self.self_stream
    }

    /// カメラが有効かを返します。
    pub const fn self_video(&self) -> bool {
        self.self_video
    }

    /// ステージでの発言が抑止されているかを返します。
    pub const fn suppress(&self) -> bool {
        self.suppress
    }

    /// ステージで発言をリクエストした時刻です。
    pub const fn request_to_speak_timestamp(&self) -> Option<Timestamp> {
        self.request_to_speak_timestamp
    }
}

impl From<&VoiceState> for VoiceStateSnapshot {
    fn from(state: &VoiceState) -> Self {
        Self {
            channel_id: state.channel_id,
            deaf: state.deaf,
            mute: state.mute,
            self_deaf: state.self_deaf,
            self_mute: state.self_mute,
            self_stream: state.self_stream,
            self_video: state.self_video,
            suppress: state.suppress,
            request_to_speak_timestamp: state.request_to_speak_timestamp,
        }
    }
}

/// 一件の Gateway 音声状態通知と、そのメンバーの前回の状態です。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct VoiceStateTransition {
    guild_id: Id<GuildMarker>,
    user_id: Id<UserMarker>,
    shard: ShardId,
    before: Option<VoiceStateSnapshot>,
    after: VoiceStateSnapshot,
}

impl VoiceStateTransition {
    /// 音声状態が通知されたサーバーの ID です。
    pub const fn guild_id(&self) -> Id<GuildMarker> {
        self.guild_id
    }

    /// 対象メンバーのユーザー ID です。
    pub const fn user_id(&self) -> Id<UserMarker> {
        self.user_id
    }

    /// 通知を受信した Gateway シャードの ID です。
    pub const fn shard(&self) -> ShardId {
        self.shard
    }

    /// 保持していた更新前の状態です。
    ///
    /// 初回の観測、切断後、キャッシュ満杯による破棄、再同期などで不明な場合は [None] です。
    pub const fn before(&self) -> Option<VoiceStateSnapshot> {
        self.before
    }

    /// Discord が今回通知した状態です。
    ///
    /// チャンネルが [None] の場合は接続していない状態です。
    pub const fn after(&self) -> VoiceStateSnapshot {
        self.after
    }
}

pub(crate) const VOICE_STATE_CACHE_EVENTS: EventTypeFlags = EventTypeFlags::READY
    .union(EventTypeFlags::GUILD_CREATE)
    .union(EventTypeFlags::GUILD_DELETE)
    .union(EventTypeFlags::VOICE_STATE_UPDATE);

type Key = (Id<GuildMarker>, Id<UserMarker>);

#[derive(Debug)]
struct Entry {
    snapshot: VoiceStateSnapshot,
    sequence: u64,
}

#[derive(Debug)]
pub(crate) struct VoiceStateCache {
    shard: ShardId,
    capacity: NonZeroUsize,
    by_guild: HashMap<Id<GuildMarker>, HashMap<Id<UserMarker>, Entry>>,
    oldest: BTreeMap<u64, Key>,
    sequence: u64,
}

impl VoiceStateCache {
    pub(crate) fn new(shard: ShardId, capacity: NonZeroUsize) -> Self {
        Self {
            shard,
            capacity,
            by_guild: HashMap::new(),
            oldest: BTreeMap::new(),
            sequence: 0,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.by_guild.clear();
        self.oldest.clear();
        self.sequence = 0;
    }

    pub(crate) fn observe(&mut self, event: &Event) -> Option<VoiceStateTransition> {
        match event {
            Event::Ready(_) => self.reset(),
            Event::GuildCreate(create) => self.sync_guild(create),
            Event::GuildDelete(delete) => self.forget_guild(delete.id),
            Event::VoiceStateUpdate(update) => {
                let guild_id = update.guild_id?;
                let user_id = update.user_id;
                let after = VoiceStateSnapshot::from(&update.0);
                let before = self.store((guild_id, user_id), after);
                return Some(VoiceStateTransition {
                    guild_id,
                    user_id,
                    shard: self.shard,
                    before,
                    after,
                });
            }
            _ => {}
        }
        None
    }

    fn sync_guild(&mut self, create: &GuildCreate) {
        let guild_id = create.id();
        self.forget_guild(guild_id);
        if let GuildCreate::Available(guild) = create {
            for state in &guild.voice_states {
                if state.guild_id.is_none_or(|id| id == guild_id) {
                    self.store((guild_id, state.user_id), VoiceStateSnapshot::from(state));
                }
            }
        }
    }

    fn forget_guild(&mut self, guild: Id<GuildMarker>) {
        // GuildCreate は起動時に連続するので、対象サーバーの状態だけを走査する。
        if let Some(members) = self.by_guild.remove(&guild) {
            for entry in members.into_values() {
                self.oldest.remove(&entry.sequence);
            }
        }
    }

    fn remove_member(&mut self, (guild_id, user_id): Key) -> Option<Entry> {
        let hash_map::Entry::Occupied(mut guild) = self.by_guild.entry(guild_id) else {
            return None;
        };
        let entry = guild.get_mut().remove(&user_id);
        if guild.get().is_empty() {
            guild.remove();
        }
        entry
    }

    fn store(&mut self, key: Key, snapshot: VoiceStateSnapshot) -> Option<VoiceStateSnapshot> {
        let (guild_id, user_id) = key;
        let connected = snapshot.channel_id().is_some();
        let entry = if connected {
            self.by_guild
                .get_mut(&guild_id)
                .and_then(|members| members.remove(&user_id))
        } else {
            self.remove_member(key)
        };
        let before = entry.map(|entry| {
            self.oldest.remove(&entry.sequence);
            entry.snapshot
        });
        if !connected {
            return before;
        }
        if self.sequence == u64::MAX {
            self.reset();
        }
        if self.oldest.len() >= self.capacity.get()
            && let Some((_, oldest)) = self.oldest.pop_first()
        {
            let evicted = self.remove_member(oldest);
            debug_assert!(evicted.is_some());
        }
        let sequence = self.sequence;
        self.sequence += 1;
        self.oldest.insert(sequence, key);
        self.by_guild
            .entry(guild_id)
            .or_default()
            .insert(user_id, Entry { snapshot, sequence });
        before
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use twilight_model::gateway::payload::incoming::{GuildDelete, VoiceStateUpdate};

    use super::*;

    fn cache(capacity: usize) -> VoiceStateCache {
        VoiceStateCache::new(ShardId::ONE, NonZeroUsize::new(capacity).unwrap())
    }

    fn voice(guild: Option<u64>, user: u64, channel: Option<u64>) -> VoiceState {
        VoiceState {
            channel_id: channel.map(Id::new),
            guild_id: guild.map(Id::new),
            member: None,
            user_id: Id::new(user),
            deaf: false,
            mute: false,
            self_deaf: false,
            self_mute: false,
            self_stream: false,
            self_video: false,
            suppress: false,
            request_to_speak_timestamp: None,
            session_id: "secret-voice-session-fixture".into(),
        }
    }

    fn create(id: u64, voice_states: &[VoiceState]) -> Event {
        let payload = json!({
            "id":id.to_string(), "owner_id":"2", "name":"fixture", "afk_timeout":60,
            "default_message_notifications":0, "explicit_content_filter":0, "features":[],
            "mfa_level":0, "nsfw_level":0, "preferred_locale":"ja", "roles":[],
            "system_channel_flags":0, "premium_progress_bar_enabled":false,
            "verification_level":0, "voice_states":voice_states
        });
        Event::GuildCreate(Box::new(serde_json::from_value(payload).unwrap()))
    }

    fn unavailable(id: u64) -> Event {
        Event::GuildCreate(Box::new(
            serde_json::from_value(json!({
                "id":id.to_string(), "unavailable":true
            }))
            .unwrap(),
        ))
    }

    fn ready() -> Event {
        Event::Ready(serde_json::from_value(json!({
            "application":{"flags":0,"id":"2"}, "guilds":[], "session_id":"session",
            "resume_gateway_url":"wss://gateway.discord.gg", "v":10,
            "user":{"id":"2","username":"akari","discriminator":"0","bot":true,"mfa_enabled":false}
        })).unwrap())
    }

    fn update(cache: &mut VoiceStateCache, state: VoiceState) -> Option<VoiceStateTransition> {
        cache.observe(&Event::VoiceStateUpdate(Box::new(VoiceStateUpdate(state))))
    }

    #[test]
    fn guild_create_seeds_the_before_snapshot_and_excludes_foreign_state() {
        let mut cache = cache(4);
        let mut initial = voice(None, 3, Some(10));
        initial.mute = true;
        let Event::GuildCreate(mut payload) = create(1, &[initial]) else {
            panic!("expected a GuildCreate event");
        };
        let GuildCreate::Available(guild) = payload.as_mut() else {
            panic!("expected an available guild");
        };
        // Twilight はデコード時に親サーバーの ID を設定するので、異常値はデコード後に作る。
        guild.voice_states.push(voice(Some(2), 4, Some(11)));
        assert!(cache.observe(&Event::GuildCreate(payload)).is_none());

        let mut next = voice(Some(1), 3, Some(10));
        next.deaf = true;
        next.self_deaf = true;
        next.self_mute = true;
        next.self_stream = true;
        next.self_video = true;
        next.suppress = true;
        next.request_to_speak_timestamp = Some(Timestamp::from_secs(1_632_072_645).unwrap());
        let event = update(&mut cache, next).unwrap();
        assert_eq!(event.guild_id(), Id::new(1));
        assert_eq!(event.user_id(), Id::new(3));
        assert_eq!(event.shard(), ShardId::ONE);
        assert_eq!(event.before().unwrap().channel_id(), Some(Id::new(10)));
        assert!(event.before().unwrap().mute());
        assert!(!event.after().mute());
        assert!(event.after().deaf());
        assert!(event.after().self_deaf());
        assert!(event.after().self_mute());
        assert!(event.after().self_stream());
        assert!(event.after().self_video());
        assert!(event.after().suppress());
        assert_eq!(
            event.after().request_to_speak_timestamp(),
            Timestamp::from_secs(1_632_072_645).ok()
        );
        assert!(!format!("{event:?}").contains("secret-voice-session-fixture"));
        assert!(
            update(&mut cache, voice(Some(1), 4, Some(11)))
                .unwrap()
                .before()
                .is_none()
        );
    }

    #[test]
    fn moves_and_both_known_and_unknown_disconnects_are_represented() {
        let mut cache = cache(2);
        let first = update(&mut cache, voice(Some(1), 3, Some(10))).unwrap();
        assert!(first.before().is_none());
        let moved = update(&mut cache, voice(Some(1), 3, Some(11))).unwrap();
        assert_eq!(moved.before().unwrap().channel_id(), Some(Id::new(10)));
        assert_eq!(moved.after().channel_id(), Some(Id::new(11)));
        let disconnected = update(&mut cache, voice(Some(1), 3, None)).unwrap();
        assert_eq!(
            disconnected.before().unwrap().channel_id(),
            Some(Id::new(11))
        );
        assert!(disconnected.after().channel_id().is_none());
        assert!(
            update(&mut cache, voice(Some(1), 3, None))
                .unwrap()
                .before()
                .is_none()
        );
        assert!(update(&mut cache, voice(None, 3, Some(10))).is_none());
        assert!(cache.by_guild.is_empty());
        assert!(cache.oldest.is_empty());
    }

    #[test]
    fn most_recent_updates_are_retained_without_growing_the_index() {
        let mut cache = cache(2);
        update(&mut cache, voice(Some(1), 3, Some(10))).unwrap();
        update(&mut cache, voice(Some(1), 4, Some(10))).unwrap();
        for _ in 0..10 {
            assert!(
                update(&mut cache, voice(Some(1), 3, Some(10)))
                    .unwrap()
                    .before()
                    .is_some()
            );
        }
        update(&mut cache, voice(Some(1), 5, Some(10))).unwrap();
        assert_eq!(cache.by_guild[&Id::new(1)].len(), 2);
        assert_eq!(cache.oldest.len(), 2);
        assert!(
            update(&mut cache, voice(Some(1), 3, Some(10)))
                .unwrap()
                .before()
                .is_some()
        );
        assert!(
            update(&mut cache, voice(Some(1), 4, Some(10)))
                .unwrap()
                .before()
                .is_none()
        );
    }

    #[test]
    fn guild_resync_releases_capacity_and_eviction_removes_empty_guilds() {
        let mut cache = cache(3);
        update(&mut cache, voice(Some(1), 3, Some(10))).unwrap();
        update(&mut cache, voice(Some(1), 4, Some(10))).unwrap();
        update(&mut cache, voice(Some(2), 3, Some(20))).unwrap();

        assert!(
            cache
                .observe(&create(1, &[voice(None, 3, Some(11))]))
                .is_none()
        );
        assert_eq!(cache.oldest.len(), 2);
        assert_eq!(cache.by_guild[&Id::new(1)].len(), 1);
        assert_eq!(cache.by_guild[&Id::new(2)].len(), 1);
        update(&mut cache, voice(Some(3), 3, Some(30))).unwrap();

        // The oldest state belongs to guild 2 even though guild 1 was added first.
        update(&mut cache, voice(Some(3), 4, Some(30))).unwrap();
        assert!(!cache.by_guild.contains_key(&Id::new(2)));
        assert_eq!(cache.oldest.len(), 3);
        assert_eq!(
            update(&mut cache, voice(Some(1), 3, Some(12)))
                .unwrap()
                .before()
                .unwrap()
                .channel_id(),
            Some(Id::new(11))
        );

        assert!(
            cache
                .observe(&Event::GuildDelete(GuildDelete {
                    id: Id::new(3),
                    unavailable: None,
                }))
                .is_none()
        );
        assert_eq!(cache.oldest.len(), 1);
        assert_eq!(cache.by_guild.len(), 1);
        assert!(
            update(&mut cache, voice(Some(3), 4, Some(30)))
                .unwrap()
                .before()
                .is_none()
        );
    }

    #[test]
    fn guild_departure_and_resync_are_local_but_ready_and_malformed_reset_the_shard() {
        let mut first = VoiceStateCache::new(ShardId::new(0, 2), NonZeroUsize::new(4).unwrap());
        let mut second = VoiceStateCache::new(ShardId::new(1, 2), NonZeroUsize::new(4).unwrap());
        update(&mut first, voice(Some(1), 3, Some(10))).unwrap();
        update(&mut first, voice(Some(2), 3, Some(20))).unwrap();
        update(&mut second, voice(Some(3), 3, Some(30))).unwrap();
        assert!(first.observe(&Event::Resumed).is_none());
        assert!(
            update(&mut first, voice(Some(1), 3, Some(10)))
                .unwrap()
                .before()
                .is_some()
        );
        assert!(first.observe(&unavailable(1)).is_none());
        assert!(
            update(&mut first, voice(Some(1), 3, Some(11)))
                .unwrap()
                .before()
                .is_none()
        );

        assert!(
            first
                .observe(&create(1, &[voice(None, 3, Some(12))]))
                .is_none()
        );
        assert_eq!(
            update(&mut first, voice(Some(1), 3, Some(13)))
                .unwrap()
                .before()
                .unwrap()
                .channel_id(),
            Some(Id::new(12))
        );
        assert!(
            first
                .observe(&Event::GuildDelete(GuildDelete {
                    id: Id::new(1),
                    unavailable: Some(true)
                }))
                .is_none()
        );
        assert!(
            update(&mut first, voice(Some(1), 3, Some(10)))
                .unwrap()
                .before()
                .is_none()
        );
        assert!(
            update(&mut first, voice(Some(2), 3, Some(20)))
                .unwrap()
                .before()
                .is_some()
        );

        assert!(first.observe(&ready()).is_none());
        assert!(
            update(&mut first, voice(Some(2), 3, Some(20)))
                .unwrap()
                .before()
                .is_none()
        );
        first.reset();
        assert!(
            update(&mut first, voice(Some(2), 3, Some(20)))
                .unwrap()
                .before()
                .is_none()
        );
        let independent = update(&mut second, voice(Some(3), 3, Some(30))).unwrap();
        assert!(independent.before().is_some());
        assert_eq!(independent.shard(), ShardId::new(1, 2));
    }
}
