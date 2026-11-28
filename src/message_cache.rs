use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    num::NonZeroUsize,
    sync::Arc,
};

use twilight_gateway::{EventTypeFlags, Intents, ShardId};
use twilight_model::{
    channel::Message,
    id::{
        Id,
        marker::{ChannelMarker, GuildMarker, MessageMarker},
    },
};

use crate::{
    Error, Events, MessageDelete, MessageDeleteBulk, MessageUpdate, Result, check_lifecycle_intents,
};

/// Akari が保持したメッセージの、更新前と更新後のスナップショットです。
///
/// 更新ペイロードに含まれる本文、添付ファイル、Embed、コンポーネント、編集時刻、
/// フラグ、ピン、メンション、通話、TTS、投票を反映します。
/// その他の項目は最後に受信した新着メッセージの値です。
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CachedMessageEdit {
    /// 更新直前の、最後に受信した状態。
    pub before: Arc<Message>,
    /// 更新通知に含まれる項目を反映した状態。
    pub after: Arc<Message>,
}

struct MessageCache {
    capacity: NonZeroUsize,
    entries: BTreeMap<Id<MessageMarker>, Entry>,
}

pub(crate) struct MessageCacheRuntime {
    cache: RefCell<MessageCache>,
    channels: Option<BTreeSet<Id<ChannelMarker>>>,
    cached: EventTypeFlags,
    raw: EventTypeFlags,
    incoming: EventTypeFlags,
}

#[derive(Clone, Copy)]
pub(crate) struct MessageCacheRoute<'a> {
    runtime: &'a MessageCacheRuntime,
    shard: ShardId,
}

impl MessageCacheRuntime {
    pub(crate) fn prepare(
        capacity: usize,
        events: &Events,
        intents: Intents,
    ) -> Result<Option<Self>> {
        let cached = events.cached_message_types();
        if cached.is_empty() {
            return Ok(None);
        }
        let capacity = NonZeroUsize::new(capacity).ok_or(Error::MessageCacheRequired)?;
        check_lifecycle_intents(intents, cached)?;
        let mut incoming = EventTypeFlags::MESSAGE_CREATE
            | EventTypeFlags::MESSAGE_UPDATE
            | EventTypeFlags::MESSAGE_DELETE;
        if intents.contains(Intents::GUILD_MESSAGES) {
            incoming |= EventTypeFlags::MESSAGE_DELETE_BULK;
        }
        Ok(Some(Self {
            cache: RefCell::new(MessageCache::new(capacity)),
            channels: None,
            cached,
            raw: events.message_lifecycle_types(),
            incoming,
        }))
    }

    pub(crate) fn with_channels(mut self, channels: Option<BTreeSet<Id<ChannelMarker>>>) -> Self {
        self.channels = channels;
        self
    }

    pub(crate) const fn event_types(&self) -> EventTypeFlags {
        self.incoming
    }

    pub(crate) const fn shard(&self, shard: ShardId) -> MessageCacheRoute<'_> {
        MessageCacheRoute {
            runtime: self,
            shard,
        }
    }
}

impl MessageCacheRoute<'_> {
    pub(crate) fn includes(&self, channel: Id<ChannelMarker>) -> bool {
        self.runtime
            .channels
            .as_ref()
            .is_none_or(|channels| channels.contains(&channel))
    }

    pub(crate) fn store(&self, message: &Arc<Message>) {
        if self.includes(message.channel_id) {
            self.runtime
                .cache
                .borrow_mut()
                .store(self.shard, Arc::clone(message));
        }
    }

    pub(crate) fn reset(&self) {
        self.runtime.cache.borrow_mut().start_session(self.shard);
    }

    pub(crate) fn has_raw(&self, kind: EventTypeFlags) -> bool {
        self.runtime.raw.contains(kind)
    }

    pub(crate) fn edit(&self, update: &MessageUpdate) -> Option<CachedMessageEdit> {
        let notify = self.runtime.cached.contains(EventTypeFlags::MESSAGE_UPDATE);
        self.runtime
            .cache
            .borrow_mut()
            .edit(self.shard, update, notify)
    }

    pub(crate) fn delete(&self, delete: &MessageDelete) -> Option<Arc<Message>> {
        let removed = self.runtime.cache.borrow_mut().delete(self.shard, delete);
        removed.filter(|_| self.runtime.cached.contains(EventTypeFlags::MESSAGE_DELETE))
    }

    pub(crate) fn bulk_delete(&self, delete: &MessageDeleteBulk) -> Vec<Arc<Message>> {
        let notify = self
            .runtime
            .cached
            .contains(EventTypeFlags::MESSAGE_DELETE_BULK);
        self.runtime
            .cache
            .borrow_mut()
            .bulk_delete(self.shard, delete, notify)
    }
}

pub(crate) fn invalidates_message_history(error: &Error) -> bool {
    matches!(error, Error::MessageUpdateDecode(_))
        || crate::message_events::lost_gateway_event(
            error,
            EventTypeFlags::READY
                | EventTypeFlags::MESSAGE_CREATE
                | EventTypeFlags::MESSAGE_UPDATE
                | EventTypeFlags::MESSAGE_DELETE
                | EventTypeFlags::MESSAGE_DELETE_BULK,
        )
}

struct Entry {
    shard: ShardId,
    message: Arc<Message>,
}

impl Entry {
    fn matches(
        &self,
        shard: ShardId,
        channel: Id<ChannelMarker>,
        guild: Option<Id<GuildMarker>>,
    ) -> bool {
        self.shard == shard
            && self.message.channel_id == channel
            && guild.is_none_or(|guild| self.message.guild_id == Some(guild))
    }
}

impl MessageCache {
    pub(crate) fn new(capacity: NonZeroUsize) -> Self {
        Self {
            capacity,
            entries: BTreeMap::new(),
        }
    }

    pub(crate) fn store(&mut self, shard: ShardId, message: Arc<Message>) {
        self.entries.insert(message.id, Entry { shard, message });
        if self.entries.len() > self.capacity.get() {
            self.entries.pop_first();
        }
    }

    pub(crate) fn start_session(&mut self, shard: ShardId) {
        self.entries.retain(|_, entry| entry.shard != shard);
    }

    pub(crate) fn edit(
        &mut self,
        shard: ShardId,
        update: &MessageUpdate,
        notify: bool,
    ) -> Option<CachedMessageEdit> {
        let entry = self.entries.get_mut(&update.id)?;
        if !entry.matches(shard, update.channel_id, update.guild_id)
            || update
                .author_id
                .is_some_and(|author| entry.message.author.id != author)
        {
            return None;
        }

        let before = notify.then(|| Arc::clone(&entry.message));
        update.apply_to(Arc::make_mut(&mut entry.message));
        before.map(|before| CachedMessageEdit {
            before,
            after: Arc::clone(&entry.message),
        })
    }

    pub(crate) fn delete(
        &mut self,
        shard: ShardId,
        delete: &MessageDelete,
    ) -> Option<Arc<Message>> {
        self.remove(shard, delete.channel_id, delete.guild_id, delete.id)
    }

    pub(crate) fn bulk_delete(
        &mut self,
        shard: ShardId,
        delete: &MessageDeleteBulk,
        notify: bool,
    ) -> Vec<Arc<Message>> {
        let mut removed = Vec::new();
        for &id in &delete.ids {
            if let Some(message) = self.remove(shard, delete.channel_id, delete.guild_id, id)
                && notify
            {
                removed.push(message);
            }
        }
        removed
    }

    fn remove(
        &mut self,
        shard: ShardId,
        channel: Id<ChannelMarker>,
        guild: Option<Id<GuildMarker>>,
        id: Id<MessageMarker>,
    ) -> Option<Arc<Message>> {
        if !self.entries.get(&id)?.matches(shard, channel, guild) {
            return None;
        }
        self.entries.remove(&id).map(|entry| entry.message)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;
    use tokio::sync::mpsc;
    use twilight_gateway::Event;
    use twilight_http::Client;

    use super::*;
    use crate::{
        NoMessageHandler, dispatch_events, hooks::Hooks, message_events::IncomingEvent,
        permissions::PermissionResolver, route_gateway_event,
    };

    fn message(id: u64) -> Arc<Message> {
        let model = serde_json::from_value(json!({
            "id": id.to_string(), "channel_id": "2", "guild_id": "3", "type": 0,
            "author": {"id": "4", "username": "member", "discriminator": "0"},
            "content": format!("message {id}"), "timestamp": "2026-01-01T00:00:00.000000+00:00",
            "edited_timestamp": "2026-01-01T00:01:00.000000+00:00", "flags": 4,
            "attachments": [], "embeds": [], "mention_everyone": true, "mention_roles": ["7"],
            "mentions": [], "pinned": false, "tts": false
        }))
        .unwrap();
        Arc::new(model)
    }

    fn deletion(id: u64) -> MessageDelete {
        MessageDelete {
            channel_id: Id::new(2),
            guild_id: Some(Id::new(3)),
            id: Id::new(id),
        }
    }

    fn edited(id: u64, content: &str) -> IncomingEvent {
        IncomingEvent::MessageUpdate(
            serde_json::from_value(json!({
                "id": id.to_string(), "channel_id": "2", "content": content
            }))
            .unwrap(),
        )
    }

    #[test]
    fn subscribes_to_maintenance_events_without_requesting_message_content() {
        let mut events = Events::new();
        assert!(
            MessageCacheRuntime::prepare(8, &events, Intents::empty())
                .unwrap()
                .is_none()
        );
        events.on_message_edit(async |_: &CachedMessageEdit| -> Result<()> { Ok(()) });
        assert!(matches!(
            MessageCacheRuntime::prepare(0, &events, Intents::GUILD_MESSAGES),
            Err(Error::MessageCacheRequired)
        ));
        let dm = MessageCacheRuntime::prepare(8, &events, Intents::DIRECT_MESSAGES)
            .unwrap()
            .unwrap();
        assert_eq!(
            dm.event_types(),
            EventTypeFlags::MESSAGE_CREATE
                | EventTypeFlags::MESSAGE_UPDATE
                | EventTypeFlags::MESSAGE_DELETE
        );
        events.on_bulk_message_delete(async |_: &[Arc<Message>]| -> Result<()> { Ok(()) });
        assert!(matches!(
            MessageCacheRuntime::prepare(8, &events, Intents::DIRECT_MESSAGES),
            Err(Error::MissingEventIntents { any_of }) if any_of == Intents::GUILD_MESSAGES
        ));
    }

    #[test]
    fn bounded_cache_evicts_old_ids_and_ready_clears_only_its_shard() {
        let first = ShardId::new(0, 2);
        let second = ShardId::new(1, 2);
        let mut cache = MessageCache::new(NonZeroUsize::new(2).unwrap());
        cache.store(first, message(6));
        cache.store(first, message(5));
        cache.store(second, message(7));
        assert!(cache.delete(first, &deletion(5)).is_none());
        assert!(cache.delete(second, &deletion(6)).is_none());
        cache.start_session(first);
        assert!(cache.delete(first, &deletion(6)).is_none());
        assert_eq!(cache.delete(second, &deletion(7)).unwrap().id, Id::new(7));
    }

    #[test]
    fn edit_merges_partial_lists_and_distinguishes_nullable_fields() {
        let shard = ShardId::ONE;
        let original = message(11);
        let mut cache = MessageCache::new(NonZeroUsize::new(1).unwrap());
        cache.store(shard, Arc::clone(&original));
        let update = serde_json::from_str::<MessageUpdate>(
            r#"{
            "id":"11","channel_id":"2","edited_timestamp":null,
            "embeds":[{"type":"rich","description":"preview"}],
            "pinned":true,"mention_everyone":false,"mention_roles":[]
        }"#,
        )
        .unwrap();
        let first = cache.edit(shard, &update, true).unwrap();
        assert!(Arc::ptr_eq(&first.before, &original));
        assert!(!first.before.pinned);
        assert!(first.before.edited_timestamp.is_some());
        assert_eq!(first.after.edited_timestamp, None);
        assert_eq!(first.after.flags, original.flags);
        assert_eq!(first.after.content, original.content);
        assert_eq!(first.after.embeds.len(), 1);
        assert!(first.after.pinned);
        assert!(!first.after.mention_everyone);
        assert!(first.after.mention_roles.is_empty());

        let update = serde_json::from_str::<MessageUpdate>(
            r#"{"id":"11","channel_id":"2","content":"","embeds":[],"flags":null}"#,
        )
        .unwrap();
        let second = cache.edit(shard, &update, true).unwrap();
        assert!(Arc::ptr_eq(&second.before, &first.after));
        assert_eq!(second.after.flags, None);
        assert!(second.after.embeds.is_empty());
        assert!(second.after.content.is_empty());
        assert_eq!(cache.delete(shard, &deletion(11)).unwrap().id, Id::new(11));
        assert!(cache.delete(shard, &deletion(11)).is_none());
    }

    #[test]
    fn undecodable_update_invalidates_only_its_source_shard() {
        let mut events = Events::new();
        events.on_message_delete(async |_: &Message| -> Result<()> { Ok(()) });
        let runtime = MessageCacheRuntime::prepare(2, &events, Intents::GUILD_MESSAGES)
            .unwrap()
            .unwrap();
        let first = ShardId::new(0, 2);
        let second = ShardId::new(1, 2);
        runtime.shard(first).store(&message(8));
        runtime.shard(second).store(&message(9));

        let error =
            Error::MessageUpdateDecode(serde_json::from_str::<MessageUpdate>("{}").unwrap_err());
        let permissions = PermissionResolver::new(None);
        let (tx, mut rx) = mpsc::channel(1);
        assert!(
            route_gateway_event(
                Err(error),
                &events,
                &permissions,
                &tx,
                &Hooks::default().reporter,
                first,
                (Some(runtime.shard(first)), None, None),
            )
            .is_none()
        );
        assert!(rx.try_recv().is_err());
        assert!(runtime.shard(first).delete(&deletion(8)).is_none());
        assert_eq!(
            runtime.shard(second).delete(&deletion(9)).unwrap().id,
            Id::new(9)
        );
    }

    #[tokio::test]
    async fn cached_lifecycle_events_share_queue_order_with_raw_events() {
        let mut events = Events::new();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let raw = Arc::clone(&observed);
        let edits = Arc::clone(&observed);
        let deletes = Arc::clone(&observed);
        let bulk = Arc::clone(&observed);
        events.on_raw_message_edit(async move |update: &MessageUpdate| -> Result<()> {
            raw.lock().unwrap().push(format!("raw {}", update.id));
            Ok(())
        });
        events.on_message_edit(async move |edit: &CachedMessageEdit| -> Result<()> {
            edits
                .lock()
                .unwrap()
                .push(format!("{} -> {}", edit.before.content, edit.after.content));
            Ok(())
        });
        events.on_message_delete(async move |message: &Message| -> Result<()> {
            deletes
                .lock()
                .unwrap()
                .push(format!("deleted {}", message.content));
            Ok(())
        });
        events.on_bulk_message_delete(async move |messages: &[Arc<Message>]| -> Result<()> {
            assert_eq!(
                messages.iter().map(|m| m.id.get()).collect::<Vec<_>>(),
                [6, 5]
            );
            bulk.lock().unwrap().push("bulk".into());
            Ok(())
        });
        let runtime = MessageCacheRuntime::prepare(4, &events, Intents::GUILD_MESSAGES)
            .unwrap()
            .unwrap()
            .with_channels(Some([Id::new(2)].into()));
        let shard = ShardId::ONE;
        let cache = runtime.shard(shard);
        for id in 4..=6 {
            cache.store(&message(id));
        }
        let mut excluded_message = message(99);
        Arc::make_mut(&mut excluded_message).channel_id = Id::new(10);
        cache.store(&excluded_message);
        let excluded_edit = serde_json::from_value(json!({
            "id": "99", "channel_id": "10", "content": "outside scope"
        }))
        .unwrap();
        let payloads = [
            IncomingEvent::MessageUpdate(excluded_edit),
            edited(4, "edited"),
            IncomingEvent::Standard(Event::MessageDelete(deletion(4))),
            IncomingEvent::Standard(Event::MessageDelete(deletion(4))),
            IncomingEvent::Standard(Event::MessageDeleteBulk(MessageDeleteBulk {
                channel_id: Id::new(2),
                guild_id: Some(Id::new(3)),
                ids: [6, 99, 5, 6].into_iter().map(Id::new).collect(),
            })),
        ];
        let (tx, rx) = mpsc::channel(4);
        let hooks = Hooks::default();
        let permissions = PermissionResolver::new(None);
        for payload in payloads {
            assert!(
                route_gateway_event(
                    Ok(payload),
                    &events,
                    &permissions,
                    &tx,
                    &hooks.reporter,
                    shard,
                    (Some(cache), None, None),
                )
                .is_none()
            );
        }
        drop(tx);
        dispatch_events(
            rx,
            None::<NoMessageHandler>,
            &events,
            &Client::new("test".into()),
            &permissions,
            &hooks,
        )
        .await;
        assert_eq!(
            *observed.lock().unwrap(),
            [
                "raw 99",
                "raw 4",
                "message 4 -> edited",
                "deleted edited",
                "bulk"
            ]
        );
    }

    #[test]
    fn excluded_channels_never_retain_a_snapshot_or_displace_scoped_messages() {
        let mut events = Events::new();
        events.on_message_delete(async |_: &Message| -> Result<()> { Ok(()) });
        let prepare = |channels| {
            MessageCacheRuntime::prepare(1, &events, Intents::GUILD_MESSAGES)
                .unwrap()
                .unwrap()
                .with_channels(Some(channels))
        };
        let runtime = prepare([Id::new(2)].into());
        let route = runtime.shard(ShardId::ONE);
        let included = message(4);
        let mut excluded = message(5);
        Arc::make_mut(&mut excluded).channel_id = Id::new(10);
        assert!(route.includes(included.channel_id));
        assert!(!route.includes(excluded.channel_id));
        route.store(&included);
        route.store(&excluded);
        assert_eq!(Arc::strong_count(&included), 2);
        assert_eq!(Arc::strong_count(&excluded), 1);
        assert_eq!(route.delete(&deletion(4)).unwrap().id, included.id);

        let empty = prepare(BTreeSet::new());
        let route = empty.shard(ShardId::ONE);
        route.store(&included);
        assert!(!route.includes(included.channel_id));
        assert!(route.delete(&deletion(4)).is_none());
        assert_eq!(Arc::strong_count(&included), 1);
    }
}
