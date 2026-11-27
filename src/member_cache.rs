use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    num::NonZeroUsize,
    sync::Arc,
};

use twilight_gateway::{Event, EventTypeFlags, Intents, ShardId, error::ReceiveMessageErrorType};
use twilight_model::{
    gateway::{
        event::GatewayEventDeserializer,
        payload::incoming::{GuildCreate, MemberUpdate},
    },
    guild::Member,
    id::{
        Id,
        marker::{GuildMarker, RoleMarker, UserMarker},
    },
};

use crate::{Error, Events, Result, message_events::MemberUpdateFields};

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

pub(crate) enum CachedMemberEvent {
    Update(CachedMemberUpdate),
    Remove(CachedMemberRemove),
}

pub(crate) struct MemberCacheRuntime {
    cache: RefCell<MemberCache>,
    events: EventTypeFlags,
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
    ) -> Result<Option<Self>> {
        let events = events.cached_member_types();
        if events.is_empty() {
            return Ok(None);
        }
        let capacity = NonZeroUsize::new(capacity).ok_or(Error::MemberCacheRequired)?;
        for intent in [Intents::GUILDS, Intents::GUILD_MEMBERS] {
            if !intents.contains(intent) {
                return Err(Error::MissingEventIntents { any_of: intent });
            }
        }
        Ok(Some(Self {
            cache: RefCell::new(MemberCache::new(capacity)),
            events,
        }))
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
}

impl MemberCacheRoute<'_> {
    pub(crate) fn observe(self, event: &Event) -> Option<CachedMemberEvent> {
        if !self.runtime.event_types().intersects(event.kind().into())
            && !matches!(event, Event::Ready(_))
        {
            return None;
        }
        let mut cache = self.runtime.cache.borrow_mut();
        match event {
            Event::Ready(_) => cache.reset(self.shard),
            Event::GuildCreate(guild) => {
                cache.clear_guild(self.shard, guild.id());
                if let GuildCreate::Available(guild) = guild.as_ref() {
                    for member in guild.members.iter().take(cache.capacity.get()) {
                        cache.insert(self.shard, guild.id, member);
                    }
                }
            }
            Event::GuildDelete(guild) => cache.clear_guild(self.shard, guild.id),
            Event::RoleDelete(role) => cache.remove_role(self.shard, role.guild_id, role.role_id),
            Event::MemberAdd(event) => cache.insert(self.shard, event.guild_id, &event.member),
            Event::MemberRemove(event) => {
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
        let mut cache = self.runtime.cache.borrow_mut();
        let Some(fields) = fields else {
            cache.reset(self.shard);
            return None;
        };
        let notify = self.runtime.events.contains(EventTypeFlags::MEMBER_UPDATE);
        cache
            .update(self.shard, event, fields, notify)
            .map(CachedMemberEvent::Update)
    }

    pub(crate) fn invalidate_on_decode_error(self, error: &Error) {
        if let Error::GatewayReceive(error) = error
            && let ReceiveMessageErrorType::Deserializing { event } = error.kind()
            && GatewayEventDeserializer::from_json(event).is_none_or(|header| {
                matches!(
                    header.event_type(),
                    Some(
                        "READY"
                            | "GUILD_CREATE"
                            | "GUILD_DELETE"
                            | "GUILD_ROLE_DELETE"
                            | "GUILD_MEMBER_ADD"
                            | "GUILD_MEMBER_UPDATE"
                            | "GUILD_MEMBER_REMOVE"
                    )
                )
            })
        {
            self.runtime.cache.borrow_mut().reset(self.shard);
        }
    }
}

#[cfg(test)]
mod tests;

type Key = (Id<GuildMarker>, Id<UserMarker>);

struct MemberCache {
    capacity: NonZeroUsize,
    entries: HashMap<Key, Entry>,
    order: BTreeMap<u64, Key>,
    next: u64,
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
        }
    }

    fn insert(&mut self, shard: ShardId, guild: Id<GuildMarker>, member: &Member) {
        let key = (guild, member.user.id);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.shard = shard;
            entry.member = Arc::new(member.clone());
            return;
        }
        if self.entries.len() == self.capacity.get()
            && let Some((_, oldest)) = self.order.pop_first()
        {
            self.entries.remove(&oldest);
        }
        if self.next == u64::MAX {
            self.reindex();
        }
        let order = self.next;
        self.next += 1;
        self.order.insert(order, key);
        self.entries.insert(
            key,
            Entry {
                shard,
                order,
                member: Arc::new(member.clone()),
            },
        );
    }

    fn reindex(&mut self) {
        self.next = 0;
        for key in std::mem::take(&mut self.order).into_values() {
            if let Some(entry) = self.entries.get_mut(&key) {
                entry.order = self.next;
                self.order.insert(self.next, key);
                self.next += 1;
            }
        }
    }

    fn remove(&mut self, shard: ShardId, key: Key) -> Option<Arc<Member>> {
        if self.entries.get(&key)?.shard != shard {
            return None;
        }
        let entry = self.entries.remove(&key)?;
        self.order.remove(&entry.order);
        Some(entry.member)
    }

    fn reset(&mut self, shard: ShardId) {
        self.clear_matching(|_, entry| entry.shard == shard);
    }

    fn clear_guild(&mut self, shard: ShardId, guild: Id<GuildMarker>) {
        self.clear_matching(|key, entry| key.0 == guild && entry.shard == shard);
    }

    fn clear_matching(&mut self, matches: impl Fn(&Key, &Entry) -> bool) {
        self.entries.retain(|key, entry| {
            let keep = !matches(key, entry);
            if !keep {
                self.order.remove(&entry.order);
            }
            keep
        });
    }

    fn remove_role(&mut self, shard: ShardId, guild: Id<GuildMarker>, role: Id<RoleMarker>) {
        for ((entry_guild, _), entry) in &mut self.entries {
            if *entry_guild == guild && entry.shard == shard && entry.member.roles.contains(&role) {
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
        let entry = self.entries.get_mut(&(event.guild_id, event.user.id))?;
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
