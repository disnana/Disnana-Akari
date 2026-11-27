//! 実行者、Bot、ほかのメンバーの権限を調べ、確認する。
//!
//! 計算は `PermissionResolver` に任せる。スラッシュコマンドでは、Discord が送った実行者と
//! Bot の権限をそのまま使い、通信しない。それ以外は、ロールとチャンネルを取得して計算する。
//! 取得できないとき、判断できないときは、許可せずに失敗する。

use twilight_http::Client;
use twilight_model::{
    application::interaction::Interaction,
    channel::Message,
    guild::Permissions,
    id::{
        Id,
        marker::{ChannelMarker, GuildMarker, RoleMarker, UserMarker},
    },
    util::datetime::Timestamp,
};

use crate::{
    CheckFailure, HierarchyDenied, Result,
    permissions::{PermissionResolver, check_permissions},
    slash::interaction_channel_id,
};

/// コマンドを実行した人の情報。
#[derive(Clone, Copy)]
struct Actor<'a> {
    id: Id<UserMarker>,
    roles: &'a [Id<RoleMarker>],
    timed_out_until: Option<Timestamp>,
    /// Discord が計算して送ってきた権限（スラッシュコマンドだけ）。
    permissions: Option<Permissions>,
}

/// 実行者、Bot、ほかのメンバーの権限を調べ、確認します。
///
/// `ctx.access()` で取得します。勝手には確認せず、呼んだときだけ動きます。
/// スラッシュコマンドでは、実行者と Bot の権限に通信は要りません（Discord が送ります）。
#[derive(Clone, Copy)]
pub struct Access<'a> {
    http: &'a Client,
    resolver: &'a PermissionResolver,
    guild_id: Option<Id<GuildMarker>>,
    channel_id: Option<Id<ChannelMarker>>,
    actor: Option<Actor<'a>>,
    /// Discord が送ってきた、Bot 自身の権限（スラッシュコマンドだけ）。
    app_permissions: Option<Permissions>,
}

impl std::fmt::Debug for Access<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Access")
            .field("guild_id", &self.guild_id)
            .field("channel_id", &self.channel_id)
            .finish_non_exhaustive()
    }
}

/// 実行者と Bot の権限を、一度にまとめて取得した結果です。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct PermissionSnapshot {
    /// 呼ばれたサーバーです。
    pub guild_id: Id<GuildMarker>,
    /// 呼ばれたチャンネルです。
    pub channel_id: Id<ChannelMarker>,
    /// 実行者の、このチャンネルでの権限です。
    pub author: Permissions,
    /// Bot の、このチャンネルでの権限です。
    pub bot: Permissions,
    /// 実行者が、サーバーの所有者かどうかです。
    pub author_is_owner: bool,
}

impl<'a> Access<'a> {
    pub(crate) fn for_message(
        http: &'a Client,
        resolver: &'a PermissionResolver,
        message: &'a Message,
    ) -> Self {
        Self {
            http,
            resolver,
            guild_id: message.guild_id,
            channel_id: Some(message.channel_id),
            actor: message.member.as_ref().map(|member| Actor {
                id: message.author.id,
                roles: &member.roles,
                timed_out_until: member.communication_disabled_until,
                permissions: None,
            }),
            app_permissions: None,
        }
    }

    pub(crate) fn for_interaction(
        http: &'a Client,
        resolver: &'a PermissionResolver,
        interaction: &'a Interaction,
    ) -> Self {
        let actor = interaction
            .author_id()
            .zip(interaction.member.as_ref())
            .map(|(id, member)| Actor {
                id,
                roles: &member.roles,
                timed_out_until: member.communication_disabled_until,
                permissions: member.permissions,
            });
        Self {
            http,
            resolver,
            guild_id: interaction.guild_id,
            channel_id: interaction_channel_id(interaction),
            actor,
            app_permissions: interaction.app_permissions,
        }
    }

    /// サーバーの中で呼ばれたことを確認し、その ID を返します。
    ///
    /// # エラー
    ///
    /// DM などでは [`CheckFailure::GuildOnly`] を返します。
    pub fn require_guild(&self) -> Result<Id<GuildMarker>> {
        self.guild_id.ok_or_else(|| CheckFailure::GuildOnly.into())
    }

    fn place(&self) -> Result<(Id<GuildMarker>, Id<ChannelMarker>)> {
        let guild_id = self.require_guild()?;
        let channel_id = self
            .channel_id
            .ok_or(CheckFailure::PermissionsUnavailable)?;
        Ok((guild_id, channel_id))
    }

    fn actor(&self) -> Result<Actor<'a>> {
        self.actor
            .ok_or_else(|| CheckFailure::PermissionsUnavailable.into())
    }

    /// 実行した人の、このチャンネルでの権限を返します。
    ///
    /// # エラー
    ///
    /// DM では [`CheckFailure::GuildOnly`]、権限を計算できないときは
    /// [`CheckFailure::PermissionsUnavailable`]、取得に失敗したときは HTTP のエラーを返します。
    pub async fn permissions(&self) -> Result<Permissions> {
        let (guild_id, channel_id) = self.place()?;
        let actor = self.actor()?;
        if let Some(permissions) = actor.permissions {
            return Ok(permissions);
        }
        self.resolver
            .resolve_member(
                self.http,
                guild_id,
                channel_id,
                actor.id,
                actor.roles,
                actor.timed_out_until,
            )
            .await
    }

    /// Bot 自身の、このチャンネルでの権限を返します。
    ///
    /// スラッシュコマンドでは、Discord が送った値を使い、通信しません。
    ///
    /// # エラー
    ///
    /// [`permissions`](Self::permissions) と同じです。
    pub async fn bot_permissions(&self) -> Result<Permissions> {
        let (guild_id, channel_id) = self.place()?;
        if let Some(permissions) = self.app_permissions {
            return Ok(permissions);
        }
        let bot_id = self.resolver.bot_id(self.http).await?;
        self.member_permissions(guild_id, channel_id, bot_id).await
    }

    /// 指定したメンバーの、このチャンネルでの権限を返します。メンバーを 1 回取得します。
    ///
    /// # エラー
    ///
    /// 対象がサーバーにいないときなど、取得に失敗したときは HTTP のエラーを返します。
    pub async fn permissions_of(&self, user_id: Id<UserMarker>) -> Result<Permissions> {
        let (guild_id, channel_id) = self.place()?;
        if let Some(actor) = self.actor.filter(|actor| actor.id == user_id)
            && let Some(permissions) = actor.permissions
        {
            return Ok(permissions);
        }
        self.member_permissions(guild_id, channel_id, user_id).await
    }

    async fn member_permissions(
        &self,
        guild_id: Id<GuildMarker>,
        channel_id: Id<ChannelMarker>,
        user_id: Id<UserMarker>,
    ) -> Result<Permissions> {
        let member = self
            .http
            .guild_member(guild_id, user_id)
            .await?
            .model()
            .await?;
        self.resolver
            .resolve_member(
                self.http,
                guild_id,
                channel_id,
                user_id,
                &member.roles,
                member.communication_disabled_until,
            )
            .await
    }

    /// 実行した人が、指定した権限をすべて持っているかを返します。拒否はしません。
    ///
    /// # エラー
    ///
    /// [`permissions`](Self::permissions) と同じです。持っていない場合は `Ok(false)` です。
    pub async fn can(&self, required: Permissions) -> Result<bool> {
        Ok(self.missing(required).await?.is_empty())
    }

    /// 実行した人に足りない権限を返します。足りていれば空です。
    ///
    /// # エラー
    ///
    /// [`permissions`](Self::permissions) と同じです。
    pub async fn missing(&self, required: Permissions) -> Result<Permissions> {
        Ok(required - self.permissions().await?)
    }

    /// Bot が、指定した権限をすべて持っているかを返します。拒否はしません。
    ///
    /// # エラー
    ///
    /// [`bot_permissions`](Self::bot_permissions) と同じです。
    pub async fn bot_can(&self, required: Permissions) -> Result<bool> {
        Ok(self.bot_missing(required).await?.is_empty())
    }

    /// Bot に足りない権限を返します。足りていれば空です。
    ///
    /// # エラー
    ///
    /// [`bot_permissions`](Self::bot_permissions) と同じです。
    pub async fn bot_missing(&self, required: Permissions) -> Result<Permissions> {
        Ok(required - self.bot_permissions().await?)
    }

    /// 実行した人が、指定した権限をすべて持っていることを確認します。
    ///
    /// # エラー
    ///
    /// 足りないときは、不足分を入れた [`CheckFailure::MissingPermissions`] を返します。
    pub async fn require_permissions(&self, required: Permissions) -> Result<()> {
        let guild_id = self.require_guild()?;
        let held = self.permissions().await?;
        check_permissions(Some(guild_id), Some(held), required).map_err(Into::into)
    }

    /// Bot が、指定した権限をすべて持っていることを確認します。
    ///
    /// # エラー
    ///
    /// 足りないときは、不足分を入れた [`CheckFailure::BotMissingPermissions`] を返します。
    pub async fn require_bot_permissions(&self, required: Permissions) -> Result<()> {
        let missing = self.bot_missing(required).await?;
        if missing.is_empty() {
            Ok(())
        } else {
            Err(CheckFailure::BotMissingPermissions(missing).into())
        }
    }

    /// 実行者と Bot の権限を、一度にまとめて取得します。
    ///
    /// # エラー
    ///
    /// どちらかの権限を計算できないときは、許可せずに失敗します（[`permissions`](Self::permissions) を参照）。
    pub async fn snapshot(&self) -> Result<PermissionSnapshot> {
        let (guild_id, channel_id) = self.place()?;
        let author = self.permissions().await?;
        let bot = self.bot_permissions().await?;
        let owner_id = self
            .resolver
            .guild_info(self.http, guild_id)
            .await?
            .owner_id;
        Ok(PermissionSnapshot {
            guild_id,
            channel_id,
            author,
            bot,
            author_is_owner: owner_id == self.actor()?.id,
        })
    }

    /// 実行した人が、サーバーの所有者であることを確認します。
    ///
    /// # エラー
    ///
    /// 所有者でないときは [`CheckFailure::NotGuildOwner`] を返します。
    pub async fn require_guild_owner(&self) -> Result<()> {
        let guild_id = self.require_guild()?;
        let actor = self.actor()?;
        let owner_id = self
            .resolver
            .guild_info(self.http, guild_id)
            .await?
            .owner_id;
        if owner_id == actor.id {
            Ok(())
        } else {
            Err(CheckFailure::NotGuildOwner.into())
        }
    }

    /// 実行した人が、指定したロールを持っていることを確認します。通信はしません。
    ///
    /// # エラー
    ///
    /// 持っていないときは [`CheckFailure::MissingRole`] を返します。
    pub fn require_role(&self, role: Id<RoleMarker>) -> Result<()> {
        self.require_any_role(&[role])
    }

    /// 実行した人が、指定したロールのどれかを持っていることを確認します。通信はしません。
    ///
    /// # エラー
    ///
    /// どれも持っていないときは [`CheckFailure::MissingRole`] を返します。
    pub fn require_any_role(&self, roles: &[Id<RoleMarker>]) -> Result<()> {
        self.require_guild()?;
        let actor = self.actor()?;
        if roles.iter().any(|role| actor.roles.contains(role)) {
            Ok(())
        } else {
            Err(CheckFailure::MissingRole.into())
        }
    }

    /// ロールの階層から見て、実行した人と Bot が、対象のメンバーを操作できることを確認します。
    ///
    /// サーバーの所有者は操作できません。最上位ロールが対象以上のときも拒否します（同じ位置も拒否）。
    /// 対象のメンバーと Bot のメンバーを、それぞれ 1 回取得します。
    ///
    /// # エラー
    ///
    /// 操作できないときは [`CheckFailure::Hierarchy`] を返します。
    /// 対象がサーバーにいないときなど、取得に失敗したときは HTTP のエラーを返します。
    pub async fn require_can_moderate(&self, target: Id<UserMarker>) -> Result<()> {
        let guild_id = self.require_guild()?;
        let actor = self.actor()?;
        // 自分自身と Bot は、メンバーを取得するまでもなく拒否できる。
        if target == actor.id {
            return Err(CheckFailure::Hierarchy(HierarchyDenied::TargetIsSelf).into());
        }
        let bot_id = self.resolver.bot_id(self.http).await?;
        if target == bot_id {
            return Err(CheckFailure::Hierarchy(HierarchyDenied::TargetIsBot).into());
        }
        let (info, target_member, bot_member) = tokio::try_join!(
            self.resolver.guild_info(self.http, guild_id),
            fetch_member(self.http, guild_id, target),
            fetch_member(self.http, guild_id, bot_id),
        )?;
        check_hierarchy(&HierarchyInput {
            owner_id: info.owner_id,
            author_id: actor.id,
            bot_id,
            target_id: target,
            author_top: info.top_position(actor.roles),
            bot_top: info.top_position(&bot_member.roles),
            target_top: info.top_position(&target_member.roles),
        })
        .map_err(|denied| CheckFailure::Hierarchy(denied).into())
    }
}

async fn fetch_member(
    http: &Client,
    guild_id: Id<GuildMarker>,
    user_id: Id<UserMarker>,
) -> Result<twilight_model::guild::Member> {
    Ok(http.guild_member(guild_id, user_id).await?.model().await?)
}

/// ロール階層の判定に必要な情報。
#[derive(Clone, Copy, Debug)]
pub(crate) struct HierarchyInput {
    pub(crate) owner_id: Id<UserMarker>,
    pub(crate) author_id: Id<UserMarker>,
    pub(crate) bot_id: Id<UserMarker>,
    pub(crate) target_id: Id<UserMarker>,
    pub(crate) author_top: i64,
    pub(crate) bot_top: i64,
    pub(crate) target_top: i64,
}

/// Discord のロール階層の規則を、純粋に判定する。
///
/// 所有者は誰からも操作されない。最上位ロールが同じ位置のときは「高くない」として拒否する。
/// 管理者権限は、階層を免除しない。
pub(crate) fn check_hierarchy(input: &HierarchyInput) -> std::result::Result<(), HierarchyDenied> {
    if input.target_id == input.bot_id {
        return Err(HierarchyDenied::TargetIsBot);
    }
    if input.target_id == input.author_id {
        return Err(HierarchyDenied::TargetIsSelf);
    }
    if input.target_id == input.owner_id {
        return Err(HierarchyDenied::TargetIsOwner);
    }
    if input.author_id != input.owner_id && input.author_top <= input.target_top {
        return Err(HierarchyDenied::AuthorTooLow);
    }
    if input.bot_top <= input.target_top {
        return Err(HierarchyDenied::BotTooLow);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: u64) -> Id<UserMarker> {
        Id::new(id)
    }

    fn input(author_top: i64, bot_top: i64, target_top: i64) -> HierarchyInput {
        HierarchyInput {
            owner_id: user(1),
            author_id: user(2),
            bot_id: user(3),
            target_id: user(4),
            author_top,
            bot_top,
            target_top,
        }
    }

    #[test]
    fn hierarchy_allows_a_lower_target() {
        assert_eq!(check_hierarchy(&input(5, 6, 3)), Ok(()));
    }

    #[test]
    fn hierarchy_denies_equal_or_higher_roles_for_the_author_and_the_bot() {
        assert_eq!(
            check_hierarchy(&input(3, 6, 3)),
            Err(HierarchyDenied::AuthorTooLow)
        );
        assert_eq!(
            check_hierarchy(&input(2, 6, 3)),
            Err(HierarchyDenied::AuthorTooLow)
        );
        assert_eq!(
            check_hierarchy(&input(5, 3, 3)),
            Err(HierarchyDenied::BotTooLow)
        );
        assert_eq!(
            check_hierarchy(&input(5, 2, 3)),
            Err(HierarchyDenied::BotTooLow)
        );
    }

    #[test]
    fn hierarchy_denies_self_bot_and_owner_targets_first() {
        let mut denied = input(9, 9, 0);
        denied.target_id = denied.author_id;
        assert_eq!(check_hierarchy(&denied), Err(HierarchyDenied::TargetIsSelf));
        denied.target_id = denied.bot_id;
        assert_eq!(check_hierarchy(&denied), Err(HierarchyDenied::TargetIsBot));
        denied.target_id = denied.owner_id;
        assert_eq!(
            check_hierarchy(&denied),
            Err(HierarchyDenied::TargetIsOwner)
        );
    }

    #[test]
    fn the_owner_is_not_limited_by_their_own_role_position() {
        let mut owner = input(0, 6, 3);
        owner.author_id = owner.owner_id;
        assert_eq!(check_hierarchy(&owner), Ok(()));
        // 所有者でも、Bot の階層は越えられない。
        owner.bot_top = 3;
        assert_eq!(check_hierarchy(&owner), Err(HierarchyDenied::BotTooLow));
    }

    #[test]
    fn everyone_only_members_have_position_zero_and_cannot_moderate() {
        assert_eq!(
            check_hierarchy(&input(0, 6, 0)),
            Err(HierarchyDenied::AuthorTooLow)
        );
    }

    /// 通信が起きたら失敗する、行き先のないクライアント。
    fn offline_http() -> Client {
        Client::builder()
            .token("token".into())
            .proxy("127.0.0.1:1".into(), true)
            .build()
    }

    fn slash_interaction(
        guild: bool,
        member_permissions: u64,
        app_permissions: u64,
    ) -> Interaction {
        let mut value = serde_json::json!({
            "id": "1",
            "application_id": "10",
            "type": 2,
            "token": "t",
            "version": 1,
            "attachment_size_limit": 0,
            "authorizing_integration_owners": {},
            "app_permissions": app_permissions.to_string(),
            "channel_id": "30",
            "data": {"id": "5", "name": "ping", "type": 1},
        });
        if guild {
            value["guild_id"] = "20".into();
            value["member"] = serde_json::json!({
                "roles": ["100", "101"],
                "permissions": member_permissions.to_string(),
                "joined_at": "2020-01-01T00:00:00.000000+00:00",
                "deaf": false,
                "mute": false,
                "flags": 0,
                "user": {"id": "2", "username": "u", "discriminator": "0", "avatar": null},
            });
        }
        serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}"))
    }

    #[tokio::test]
    async fn slash_permissions_come_from_discord_without_any_request() {
        let http = offline_http();
        let resolver = PermissionResolver::default();
        let interaction = slash_interaction(
            true,
            Permissions::MANAGE_MESSAGES.bits(),
            Permissions::SEND_MESSAGES.bits(),
        );
        let access = Access::for_interaction(&http, &resolver, &interaction);

        assert_eq!(
            access.permissions().await.ok(),
            Some(Permissions::MANAGE_MESSAGES)
        );
        assert_eq!(
            access.bot_permissions().await.ok(),
            Some(Permissions::SEND_MESSAGES)
        );
        assert!(
            access
                .can(Permissions::MANAGE_MESSAGES)
                .await
                .unwrap_or(false)
        );
        assert!(!access.can(Permissions::BAN_MEMBERS).await.unwrap_or(true));
        assert!(
            access
                .bot_can(Permissions::SEND_MESSAGES)
                .await
                .unwrap_or(false)
        );
        assert_eq!(
            access
                .bot_missing(Permissions::SEND_MESSAGES | Permissions::EMBED_LINKS)
                .await
                .ok(),
            Some(Permissions::EMBED_LINKS)
        );
        assert!(
            access
                .require_permissions(Permissions::MANAGE_MESSAGES)
                .await
                .is_ok()
        );
        assert!(matches!(
            access.require_permissions(Permissions::BAN_MEMBERS).await,
            Err(crate::Error::Check(CheckFailure::MissingPermissions(missing)))
                if missing == Permissions::BAN_MEMBERS
        ));
        assert!(matches!(
            access.require_bot_permissions(Permissions::EMBED_LINKS).await,
            Err(crate::Error::Check(CheckFailure::BotMissingPermissions(missing)))
                if missing == Permissions::EMBED_LINKS
        ));
    }

    #[tokio::test]
    async fn permissions_fail_closed_outside_a_guild_or_without_data() {
        let http = offline_http();
        let resolver = PermissionResolver::default();
        let dm = slash_interaction(false, 0, 0);
        let access = Access::for_interaction(&http, &resolver, &dm);
        assert!(matches!(
            access.permissions().await,
            Err(crate::Error::Check(CheckFailure::GuildOnly))
        ));
        assert!(matches!(
            access
                .require_bot_permissions(Permissions::SEND_MESSAGES)
                .await,
            Err(crate::Error::Check(CheckFailure::GuildOnly))
        ));
        assert!(matches!(
            access.require_role(Id::new(100)),
            Err(crate::Error::Check(CheckFailure::GuildOnly))
        ));
    }

    #[tokio::test]
    async fn role_checks_use_the_roles_sent_with_the_interaction() {
        let http = offline_http();
        let resolver = PermissionResolver::default();
        let interaction = slash_interaction(true, 0, 0);
        let access = Access::for_interaction(&http, &resolver, &interaction);
        assert!(access.require_role(Id::new(100)).is_ok());
        assert!(access.require_any_role(&[Id::new(7), Id::new(101)]).is_ok());
        assert!(matches!(
            access.require_role(Id::new(7)),
            Err(crate::Error::Check(CheckFailure::MissingRole))
        ));
        assert!(matches!(
            access.require_any_role(&[]),
            Err(crate::Error::Check(CheckFailure::MissingRole))
        ));
    }

    /// パスごとに決まった応答を返し、受けたリクエストを記録する簡易サーバー。
    struct Mock {
        port: u16,
        hits: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl Mock {
        async fn start(routes: Vec<(String, String)>) -> Self {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            let port = listener
                .local_addr()
                .unwrap_or_else(|error| panic!("{error}"))
                .port();
            let hits = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let record = std::sync::Arc::clone(&hits);
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    let routes = routes.clone();
                    let record = std::sync::Arc::clone(&record);
                    tokio::spawn(async move {
                        let mut stream = BufReader::new(stream);
                        let mut line = String::new();
                        if stream.read_line(&mut line).await.is_err() {
                            return;
                        }
                        let path = line
                            .split_whitespace()
                            .nth(1)
                            .unwrap_or_default()
                            .split('?')
                            .next()
                            .unwrap_or_default()
                            .to_owned();
                        loop {
                            line.clear();
                            if stream.read_line(&mut line).await.unwrap_or(0) == 0 || line == "\r\n"
                            {
                                break;
                            }
                        }
                        if let Ok(mut hits) = record.lock() {
                            hits.push(path.clone());
                        }
                        let (status, body) = routes
                            .iter()
                            .find(|(suffix, _)| path.ends_with(suffix.as_str()))
                            .map_or(("404 Not Found", "{}"), |(_, body)| ("200 OK", body));
                        let response = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.get_mut().write_all(response.as_bytes()).await;
                    });
                }
            });
            Self { port, hits }
        }

        fn client(&self) -> Client {
            Client::builder()
                .token("token".into())
                .proxy(format!("127.0.0.1:{}", self.port), true)
                .build()
        }

        fn count(&self, suffix: &str) -> usize {
            self.hits.lock().map_or(0, |hits| {
                hits.iter().filter(|path| path.ends_with(suffix)).count()
            })
        }
    }

    fn role_json(id: u64, position: i64) -> String {
        format!(
            r#"{{"id": "{id}", "name": "r", "color": 0, "colors": {{"primary_color": 0}},
            "hoist": false, "managed": false, "mentionable": false, "position": {position},
            "flags": 0, "permissions": "0"}}"#
        )
    }

    fn guild_json() -> String {
        let roles = [
            role_json(20, 0),
            role_json(100, 5),
            role_json(101, 3),
            role_json(102, 9),
        ];
        format!(
            r#"{{"id": "20", "owner_id": "1", "name": "g", "afk_timeout": 60,
            "default_message_notifications": 0, "explicit_content_filter": 0, "features": [],
            "mfa_level": 0, "nsfw_level": 0, "preferred_locale": "ja", "verification_level": 0,
            "premium_progress_bar_enabled": false, "system_channel_flags": 0,
            "roles": [{}]}}"#,
            roles.join(",")
        )
    }

    fn member_json(user: u64, roles: &[u64]) -> String {
        let roles: Vec<String> = roles.iter().map(|role| format!("\"{role}\"")).collect();
        format!(
            r#"{{"user": {{"id": "{user}", "username": "u", "discriminator": "0", "avatar": null}},
            "roles": [{}], "joined_at": "2020-01-01T00:00:00.000000+00:00",
            "deaf": false, "mute": false, "flags": 0}}"#,
            roles.join(",")
        )
    }

    async fn moderation_mock() -> Mock {
        Mock::start(vec![
            (
                "/users/@me".into(),
                r#"{"id": "3", "username": "bot", "discriminator": "0", "avatar": null, "bot": true,
                "mfa_enabled": true}"#
                    .into(),
            ),
            ("/guilds/20".into(), guild_json()),
            ("/guilds/20/members/3".into(), member_json(3, &[102])),
            ("/guilds/20/members/4".into(), member_json(4, &[101])),
            ("/guilds/20/members/5".into(), member_json(5, &[100])),
            ("/guilds/20/members/6".into(), member_json(6, &[102])),
            ("/guilds/20/members/1".into(), member_json(1, &[])),
        ])
        .await
    }

    #[tokio::test]
    async fn moderation_checks_roles_and_fetches_each_thing_once() {
        let mock = moderation_mock().await;
        let http = mock.client();
        let resolver = PermissionResolver::default();
        // 実行者（ユーザー 2）は、最上位ロール 100（位置 5）。Bot は 102（位置 9）。
        let interaction = slash_interaction(true, 0, 0);
        let access = Access::for_interaction(&http, &resolver, &interaction);

        // ロール 101（位置 3）の相手は、操作できる。
        let allowed = access.require_can_moderate(Id::new(4)).await;
        assert!(allowed.is_ok(), "{allowed:?}");
        assert_eq!(mock.count("/users/@me"), 1);
        assert_eq!(mock.count("/guilds/20"), 1);
        assert_eq!(mock.count("/guilds/20/members/3"), 1);
        assert_eq!(mock.count("/guilds/20/members/4"), 1);

        let denied = |target: u64, expected: HierarchyDenied| async move {
            assert!(
                matches!(
                    access.require_can_moderate(Id::new(target)).await,
                    Err(crate::Error::Check(CheckFailure::Hierarchy(reason))) if reason == expected
                ),
                "ユーザー {target} は {expected:?} で拒否されるはずです"
            );
        };
        // 実行者と同じ位置のロール 100 は、操作できない。
        denied(5, HierarchyDenied::AuthorTooLow).await;
        // 実行者より上のロール 102 も操作できない。
        denied(6, HierarchyDenied::AuthorTooLow).await;
        // サーバーの所有者、自分自身、Bot 自身は操作できない。
        denied(1, HierarchyDenied::TargetIsOwner).await;
        denied(2, HierarchyDenied::TargetIsSelf).await;
        denied(3, HierarchyDenied::TargetIsBot).await;
        // Bot の ID は 1 回取得した値を使い回し、自分自身と Bot の拒否ではメンバーを取得しない。
        assert_eq!(mock.count("/users/@me"), 1);
        assert_eq!(mock.count("/guilds/20/members/2"), 0);
    }

    #[tokio::test]
    async fn guild_owner_check_uses_the_guild_owner_id() {
        let mock = moderation_mock().await;
        let http = mock.client();
        let resolver = PermissionResolver::default();
        let interaction = slash_interaction(true, 0, 0);
        let access = Access::for_interaction(&http, &resolver, &interaction);
        assert!(matches!(
            access.require_guild_owner().await,
            Err(crate::Error::Check(CheckFailure::NotGuildOwner))
        ));
        assert_eq!(mock.count("/guilds/20"), 1);
    }

    #[tokio::test]
    async fn moderation_fails_closed_when_the_target_cannot_be_fetched() {
        let mock = moderation_mock().await;
        let http = mock.client();
        let resolver = PermissionResolver::default();
        let interaction = slash_interaction(true, 0, 0);
        let access = Access::for_interaction(&http, &resolver, &interaction);
        // サーバーにいないユーザーは、メンバーを取得できず、許可されない。
        assert!(access.require_can_moderate(Id::new(99)).await.is_err());
    }
}
