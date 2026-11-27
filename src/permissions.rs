//! テキストコマンドの権限を、REST で調べて計算する。
//!
//! スラッシュコマンドは Discord が計算済みの権限を送ってくれるが、メッセージには付かない。
//! そのため、サーバー（ロールとオーナー）とチャンネル（権限の上書き）を取得して計算する。
//! 計算そのものは、Twilight 公式の `twilight-util` の権限計算を使う。

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tokio::sync::OnceCell;
use twilight_http::Client;
use twilight_model::{
    channel::{ChannelType, Message, permission_overwrite::PermissionOverwrite},
    guild::Permissions,
    id::{
        Id,
        marker::{ChannelMarker, GuildMarker, RoleMarker, UserMarker},
    },
};
use twilight_util::permission_calculator::PermissionCalculator;

use crate::{CheckFailure, Result};

/// 保存する件数の上限。超えたら、古いものから捨てる。
const CACHE_LIMIT: usize = 512;

/// 権限の確認。サーバー外、権限が不明、不足の順に失敗する。
pub(crate) fn check_permissions(
    guild_id: Option<Id<GuildMarker>>,
    held: Option<Permissions>,
    required: Permissions,
) -> std::result::Result<(), CheckFailure> {
    if guild_id.is_none() {
        return Err(CheckFailure::GuildOnly);
    }
    let held = held.ok_or(CheckFailure::PermissionsUnavailable)?;
    let missing = required - held;
    if missing.is_empty() {
        Ok(())
    } else {
        Err(CheckFailure::MissingPermissions(missing))
    }
}

#[derive(Debug)]
struct GuildInfo {
    owner_id: Id<UserMarker>,
    roles: HashMap<Id<RoleMarker>, Permissions>,
}

#[derive(Debug)]
struct ChannelInfo {
    kind: ChannelType,
    parent_id: Option<Id<ChannelMarker>>,
    overwrites: Vec<PermissionOverwrite>,
}

struct Cached<T> {
    fetched: Instant,
    value: Arc<T>,
}

struct CacheSlot<T> {
    guild_id: Id<GuildMarker>,
    data: OnceCell<Cached<T>>,
}

struct Snapshot<T> {
    slot: Option<Arc<CacheSlot<T>>>,
    value: Arc<T>,
}

impl<T> Snapshot<T> {
    async fn load(
        slot: Option<Arc<CacheSlot<T>>>,
        fetch: impl AsyncFnOnce() -> Result<T>,
    ) -> Result<Self> {
        let value = if let Some(slot) = &slot {
            let loaded = slot
                .data
                .get_or_try_init(async || {
                    Ok::<_, crate::Error>(Cached {
                        value: Arc::new(fetch().await?),
                        fetched: Instant::now(),
                    })
                })
                .await?;
            Arc::clone(&loaded.value)
        } else {
            Arc::new(fetch().await?)
        };
        Ok(Self { slot, value })
    }

    fn is_current(&self, current: Option<&Arc<CacheSlot<T>>>) -> bool {
        self.slot
            .as_ref()
            .zip(current)
            .is_some_and(|(old, current)| Arc::ptr_eq(old, current))
    }
}

/// 権限を取得して計算する。保存（キャッシュ）は、期間を指定したときだけ使う。
pub(crate) struct PermissionResolver {
    ttl: Option<Duration>,
    cache: Mutex<Cache>,
}

#[derive(Default)]
struct Cache {
    guilds: SlotMap<Id<GuildMarker>, GuildInfo>,
    channels: SlotMap<Id<ChannelMarker>, ChannelInfo>,
}

type SlotMap<K, T> = HashMap<K, Arc<CacheSlot<T>>>;

impl std::fmt::Debug for PermissionResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PermissionResolver")
            .field("ttl", &self.ttl)
            .finish_non_exhaustive()
    }
}

impl PermissionResolver {
    pub(crate) fn new(ttl: Option<Duration>) -> Self {
        Self {
            ttl: ttl.filter(|ttl| !ttl.is_zero()),
            cache: Mutex::default(),
        }
    }

    pub(crate) fn cache_enabled(&self) -> bool {
        self.ttl.is_some()
    }

    pub(crate) fn invalidate_guild(&self, id: Id<GuildMarker>) {
        self.invalidate(|cache| {
            cache.guilds.remove(&id);
        });
    }

    pub(crate) fn remove_guild(&self, id: Id<GuildMarker>) {
        self.invalidate(|cache| {
            cache.guilds.remove(&id);
            cache.channels.retain(|_, channel| channel.guild_id != id);
        });
    }

    pub(crate) fn invalidate_channel(&self, id: Id<ChannelMarker>) {
        self.invalidate(|cache| {
            cache.channels.remove(&id);
        });
    }

    pub(crate) fn clear_cache(&self) {
        self.invalidate(|cache| {
            cache.guilds.clear();
            cache.channels.clear();
        });
    }

    fn invalidate(&self, update: impl FnOnce(&mut Cache)) {
        if self.cache_enabled()
            && let Ok(mut cache) = self.cache.lock()
        {
            update(&mut cache);
        }
    }

    /// メッセージを送った人の、そのチャンネルでの権限を返す。
    pub(crate) async fn resolve(&self, http: &Client, message: &Message) -> Result<Permissions> {
        let guild_id = message.guild_id.ok_or(CheckFailure::GuildOnly)?;
        // ロールはメッセージに付いてくる。無ければ、誰の権限か判断できない。
        let member = message
            .member
            .as_ref()
            .ok_or(CheckFailure::PermissionsUnavailable)?;
        let channel_id = message.channel_id;
        for _ in 0..2 {
            let guild = self.guild(http, guild_id).await?;
            let channel = self.channel(http, guild_id, channel_id).await?;
            // スレッドの権限は親チャンネルで計算し、参照したスレッド自身も検証する。
            let parent = if channel.value.kind.is_thread() {
                let id = channel
                    .value
                    .parent_id
                    .ok_or(CheckFailure::PermissionsUnavailable)?;
                Some((id, self.channel(http, guild_id, id).await?))
            } else {
                None
            };
            if self.snapshots_are_current(
                guild_id,
                &guild,
                channel_id,
                &channel,
                parent.as_ref(),
            )? {
                return Ok(compute(
                    guild_id,
                    message.author.id,
                    &member.roles,
                    &guild.value,
                    parent
                        .as_ref()
                        .map_or(&channel.value, |(_, parent)| &parent.value),
                ));
            }
        }
        // 取得中に再び変更が入った場合には、古い権限で許可しない。
        Err(CheckFailure::PermissionsUnavailable.into())
    }

    fn snapshots_are_current(
        &self,
        guild_id: Id<GuildMarker>,
        guild: &Snapshot<GuildInfo>,
        channel_id: Id<ChannelMarker>,
        channel: &Snapshot<ChannelInfo>,
        parent: Option<&(Id<ChannelMarker>, Snapshot<ChannelInfo>)>,
    ) -> std::result::Result<bool, CheckFailure> {
        if !self.cache_enabled() {
            return Ok(true);
        }
        let cache = self
            .cache
            .lock()
            .map_err(|_| CheckFailure::PermissionsUnavailable)?;
        Ok(guild.is_current(cache.guilds.get(&guild_id))
            && channel.is_current(cache.channels.get(&channel_id))
            && parent.is_none_or(|(id, snapshot)| snapshot.is_current(cache.channels.get(id))))
    }

    async fn guild(&self, http: &Client, id: Id<GuildMarker>) -> Result<Snapshot<GuildInfo>> {
        let slot = self.slot(id, id, |cache| &mut cache.guilds)?;
        Snapshot::load(slot, async || {
            let guild = http.guild(id).await?.model().await?;
            Ok(GuildInfo {
                owner_id: guild.owner_id,
                roles: guild
                    .roles
                    .into_iter()
                    .map(|role| (role.id, role.permissions))
                    .collect(),
            })
        })
        .await
    }

    async fn channel(
        &self,
        http: &Client,
        guild_id: Id<GuildMarker>,
        id: Id<ChannelMarker>,
    ) -> Result<Snapshot<ChannelInfo>> {
        let slot = self.slot(id, guild_id, |cache| &mut cache.channels)?;
        Snapshot::load(slot, async || {
            let channel = http.channel(id).await?.model().await?;
            Ok(ChannelInfo {
                kind: channel.kind,
                parent_id: channel.parent_id,
                overwrites: channel.permission_overwrites.unwrap_or_default(),
            })
        })
        .await
    }

    fn slot<K: Copy + std::hash::Hash + Eq, T>(
        &self,
        key: K,
        guild_id: Id<GuildMarker>,
        map: impl FnOnce(&mut Cache) -> &mut SlotMap<K, T>,
    ) -> std::result::Result<Option<Arc<CacheSlot<T>>>, CheckFailure> {
        let Some(ttl) = self.ttl else {
            return Ok(None);
        };
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| CheckFailure::PermissionsUnavailable)?;
        reserve_slot(map(&mut cache), key, guild_id, ttl).map(Some)
    }
}

fn reserve_slot<K: Copy + std::hash::Hash + Eq, T>(
    map: &mut SlotMap<K, T>,
    key: K,
    guild_id: Id<GuildMarker>,
    ttl: Duration,
) -> std::result::Result<Arc<CacheSlot<T>>, CheckFailure> {
    if let Some(old) = map.get(&key) {
        if old
            .data
            .get()
            .is_none_or(|data| data.fetched.elapsed() < ttl)
        {
            return Ok(Arc::clone(old));
        }
    } else if map.len() >= CACHE_LIMIT {
        // 実行中の確認が参照していない枠から、空または取得時刻が最も古いものを捨てる。
        let evict = map
            .iter()
            .filter(|(_, slot)| Arc::strong_count(slot) == 1)
            .min_by_key(|(_, slot)| slot.data.get().map(|data| data.fetched))
            .map(|(key, _)| *key)
            .ok_or(CheckFailure::PermissionsUnavailable)?;
        map.remove(&evict);
    }
    // REST 完了時はこの枠にだけ値を書く。変更イベントで枠が外れた場合、書き戻しは起きない。
    let slot = Arc::new(CacheSlot {
        guild_id,
        data: OnceCell::new(),
    });
    map.insert(key, Arc::clone(&slot));
    Ok(slot)
}

/// チャンネルではなく、サーバー全体にだけ意味がある権限。
///
/// `twilight-util` のチャンネル単位の計算は、これらを結果から取り除く。
/// 一方 Discord がスラッシュコマンドで送る権限には含まれるため、揃えるために足し戻す。
const GUILD_LEVEL: Permissions = Permissions::from_bits_truncate(
    Permissions::ADMINISTRATOR.bits()
        | Permissions::BAN_MEMBERS.bits()
        | Permissions::KICK_MEMBERS.bits()
        | Permissions::MODERATE_MEMBERS.bits()
        | Permissions::MANAGE_GUILD.bits()
        | Permissions::MANAGE_GUILD_EXPRESSIONS.bits()
        | Permissions::CREATE_GUILD_EXPRESSIONS.bits()
        | Permissions::MANAGE_NICKNAMES.bits()
        | Permissions::CHANGE_NICKNAME.bits()
        | Permissions::VIEW_AUDIT_LOG.bits()
        | Permissions::VIEW_GUILD_INSIGHTS.bits(),
);

/// 取得済みの情報から、権限を計算する。知らないロールは、権限なしとして扱う。
fn compute(
    guild_id: Id<GuildMarker>,
    user_id: Id<UserMarker>,
    member_roles: &[Id<RoleMarker>],
    guild: &GuildInfo,
    channel: &ChannelInfo,
) -> Permissions {
    // @everyone ロールの ID は、サーバーの ID と同じ。
    let everyone = guild
        .roles
        .get(&Id::new(guild_id.get()))
        .copied()
        .unwrap_or_else(Permissions::empty);
    let roles: Vec<_> = member_roles
        .iter()
        .filter_map(|id| guild.roles.get(id).map(|permissions| (*id, *permissions)))
        .collect();
    let calculator =
        PermissionCalculator::new(guild_id, user_id, everyone, &roles).owner_id(guild.owner_id);
    let root = calculator.root();
    let in_channel = calculator.in_channel(channel.kind, &channel.overwrites);
    in_channel | (root & GUILD_LEVEL)
}

#[cfg(test)]
mod tests {
    use twilight_model::channel::permission_overwrite::PermissionOverwriteType;

    use super::*;

    fn guild(owner: u64, roles: &[(u64, Permissions)]) -> GuildInfo {
        GuildInfo {
            owner_id: Id::new(owner),
            roles: roles.iter().map(|(id, p)| (Id::new(*id), *p)).collect(),
        }
    }

    fn channel(overwrites: Vec<PermissionOverwrite>) -> ChannelInfo {
        ChannelInfo {
            kind: ChannelType::GuildText,
            parent_id: None,
            overwrites,
        }
    }

    const GUILD: u64 = 100;

    fn calc(user: u64, roles: &[u64], guild: &GuildInfo, channel: &ChannelInfo) -> Permissions {
        let roles: Vec<Id<RoleMarker>> = roles.iter().map(|id| Id::new(*id)).collect();
        compute(Id::new(GUILD), Id::new(user), &roles, guild, channel)
    }

    #[test]
    fn combines_everyone_and_member_roles() {
        let guild = guild(
            1,
            &[
                (
                    GUILD,
                    Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES,
                ),
                (200, Permissions::BAN_MEMBERS),
                (300, Permissions::KICK_MEMBERS),
            ],
        );
        let held = calc(2, &[200], &guild, &channel(vec![]));
        assert!(held.contains(Permissions::SEND_MESSAGES | Permissions::BAN_MEMBERS));
        assert!(!held.contains(Permissions::KICK_MEMBERS));
        // 知らないロールは、権限なしとして扱う（余計に許可しない）。
        assert_eq!(
            calc(2, &[999], &guild, &channel(vec![])),
            calc(2, &[], &guild, &channel(vec![]))
        );
    }

    #[test]
    fn owner_and_administrator_have_everything() {
        let guild = guild(
            1,
            &[
                (GUILD, Permissions::empty()),
                (200, Permissions::ADMINISTRATOR),
            ],
        );
        assert_eq!(calc(1, &[], &guild, &channel(vec![])), Permissions::all());
        assert_eq!(
            calc(2, &[200], &guild, &channel(vec![])),
            Permissions::all()
        );
        assert!(!calc(2, &[], &guild, &channel(vec![])).contains(Permissions::BAN_MEMBERS));
    }

    #[test]
    fn channel_overwrites_can_deny_a_role_permission() {
        let guild = guild(
            1,
            &[
                (GUILD, Permissions::VIEW_CHANNEL),
                (200, Permissions::MANAGE_MESSAGES),
            ],
        );
        let overwrite = PermissionOverwrite {
            allow: Permissions::empty(),
            deny: Permissions::MANAGE_MESSAGES,
            id: Id::new(200),
            kind: PermissionOverwriteType::Role,
        };
        let held = calc(2, &[200], &guild, &channel(vec![overwrite]));
        assert!(!held.contains(Permissions::MANAGE_MESSAGES));
        assert!(held.contains(Permissions::VIEW_CHANNEL));
    }

    #[test]
    fn permission_check_fails_closed_and_reports_what_is_missing() {
        let guild = Some(Id::new(1));
        let ban = Permissions::BAN_MEMBERS;
        let both = Permissions::BAN_MEMBERS | Permissions::KICK_MEMBERS;
        assert_eq!(check_permissions(guild, Some(both), ban), Ok(()));
        assert_eq!(
            check_permissions(guild, Some(both), Permissions::empty()),
            Ok(())
        );
        assert_eq!(
            check_permissions(guild, Some(ban), both),
            Err(CheckFailure::MissingPermissions(Permissions::KICK_MEMBERS))
        );
        assert_eq!(
            check_permissions(guild, None, ban),
            Err(CheckFailure::PermissionsUnavailable)
        );
        assert_eq!(
            check_permissions(None, Some(both), ban),
            Err(CheckFailure::GuildOnly)
        );
    }

    #[test]
    fn cache_is_off_by_default_and_expired_slots_are_replaced() -> Result<()> {
        let off = PermissionResolver::new(None);
        let guild_id = Id::new(GUILD);
        assert!(
            off.slot(guild_id, guild_id, |cache| &mut cache.guilds)?
                .is_none()
        );
        assert!(!PermissionResolver::new(Some(Duration::ZERO)).cache_enabled());

        let ttl = Duration::from_secs(60);
        let mut map = HashMap::new();
        let old = reserve_slot(&mut map, 1, guild_id, ttl)?;
        assert!(
            old.data
                .set(Cached {
                    fetched: Instant::now().checked_sub(ttl).unwrap(),
                    value: Arc::new(5),
                })
                .is_ok()
        );
        let new = reserve_slot(&mut map, 1, guild_id, ttl)?;
        assert!(!Arc::ptr_eq(&old, &new));
        assert!(new.data.get().is_none());
        Ok(())
    }

    #[tokio::test]
    async fn invalidation_is_scoped_and_does_not_restore_an_in_flight_snapshot() -> Result<()> {
        let resolver = PermissionResolver::new(Some(Duration::from_secs(60)));
        let guild_id = Id::new(GUILD);
        let other_guild = Id::new(GUILD + 1);
        let parent_id = Id::new(42);
        let thread_id = Id::new(43);
        let other_channel = Id::new(44);
        let guild = Snapshot::load(
            resolver.slot(guild_id, guild_id, |cache| &mut cache.guilds)?,
            async || Ok(guild(1, &[])),
        )
        .await?;
        let thread = Snapshot::load(
            resolver.slot(thread_id, guild_id, |cache| &mut cache.channels)?,
            async || Ok(channel(Vec::new())),
        )
        .await?;
        let parent_slot = resolver.slot(parent_id, guild_id, |cache| &mut cache.channels)?;
        let parent = (
            parent_id,
            Snapshot::load(parent_slot, async || {
                resolver.invalidate_guild(other_guild);
                resolver.invalidate_channel(other_channel);
                Ok(channel(Vec::new()))
            })
            .await?,
        );
        assert!(resolver.snapshots_are_current(
            guild_id,
            &guild,
            thread_id,
            &thread,
            Some(&parent)
        )?);

        resolver.invalidate_channel(parent_id);
        let old_slot = resolver.slot(parent_id, guild_id, |cache| &mut cache.channels)?;
        let stale = Snapshot::load(old_slot, async || {
            resolver.invalidate_channel(parent_id);
            Ok(channel(Vec::new()))
        })
        .await?;
        let fresh_slot = resolver.slot(parent_id, guild_id, |cache| &mut cache.channels)?;
        assert!(!stale.is_current(fresh_slot.as_ref()));
        assert!(fresh_slot.unwrap().data.get().is_none());
        assert!(!resolver.snapshots_are_current(
            guild_id,
            &guild,
            thread_id,
            &thread,
            Some(&parent)
        )?);

        resolver.slot(other_channel, other_guild, |cache| &mut cache.channels)?;
        resolver.remove_guild(guild_id);
        assert!(!resolver.snapshots_are_current(guild_id, &guild, thread_id, &thread, None)?);
        assert!(
            resolver
                .cache
                .lock()
                .unwrap()
                .channels
                .contains_key(&other_channel)
        );
        resolver.clear_cache();
        let cache = resolver.cache.lock().unwrap();
        assert!(cache.guilds.is_empty());
        assert!(cache.channels.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_cache_misses_share_one_fetch() -> Result<()> {
        let guild_id = Id::new(GUILD);
        let mut map = HashMap::new();
        let ttl = Duration::from_secs(60);
        let first = reserve_slot(&mut map, 1, guild_id, ttl)?;
        let second = reserve_slot(&mut map, 1, guild_id, ttl)?;
        let (first, second) = tokio::join!(
            Snapshot::load(Some(first), async || {
                tokio::task::yield_now().await;
                Ok(5)
            }),
            Snapshot::load(Some(second), async || panic!("fetch was not coalesced")),
        );
        assert!(Arc::ptr_eq(&first?.value, &second?.value));
        Ok(())
    }

    #[test]
    fn capacity_does_not_evict_an_active_snapshot() -> Result<()> {
        let guild_id = Id::new(GUILD);
        let mut map: SlotMap<usize, ()> = HashMap::new();
        let ttl = Duration::from_secs(60);
        let active = reserve_slot(&mut map, 0, guild_id, ttl)?;
        for key in 1..=CACHE_LIMIT {
            reserve_slot(&mut map, key, guild_id, ttl)?;
        }
        assert_eq!(map.len(), CACHE_LIMIT);
        assert!(Arc::ptr_eq(&active, map.get(&0).unwrap()));
        Ok(())
    }
}
