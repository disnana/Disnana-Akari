use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
    num::NonZeroUsize,
    sync::{Arc, RwLock},
};

use twilight_gateway::{Event, EventTypeFlags, Intents, ShardId};
use twilight_model::{
    gateway::payload::incoming::{GuildCreate, MemberUpdate},
    guild::Member,
    id::{
        Id,
        marker::{GuildMarker, RoleMarker, UserMarker},
    },
};

use crate::{Error, Events, Result, message_events::MemberUpdateFields};

mod bootstrap;
pub(crate) use bootstrap::{BootstrapConfig, BootstrapWorker};

/// 保存済みのサーバーメンバーが更新されたときのスナップショットです。
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CachedMemberUpdate {
    /// メンバーが所属するサーバー。
    pub guild_id: Id<GuildMarker>,
    /// 更新直前の保存状態。
    pub before: Arc<Member>,
    /// 更新で受信した項目を反映した状態。
    pub after: Arc<Member>,
}

/// 保存済みのサーバーメンバーが退出したときのスナップショットです。
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CachedMemberRemove {
    /// メンバーが所属していたサーバー。
    pub guild_id: Id<GuildMarker>,
    /// 退出直前に保存されていたメンバーの状態。
    pub member: Arc<Member>,
}

/// 保存済みのサーバーメンバーを参照する、読み取り専用ハンドルです。
///
/// [`crate::Bot::with_member_cache`] で取得します。保存対象は受信済みの一部で、
/// 容量超過、未受信、退出などで見つからないことがあります。読み取り時に Discord へ問い合わせません。
/// Gateway の再接続中は、そのシャードの項目を一時的に取得できません。
#[derive(Clone)]
pub struct MemberCacheHandle(Arc<RwLock<Option<MemberCache>>>);

impl std::fmt::Debug for MemberCacheHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemberCacheHandle").finish_non_exhaustive()
    }
}

impl MemberCacheHandle {
    pub(crate) fn new() -> Self {
        Self(Arc::default())
    }

    /// 指定したメンバーの最新の保存スナップショットを返します。
    ///
    /// 取得したスナップショットは、後からキャッシュが更新されても変化しません。
    pub fn get(&self, guild: Id<GuildMarker>, user: Id<UserMarker>) -> Option<Arc<Member>> {
        self.read(|cache| {
            let entry = cache.entries.get(&guild)?.get(&user)?;
            (!cache.suspended.contains(&entry.shard)).then(|| Arc::clone(&entry.member))
        })?
    }

    /// 指定したサーバーの、現在参照できる保存済みメンバーを返します。順序は任意です。
    pub fn guild_members(&self, guild: Id<GuildMarker>) -> Vec<Arc<Member>> {
        self.read(|cache| {
            cache.entries.get(&guild).map_or_else(Vec::new, |members| {
                members
                    .values()
                    .filter(|entry| !cache.suspended.contains(&entry.shard))
                    .map(|entry| Arc::clone(&entry.member))
                    .collect()
            })
        })
        .unwrap_or_default()
    }

    fn read<T>(&self, read: impl FnOnce(&MemberCache) -> T) -> Option<T> {
        let cache = self.0.read().ok()?;
        cache.as_ref().map(read)
    }
}

pub(crate) enum CachedMemberEvent {
    Update(CachedMemberUpdate),
    Remove(CachedMemberRemove),
}

pub(crate) struct MemberCacheRuntime {
    cache: MemberStore,
    events: EventTypeFlags,
    bootstrap: Option<bootstrap::Bootstrap>,
}

enum MemberStore {
    Local(RefCell<MemberCache>),
    Shared(MemberCacheHandle),
}

impl MemberStore {
    fn write<T>(&self, write: impl FnOnce(&mut MemberCache) -> T) -> Option<T> {
        match self {
            Self::Local(cache) => Some(write(&mut cache.borrow_mut())),
            Self::Shared(handle) => handle.0.write().ok()?.as_mut().map(write),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct MemberCacheRoute<'a> {
    runtime: &'a MemberCacheRuntime,
    shard: ShardId,
}

impl MemberCacheRuntime {
    pub(crate) fn prepare(
        capacity: usize,
        events: &Events,
        intents: Intents,
        handle: Option<MemberCacheHandle>,
        bootstrap: Option<BootstrapConfig>,
    ) -> Result<Option<Self>> {
        let events = events.cached_member_types();
        if events.is_empty() && handle.is_none() && bootstrap.is_none() {
            return Ok(None);
        }
        let capacity = NonZeroUsize::new(capacity).ok_or(Error::MemberCacheRequired)?;
        for intent in [Intents::GUILDS, Intents::GUILD_MEMBERS] {
            if !intents.contains(intent) {
                return Err(Error::MissingEventIntents { any_of: intent });
            }
        }
        let (handle, bootstrap) = match bootstrap {
            Some(config) => {
                let handle = handle.unwrap_or_else(MemberCacheHandle::new);
                let bootstrap = bootstrap::Bootstrap::new(config, handle.clone(), capacity);
                (Some(handle), Some(bootstrap))
            }
            None => (handle, None),
        };
        let cache = MemberCache::new(capacity);
        let cache = match handle {
            Some(handle) => {
                if let Ok(mut shared) = handle.0.write() {
                    *shared = Some(cache);
                }
                MemberStore::Shared(handle)
            }
            None => MemberStore::Local(RefCell::new(cache)),
        };
        Ok(Some(Self {
            cache,
            events,
            bootstrap,
        }))
    }

    pub(crate) fn bootstrap_worker(&self) -> Option<BootstrapWorker> {
        self.bootstrap.as_ref().map(bootstrap::Bootstrap::worker)
    }

    pub(crate) fn event_types(&self) -> EventTypeFlags {
        EventTypeFlags::GUILD_CREATE
            | EventTypeFlags::GUILD_DELETE
            | EventTypeFlags::ROLE_DELETE
            | EventTypeFlags::MEMBER_ADD
            | EventTypeFlags::MEMBER_UPDATE
            | EventTypeFlags::MEMBER_REMOVE
    }

    pub(crate) const fn shard(&self, shard: ShardId) -> MemberCacheRoute<'_> {
        MemberCacheRoute {
            runtime: self,
            shard,
        }
    }

    pub(crate) fn suspend_shard(&self, shard: ShardId) {
        if let MemberStore::Shared(handle) = &self.cache
            && let Ok(mut cache) = handle.0.write()
            && let Some(cache) = cache.as_mut()
        {
            cache.suspended.insert(shard);
            cache.bootstrap.pause_shard(shard);
        }
    }

    pub(crate) fn suspend_all(&self) {
        if let MemberStore::Shared(handle) = &self.cache
            && let Ok(mut cache) = handle.0.write()
        {
            *cache = None;
        }
    }
}

impl Drop for MemberCacheRuntime {
    fn drop(&mut self) {
        self.suspend_all();
    }
}

impl MemberCacheRoute<'_> {
    pub(crate) fn observe(self, event: &Event) -> Option<CachedMemberEvent> {
        if !matches!(event, Event::Ready(_) | Event::Resumed)
            && !self.runtime.event_types().intersects(event.kind().into())
        {
            return None;
        }
        self.runtime
            .cache
            .write(|cache| self.observe_cached(cache, event))
            .flatten()
    }

    fn observe_cached(self, cache: &mut MemberCache, event: &Event) -> Option<CachedMemberEvent> {
        match event {
            Event::Ready(_) => {
                cache.reset(self.shard);
                cache.suspended.remove(&self.shard);
            }
            Event::Resumed => {
                cache.suspended.remove(&self.shard);
                if let Some(bootstrap) = &self.runtime.bootstrap {
                    bootstrap.wake();
                }
            }
            Event::GuildCreate(guild) => {
                cache.clear_guild(self.shard, guild.id());
                if let GuildCreate::Available(guild) = guild.as_ref() {
                    for member in guild.members.iter().take(cache.capacity.get()) {
                        cache.insert(self.shard, guild.id, member);
                    }
                    if let Some(bootstrap) = &self.runtime.bootstrap {
                        bootstrap.schedule(cache, self.shard, guild);
                    }
                }
            }
            Event::GuildDelete(guild) => cache.clear_guild(self.shard, guild.id),
            Event::RoleDelete(role) => cache.remove_role(self.shard, role.guild_id, role.role_id),
            Event::MemberAdd(event) => {
                cache
                    .bootstrap
                    .touch(self.shard, event.guild_id, event.member.user.id);
                cache.insert(self.shard, event.guild_id, &event.member);
            }
            Event::MemberRemove(event) => {
                cache
                    .bootstrap
                    .touch(self.shard, event.guild_id, event.user.id);
                let member = cache.remove(self.shard, (event.guild_id, event.user.id))?;
                if self.runtime.events.contains(EventTypeFlags::MEMBER_REMOVE) {
                    return Some(CachedMemberEvent::Remove(CachedMemberRemove {
                        guild_id: event.guild_id,
                        member,
                    }));
                }
            }
            _ => {}
        }
        None
    }

    pub(crate) fn update(
        self,
        event: &MemberUpdate,
        fields: Option<MemberUpdateFields>,
    ) -> Option<CachedMemberEvent> {
        let notify = self.runtime.events.contains(EventTypeFlags::MEMBER_UPDATE);
        self.runtime
            .cache
            .write(|cache| {
                let Some(fields) = fields else {
                    cache.reset(self.shard);
                    return None;
                };
                cache
                    .update(self.shard, event, fields, notify)
                    .map(CachedMemberEvent::Update)
            })
            .flatten()
    }

    pub(crate) fn invalidate_on_decode_error(self, error: &Error) {
        if crate::message_events::lost_gateway_event(
            error,
            self.runtime.event_types() | EventTypeFlags::READY,
        ) {
            self.runtime.cache.write(|cache| cache.reset(self.shard));
        }
    }
}

#[cfg(test)]
mod tests;

type Key = (Id<GuildMarker>, Id<UserMarker>);

struct MemberCache {
    capacity: NonZeroUsize,
    entries: HashMap<Id<GuildMarker>, HashMap<Id<UserMarker>, Entry>>,
    order: BTreeMap<u64, Key>,
    next: u64,
    suspended: HashSet<ShardId>,
    bootstrap: bootstrap::Pending,
}

struct Entry {
    shard: ShardId,
    order: u64,
    member: Arc<Member>,
}

impl MemberCache {
    fn new(capacity: NonZeroUsize) -> Self {
        Self {
            capacity,
            entries: HashMap::new(),
            order: BTreeMap::new(),
            next: 0,
            suspended: HashSet::new(),
            bootstrap: bootstrap::Pending::default(),
        }
    }

    fn insert(&mut self, shard: ShardId, guild: Id<GuildMarker>, member: &Member) {
        self.insert_snapshot(shard, guild, Arc::new(member.clone()));
    }

    fn insert_snapshot(&mut self, shard: ShardId, guild: Id<GuildMarker>, member: Arc<Member>) {
        let key = (guild, member.user.id);
        if let Some(entry) = self.entry_mut(key) {
            entry.shard = shard;
            entry.member = member;
            return;
        }
        if self.order.len() == self.capacity.get()
            && let Some((_, oldest)) = self.order.pop_first()
        {
            self.remove_entry(oldest);
        }
        if self.next == u64::MAX {
            self.reindex();
        }
        let order = self.next;
        self.next += 1;
        self.order.insert(order, key);
        self.entries.entry(guild).or_default().insert(
            member.user.id,
            Entry {
                shard,
                order,
                member,
            },
        );
    }

    fn reindex(&mut self) {
        self.next = 0;
        for key in std::mem::take(&mut self.order).into_values() {
            let order = self.next;
            if let Some(entry) = self.entry_mut(key) {
                entry.order = order;
                self.order.insert(order, key);
                self.next += 1;
            }
        }
    }

    fn remove(&mut self, shard: ShardId, key: Key) -> Option<Arc<Member>> {
        if self.entries.get(&key.0)?.get(&key.1)?.shard != shard {
            return None;
        }
        let entry = self.remove_entry(key)?;
        self.order.remove(&entry.order);
        Some(entry.member)
    }

    fn entry_mut(&mut self, key: Key) -> Option<&mut Entry> {
        self.entries.get_mut(&key.0)?.get_mut(&key.1)
    }

    fn remove_entry(&mut self, (guild, user): Key) -> Option<Entry> {
        let members = self.entries.get_mut(&guild)?;
        let entry = members.remove(&user)?;
        if members.is_empty() {
            self.entries.remove(&guild);
        }
        Some(entry)
    }

    fn reset(&mut self, shard: ShardId) {
        self.bootstrap.cancel_shard(shard);
        self.clear_matching(|_, entry| entry.shard == shard);
    }

    fn clear_guild(&mut self, shard: ShardId, guild: Id<GuildMarker>) {
        self.bootstrap.cancel_guild(shard, guild);
        if let Some(members) = self.entries.get_mut(&guild) {
            members.retain(|_, entry| {
                if entry.shard == shard {
                    self.order.remove(&entry.order);
                    false
                } else {
                    true
                }
            });
            if members.is_empty() {
                self.entries.remove(&guild);
            }
        }
    }

    fn clear_matching(&mut self, matches: impl Fn(&Key, &Entry) -> bool) {
        self.entries.retain(|guild, members| {
            members.retain(|user, entry| {
                let keep = !matches(&(*guild, *user), entry);
                if !keep {
                    self.order.remove(&entry.order);
                }
                keep
            });
            !members.is_empty()
        });
    }

    fn remove_role(&mut self, shard: ShardId, guild: Id<GuildMarker>, role: Id<RoleMarker>) {
        self.bootstrap.cancel_guild(shard, guild);
        let Some(members) = self.entries.get_mut(&guild) else {
            return;
        };
        for entry in members.values_mut() {
            if entry.shard == shard && entry.member.roles.contains(&role) {
                Arc::make_mut(&mut entry.member)
                    .roles
                    .retain(|id| *id != role);
            }
        }
    }

    fn update(
        &mut self,
        shard: ShardId,
        event: &MemberUpdate,
        fields: MemberUpdateFields,
        notify: bool,
    ) -> Option<CachedMemberUpdate> {
        self.bootstrap.touch(shard, event.guild_id, event.user.id);
        let entry = self.entry_mut((event.guild_id, event.user.id))?;
        if entry.shard != shard {
            return None;
        }
        let before = notify.then(|| Arc::clone(&entry.member));
        fields.apply(event, Arc::make_mut(&mut entry.member));
        before.map(|before| CachedMemberUpdate {
            guild_id: event.guild_id,
            before,
            after: Arc::clone(&entry.member),
        })
    }
}
