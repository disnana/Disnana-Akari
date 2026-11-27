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

/// 権限を取得して計算する。保存（キャッシュ）は、期間を指定したときだけ使う。
pub(crate) struct PermissionResolver {
    ttl: Option<Duration>,
    guilds: Mutex<HashMap<Id<GuildMarker>, Cached<GuildInfo>>>,
    channels: Mutex<HashMap<Id<ChannelMarker>, Cached<ChannelInfo>>>,
}

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
            guilds: Mutex::default(),
            channels: Mutex::default(),
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
        let guild = self.guild(http, guild_id).await?;
        let channel = self.channel(http, message.channel_id).await?;
        // スレッドの権限は、親チャンネルのものを使う。
        let channel = if channel.kind.is_thread() {
            let parent = channel
                .parent_id
                .ok_or(CheckFailure::PermissionsUnavailable)?;
            self.channel(http, parent).await?
        } else {
            channel
        };
        Ok(compute(
            guild_id,
            message.author.id,
            &member.roles,
            &guild,
            &channel,
        ))
    }

    async fn guild(&self, http: &Client, id: Id<GuildMarker>) -> Result<Arc<GuildInfo>> {
        if let Some(found) = self.lookup(&self.guilds, id) {
            return Ok(found);
        }
        let guild = http.guild(id).await?.model().await?;
        let info = Arc::new(GuildInfo {
            owner_id: guild.owner_id,
            roles: guild
                .roles
                .into_iter()
                .map(|role| (role.id, role.permissions))
                .collect(),
        });
        self.store(&self.guilds, id, &info);
        Ok(info)
    }

    async fn channel(&self, http: &Client, id: Id<ChannelMarker>) -> Result<Arc<ChannelInfo>> {
        if let Some(found) = self.lookup(&self.channels, id) {
            return Ok(found);
        }
        let channel = http.channel(id).await?.model().await?;
        let info = Arc::new(ChannelInfo {
            kind: channel.kind,
            parent_id: channel.parent_id,
            overwrites: channel.permission_overwrites.unwrap_or_default(),
        });
        self.store(&self.channels, id, &info);
        Ok(info)
    }

    fn lookup<K: std::hash::Hash + Eq, T>(
        &self,
        map: &Mutex<HashMap<K, Cached<T>>>,
        key: K,
    ) -> Option<Arc<T>> {
        let ttl = self.ttl?;
        let map = map.lock().ok()?;
        let cached = map.get(&key)?;
        (cached.fetched.elapsed() < ttl).then(|| Arc::clone(&cached.value))
    }

    fn store<K: std::hash::Hash + Eq, T>(
        &self,
        map: &Mutex<HashMap<K, Cached<T>>>,
        key: K,
        value: &Arc<T>,
    ) {
        let Some(ttl) = self.ttl else {
            return;
        };
        let Ok(mut map) = map.lock() else {
            return;
        };
        if map.len() >= CACHE_LIMIT {
            map.retain(|_, cached| cached.fetched.elapsed() < ttl);
            if map.len() >= CACHE_LIMIT {
                map.clear();
            }
        }
        map.insert(
            key,
            Cached {
                fetched: Instant::now(),
                value: Arc::clone(value),
            },
        );
    }
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
    fn cache_is_off_by_default_and_expires() {
        let off = PermissionResolver::new(None);
        let value = Arc::new(5);
        off.store(&Mutex::new(HashMap::new()), 1, &value);
        let map: Mutex<HashMap<i32, Cached<i32>>> = Mutex::default();
        off.store(&map, 1, &value);
        assert!(off.lookup(&map, 1).is_none());

        let on = PermissionResolver::new(Some(Duration::from_secs(60)));
        on.store(&map, 1, &value);
        assert_eq!(on.lookup(&map, 1).as_deref(), Some(&5));
        // 期間が 0 なら、保存しない。
        let zero = PermissionResolver::new(Some(Duration::ZERO));
        assert!(zero.ttl.is_none());
        // 期限切れは返さない。
        map.lock().unwrap().get_mut(&1).unwrap().fetched =
            Instant::now().checked_sub(Duration::from_secs(61)).unwrap();
        assert!(on.lookup(&map, 1).is_none());
    }
}
