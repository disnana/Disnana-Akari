use std::collections::HashMap;

use twilight_gateway::{Event, EventTypeFlags};
use twilight_model::{
    gateway::{ShardId, payload::incoming::GuildCreate},
    id::{Id, marker::GuildMarker},
};

/// Gateway から観測した、Bot が参加しているサーバーの状態の変化です。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GuildLifecycleKind {
    /// この Gateway セッションの開始時から参加していたサーバーが利用可能になりました。
    InitialAvailable,
    /// セッション開始後に参加したサーバーが初めて利用可能になりました。
    Joined,
    /// 一時的に利用不能だったサーバーが復旧しました。
    Available,
    /// 利用可能だったサーバーが一時的に利用不能になりました。
    Unavailable,
    /// Bot がサーバーから退出したか、サーバーから削除されました。
    Removed,
}

/// サーバーの状態変化と、その変化を受信したシャードです。
///
/// サーバーの詳細が必要な場合は、`Events::on_gateway` で Gateway のペイロードも受け取れます。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct GuildLifecycleEvent {
    guild_id: Id<GuildMarker>,
    shard: ShardId,
    kind: GuildLifecycleKind,
}

impl GuildLifecycleEvent {
    /// 状態が変わったサーバーの ID を返します。
    pub const fn guild_id(&self) -> Id<GuildMarker> {
        self.guild_id
    }

    /// 状態の変化を受け取ったシャードの ID を返します。
    pub const fn shard(&self) -> ShardId {
        self.shard
    }

    /// 状態の変化の種類を返します。
    pub const fn kind(&self) -> GuildLifecycleKind {
        self.kind
    }
}

pub(crate) const LIFECYCLE_EVENTS: EventTypeFlags = EventTypeFlags::READY
    .union(EventTypeFlags::GUILD_CREATE)
    .union(EventTypeFlags::GUILD_DELETE);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GuildState {
    Initial,
    PendingJoin,
    Available,
    Unavailable,
}

#[derive(Debug)]
pub(crate) struct GuildLifecycleTracker {
    shard: ShardId,
    guilds: Option<HashMap<Id<GuildMarker>, GuildState>>,
}

impl GuildLifecycleTracker {
    pub(crate) const fn new(shard: ShardId) -> Self {
        Self {
            shard,
            guilds: None,
        }
    }

    /// 状態を確定できなくなったとき、次の Ready まで通知を止める。
    pub(crate) fn reset(&mut self) {
        self.guilds = None;
    }

    pub(crate) fn observe(&mut self, event: &Event) -> Option<GuildLifecycleEvent> {
        if let Event::Ready(ready) = event {
            if ready.shard.is_some_and(|shard| shard != self.shard) {
                self.reset();
            } else {
                self.guilds = Some(
                    ready
                        .guilds
                        .iter()
                        .map(|guild| (guild.id, GuildState::Initial))
                        .collect(),
                );
            }
            return None;
        }

        let guilds = self.guilds.as_mut()?;
        let (guild_id, kind) = match event {
            Event::GuildCreate(create) => {
                let id = create.id();
                let kind = match create.as_ref() {
                    GuildCreate::Available(guild) => match guilds.insert(id, GuildState::Available)
                    {
                        None if guild.unavailable == Some(false) => GuildLifecycleKind::Available,
                        None | Some(GuildState::PendingJoin) => GuildLifecycleKind::Joined,
                        Some(GuildState::Initial) => GuildLifecycleKind::InitialAvailable,
                        Some(GuildState::Unavailable) => GuildLifecycleKind::Available,
                        Some(GuildState::Available) => return None,
                    },
                    GuildCreate::Unavailable(_) => {
                        mark_unavailable(guilds.entry(id).or_insert(GuildState::PendingJoin))?
                    }
                };
                (id, kind)
            }
            Event::GuildDelete(delete) if delete.unavailable == Some(true) => {
                (delete.id, mark_unavailable(guilds.get_mut(&delete.id)?)?)
            }
            Event::GuildDelete(delete) => {
                guilds.remove(&delete.id)?;
                (delete.id, GuildLifecycleKind::Removed)
            }
            // Resume は同じセッションの続きなので所属を更新しない。
            _ => return None,
        };

        Some(GuildLifecycleEvent {
            guild_id,
            shard: self.shard,
            kind,
        })
    }
}

fn mark_unavailable(state: &mut GuildState) -> Option<GuildLifecycleKind> {
    if *state != GuildState::Available {
        return None;
    }
    *state = GuildState::Unavailable;
    Some(GuildLifecycleKind::Unavailable)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use twilight_model::gateway::payload::incoming::GuildDelete;

    use super::*;

    fn ready(ids: &[u64], shard: Option<ShardId>) -> Event {
        let guilds: Vec<Value> = ids
            .iter()
            .map(|id| json!({"id": id.to_string(), "unavailable": true}))
            .collect();
        Event::Ready(
            serde_json::from_value(json!({
                "application": {"flags": 0, "id": "2"},
                "guilds": guilds,
                "resume_gateway_url": "wss://gateway.discord.gg",
                "session_id": "session",
                "shard": shard,
                "user": {"avatar": null, "bot": true, "discriminator": "0", "id": "2", "mfa_enabled": false, "username": "akari"},
                "v": 10
            }))
            .unwrap(),
        )
    }

    fn create(id: u64, unavailable: bool) -> Event {
        create_with_flag(id, unavailable.then_some(true))
    }

    fn create_with_flag(id: u64, unavailable: Option<bool>) -> Event {
        let mut data = if unavailable == Some(true) {
            json!({"id": id.to_string(), "unavailable": true})
        } else {
            json!({
                "id": id.to_string(), "owner_id": "2", "name": "guild",
                "afk_timeout": 60, "default_message_notifications": 0, "explicit_content_filter": 0,
                "features": [], "mfa_level": 0, "nsfw_level": 0, "preferred_locale": "ja",
                "premium_progress_bar_enabled": false, "roles": [], "system_channel_flags": 0,
                "verification_level": 0
            })
        };
        if let Some(unavailable) = unavailable {
            data["unavailable"] = unavailable.into();
        }
        Event::GuildCreate(Box::new(serde_json::from_value(data).unwrap()))
    }

    fn delete(id: u64, unavailable: Option<bool>) -> Event {
        Event::GuildDelete(GuildDelete {
            id: Id::new(id),
            unavailable,
        })
    }

    fn kind(tracker: &mut GuildLifecycleTracker, event: &Event) -> Option<GuildLifecycleKind> {
        tracker.observe(event).map(|event| event.kind())
    }

    #[test]
    fn ready_seeds_only_its_own_shard_and_initial_guilds_do_not_count_as_joins() {
        use GuildLifecycleKind::{InitialAvailable, Joined};

        let first_id = ShardId::new(0, 2);
        let second_id = ShardId::new(1, 2);
        let mut first = GuildLifecycleTracker::new(first_id);
        let mut second = GuildLifecycleTracker::new(second_id);

        assert!(first.observe(&ready(&[10], Some(first_id))).is_none());
        assert!(second.observe(&ready(&[20], Some(second_id))).is_none());
        let event = second.observe(&create(20, false)).unwrap();
        assert_eq!(event.kind(), InitialAvailable);
        assert_eq!(event.shard(), second_id);
        assert_eq!(event.guild_id(), Id::new(20));
        assert_eq!(kind(&mut first, &create(10, false)), Some(InitialAvailable));
        assert_eq!(kind(&mut first, &create(30, false)), Some(Joined));
        assert_eq!(kind(&mut first, &create(30, false)), None);

        assert!(first.observe(&ready(&[30], Some(first_id))).is_none());
        assert_eq!(kind(&mut first, &create(30, false)), Some(InitialAvailable));
    }

    #[test]
    fn outages_and_removals_remain_distinct_across_resume() {
        use GuildLifecycleKind::{Available, InitialAvailable, Joined, Removed, Unavailable};

        let mut tracker = GuildLifecycleTracker::new(ShardId::ONE);
        assert!(tracker.observe(&ready(&[10, 20], None)).is_none());
        assert_eq!(kind(&mut tracker, &delete(10, Some(true))), None);
        assert_eq!(
            kind(&mut tracker, &create(10, false)),
            Some(InitialAvailable)
        );
        assert_eq!(
            kind(&mut tracker, &delete(10, Some(true))),
            Some(Unavailable)
        );
        assert_eq!(kind(&mut tracker, &delete(10, Some(true))), None);
        assert_eq!(kind(&mut tracker, &Event::Resumed), None);
        assert_eq!(kind(&mut tracker, &create(10, false)), Some(Available));
        assert_eq!(kind(&mut tracker, &delete(10, None)), Some(Removed));
        assert_eq!(kind(&mut tracker, &delete(10, None)), None);
        assert_eq!(kind(&mut tracker, &create(10, false)), Some(Joined));

        assert_eq!(kind(&mut tracker, &create(30, true)), None);
        assert_eq!(kind(&mut tracker, &create(30, false)), Some(Joined));
        assert_eq!(kind(&mut tracker, &create(30, true)), Some(Unavailable));
        assert_eq!(kind(&mut tracker, &delete(30, Some(false))), Some(Removed));
        assert_eq!(kind(&mut tracker, &delete(20, None)), Some(Removed));
    }

    #[test]
    fn malformed_events_suspend_notifications_until_a_fresh_ready() {
        use GuildLifecycleKind::{InitialAvailable, Joined};

        let mut tracker = GuildLifecycleTracker::new(ShardId::ONE);
        assert_eq!(kind(&mut tracker, &create(10, false)), None);
        assert!(tracker.observe(&ready(&[], None)).is_none());
        assert_eq!(kind(&mut tracker, &create(10, false)), Some(Joined));

        tracker.reset();
        assert_eq!(kind(&mut tracker, &create(20, false)), None);
        assert_eq!(kind(&mut tracker, &delete(10, None)), None);
        assert_eq!(kind(&mut tracker, &Event::Resumed), None);
        assert_eq!(kind(&mut tracker, &create(10, false)), None);

        assert!(tracker.observe(&ready(&[10, 20], None)).is_none());
        assert_eq!(
            kind(&mut tracker, &create(10, false)),
            Some(InitialAvailable)
        );
        assert_eq!(
            kind(&mut tracker, &create(20, false)),
            Some(InitialAvailable)
        );
        assert_eq!(kind(&mut tracker, &create(30, false)), Some(Joined));
    }

    #[test]
    fn explicit_recovery_for_an_unknown_guild_does_not_count_as_joining() {
        use GuildLifecycleKind::{Available, InitialAvailable, Joined};

        let mut tracker = GuildLifecycleTracker::new(ShardId::ONE);
        assert!(tracker.observe(&ready(&[10], None)).is_none());
        assert_eq!(
            kind(&mut tracker, &create_with_flag(10, Some(false))),
            Some(InitialAvailable)
        );
        assert_eq!(
            kind(&mut tracker, &create_with_flag(20, Some(false))),
            Some(Available)
        );
        assert_eq!(kind(&mut tracker, &create_with_flag(20, Some(false))), None);
        assert_eq!(kind(&mut tracker, &create(30, true)), None);
        assert_eq!(
            kind(&mut tracker, &create_with_flag(30, Some(false))),
            Some(Joined)
        );
    }
}
