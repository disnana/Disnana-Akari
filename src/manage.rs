//! メンバー、ロール、VC の管理操作。
//!
//! どの操作も、Discord に送る前に、Bot の権限とロールの階層を確認し、足りなければ通信せずに失敗する。
//! **実行した人への確認は、しない。** 呼ぶ前に、`Access::require_permissions` や
//! `Access::require_can_moderate` で確認する。1 つの操作は、Discord への 1 回のリクエスト。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use twilight_http::{error::ErrorType, request::AuditLogReason};
use twilight_model::{
    guild::Permissions,
    id::{
        Id,
        marker::{ChannelMarker, GuildMarker, RoleMarker, UserMarker},
    },
    util::datetime::Timestamp,
};

use crate::{Access, CheckFailure, Error, HierarchyDenied, InvalidArgument, Result};

/// タイムアウトの最長（28 日）。Discord の上限。
pub const MAX_TIMEOUT: Duration = Duration::from_secs(28 * 24 * 60 * 60);

/// 参加者の BAN で、一緒に削除できるメッセージの最長（7 日）。Discord の上限。
pub const MAX_BAN_MESSAGE_DELETE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// メンバー、ロール、VC を管理する操作です。
///
/// `ctx.manage()` で取得します。どの操作も、Discord に送る前に、Bot の権限とロールの階層を確認し、
/// 足りなければ通信せずに失敗します。**実行した人が操作してよいかは、確認しません。**
/// 呼ぶ前に [`Access::require_permissions`] と [`Access::require_can_moderate`] で確認してください。
/// `reason` は監査ログに残る理由で、512 文字以内です。
#[derive(Clone, Copy, Debug)]
pub struct Manage<'a> {
    access: Access<'a>,
}

impl<'a> Manage<'a> {
    pub(crate) fn new(access: Access<'a>) -> Self {
        Self { access }
    }

    /// 同じ呼び出しの [`Access`] を返します。
    pub fn access(&self) -> Access<'a> {
        self.access
    }

    /// Bot に必要な権限があり、対象のメンバーを操作できることを確認する。
    /// 対象がサーバーにいない場合（`allow_absent`）は、階層の確認を省く。
    async fn prepare_member(
        &self,
        target: Id<UserMarker>,
        required: Permissions,
        allow_absent: bool,
    ) -> Result<Id<GuildMarker>> {
        let guild_id = self.access.require_guild()?;
        self.access.require_bot_permissions(required).await?;
        match self.access.require_bot_can_moderate(target).await {
            Err(Error::Http(error))
                if allow_absent
                    && matches!(
                        error.kind(),
                        ErrorType::Response { status, .. } if status.get() == 404
                    ) => {}
            other => other?,
        }
        Ok(guild_id)
    }

    /// メンバーをサーバーからキックします。必要な Bot の権限: `KICK_MEMBERS`。
    ///
    /// # エラー
    ///
    /// Bot の権限が足りないときは [`CheckFailure::BotMissingPermissions`]、ロールの階層で操作できない
    /// ときは [`CheckFailure::Hierarchy`]、そのほかは HTTP のエラーを返します。
    pub async fn kick_member(&self, user_id: Id<UserMarker>, reason: &str) -> Result<()> {
        let guild_id = self
            .prepare_member(user_id, Permissions::KICK_MEMBERS, false)
            .await?;
        self.access
            .http()
            .remove_guild_member(guild_id, user_id)
            .reason(reason)
            .await?;
        Ok(())
    }

    /// ユーザーを BAN します。サーバーにいないユーザーも BAN できます。
    /// 必要な Bot の権限: `BAN_MEMBERS`。`delete_messages` は、一緒に削除する最近のメッセージの期間で、
    /// 7 日以内です。
    ///
    /// # エラー
    ///
    /// `delete_messages` が 7 日を超えるときは、通信せずに [`InvalidArgument::BanMessageWindowTooLong`] を
    /// 返します。そのほかは [`kick_member`](Self::kick_member) と同じです。
    pub async fn ban_member(
        &self,
        user_id: Id<UserMarker>,
        delete_messages: Duration,
        reason: &str,
    ) -> Result<()> {
        if delete_messages > MAX_BAN_MESSAGE_DELETE {
            return Err(InvalidArgument::BanMessageWindowTooLong.into());
        }
        let guild_id = self
            .prepare_member(user_id, Permissions::BAN_MEMBERS, true)
            .await?;
        let seconds = u32::try_from(delete_messages.as_secs()).unwrap_or(u32::MAX);
        self.access
            .http()
            .create_ban(guild_id, user_id)
            .delete_message_seconds(seconds)
            .reason(reason)
            .await?;
        Ok(())
    }

    /// BAN を解除します。必要な Bot の権限: `BAN_MEMBERS`。
    ///
    /// # エラー
    ///
    /// Bot の権限が足りないときは [`CheckFailure::BotMissingPermissions`]、そのほかは HTTP のエラーを返します。
    pub async fn unban_member(&self, user_id: Id<UserMarker>, reason: &str) -> Result<()> {
        let guild_id = self.access.require_guild()?;
        self.access
            .require_bot_permissions(Permissions::BAN_MEMBERS)
            .await?;
        self.access
            .http()
            .delete_ban(guild_id, user_id)
            .reason(reason)
            .await?;
        Ok(())
    }

    /// メンバーを、指定した期間だけタイムアウトします（28 日以内、ちょうど 0 は解除）。
    /// 必要な Bot の権限: `MODERATE_MEMBERS`。
    ///
    /// # エラー
    ///
    /// 期間が 28 日を超えるときは、通信せずに [`InvalidArgument::TimeoutTooLong`] を返します。
    /// そのほかは [`kick_member`](Self::kick_member) と同じです。
    pub async fn timeout_member(
        &self,
        user_id: Id<UserMarker>,
        duration: Duration,
        reason: &str,
    ) -> Result<()> {
        if duration > MAX_TIMEOUT {
            return Err(InvalidArgument::TimeoutTooLong.into());
        }
        let until = if duration.is_zero() {
            None
        } else {
            Some(timestamp_after(duration)?)
        };
        let guild_id = self
            .prepare_member(user_id, Permissions::MODERATE_MEMBERS, false)
            .await?;
        self.access
            .http()
            .update_guild_member(guild_id, user_id)
            .communication_disabled_until(until)
            .reason(reason)
            .await?;
        Ok(())
    }

    /// メンバーのニックネームを変更します（`None` で解除）。必要な Bot の権限: `MANAGE_NICKNAMES`。
    ///
    /// # エラー
    ///
    /// [`kick_member`](Self::kick_member) と同じです。ニックネームが 32 文字を超えるときは、
    /// Twilight の検証により、通信せずに HTTP のエラーになります。
    pub async fn set_nickname(
        &self,
        user_id: Id<UserMarker>,
        nickname: Option<&str>,
        reason: &str,
    ) -> Result<()> {
        let guild_id = self
            .prepare_member(user_id, Permissions::MANAGE_NICKNAMES, false)
            .await?;
        self.access
            .http()
            .update_guild_member(guild_id, user_id)
            .nick(nickname)
            .reason(reason)
            .await?;
        Ok(())
    }

    /// メンバーにロールを付けます。必要な Bot の権限: `MANAGE_ROLES`。ロールは、Bot の最上位ロールより
    /// 低い必要があります。
    ///
    /// # エラー
    ///
    /// [`kick_member`](Self::kick_member) と同じです。ロールが Bot の最上位ロール以上のときは
    /// [`HierarchyDenied::BotTooLow`] です。
    pub async fn add_role(
        &self,
        user_id: Id<UserMarker>,
        role_id: Id<RoleMarker>,
        reason: &str,
    ) -> Result<()> {
        let guild_id = self.prepare_role_change(user_id, role_id).await?;
        self.access
            .http()
            .add_guild_member_role(guild_id, user_id, role_id)
            .reason(reason)
            .await?;
        Ok(())
    }

    /// メンバーからロールを外します。必要な Bot の権限とロールの位置は、[`add_role`](Self::add_role) と同じです。
    ///
    /// # エラー
    ///
    /// [`add_role`](Self::add_role) と同じです。
    pub async fn remove_role(
        &self,
        user_id: Id<UserMarker>,
        role_id: Id<RoleMarker>,
        reason: &str,
    ) -> Result<()> {
        let guild_id = self.prepare_role_change(user_id, role_id).await?;
        self.access
            .http()
            .remove_guild_member_role(guild_id, user_id, role_id)
            .reason(reason)
            .await?;
        Ok(())
    }

    async fn prepare_role_change(
        &self,
        user_id: Id<UserMarker>,
        role_id: Id<RoleMarker>,
    ) -> Result<Id<GuildMarker>> {
        let guild_id = self
            .prepare_member(user_id, Permissions::MANAGE_ROLES, false)
            .await?;
        if !self.access.bot_outranks_role(role_id).await? {
            return Err(CheckFailure::Hierarchy(HierarchyDenied::BotTooLow).into());
        }
        Ok(guild_id)
    }

    /// メンバーを、別のボイスチャンネルに移動します（`None` で切断）。必要な Bot の権限: `MOVE_MEMBERS`
    /// （切断も同じ）。メンバーは、ボイスチャンネルにいる必要があります。
    ///
    /// # エラー
    ///
    /// Bot の権限が足りないときは [`CheckFailure::BotMissingPermissions`]、そのほかは HTTP のエラーを返します。
    pub async fn move_member(
        &self,
        user_id: Id<UserMarker>,
        channel_id: Option<Id<ChannelMarker>>,
        reason: &str,
    ) -> Result<()> {
        let guild_id = self.access.require_guild()?;
        self.access
            .require_bot_permissions(Permissions::MOVE_MEMBERS)
            .await?;
        self.access
            .http()
            .update_guild_member(guild_id, user_id)
            .channel_id(channel_id)
            .reason(reason)
            .await?;
        Ok(())
    }

    /// メンバーを、ボイスチャンネルでミュートします（`false` で解除）。必要な Bot の権限: `MUTE_MEMBERS`。
    ///
    /// # エラー
    ///
    /// [`move_member`](Self::move_member) と同じです。
    pub async fn set_voice_mute(
        &self,
        user_id: Id<UserMarker>,
        mute: bool,
        reason: &str,
    ) -> Result<()> {
        let guild_id = self.access.require_guild()?;
        self.access
            .require_bot_permissions(Permissions::MUTE_MEMBERS)
            .await?;
        self.access
            .http()
            .update_guild_member(guild_id, user_id)
            .mute(mute)
            .reason(reason)
            .await?;
        Ok(())
    }

    /// メンバーのスピーカーを、ボイスチャンネルでミュートします（`false` で解除）。
    /// 必要な Bot の権限: `DEAFEN_MEMBERS`。
    ///
    /// # エラー
    ///
    /// [`move_member`](Self::move_member) と同じです。
    pub async fn set_voice_deafen(
        &self,
        user_id: Id<UserMarker>,
        deafen: bool,
        reason: &str,
    ) -> Result<()> {
        let guild_id = self.access.require_guild()?;
        self.access
            .require_bot_permissions(Permissions::DEAFEN_MEMBERS)
            .await?;
        self.access
            .http()
            .update_guild_member(guild_id, user_id)
            .deaf(deafen)
            .reason(reason)
            .await?;
        Ok(())
    }
}

/// いまから `duration` 後の時刻。
fn timestamp_after(duration: Duration) -> Result<Timestamp> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(crate::Error::handler)?;
    let seconds = i64::try_from((now + duration).as_secs()).map_err(crate::Error::handler)?;
    Timestamp::from_secs(seconds).map_err(crate::Error::handler)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Access,
        access::tests::{Mock, guild_json_with, member_json, offline_http, slash_interaction},
        permissions::PermissionResolver,
    };

    fn user_json(id: u64) -> String {
        format!(
            r#"{{"id": "{id}", "username": "u", "discriminator": "0", "avatar": null, "bot": true, "mfa_enabled": true}}"#
        )
    }

    /// 実行者は 2（最上位ロール 100）、Bot は 3（ロール 102、位置 9）、所有者は 1。
    /// 相手のメンバー: 4（ロール 101、位置 3）、5（ロール 102、Bot と同位）、1（所有者）。
    async fn mock(bot_role: Permissions) -> Mock {
        Mock::start(vec![
            ("/users/@me".into(), user_json(3)),
            ("/guilds/20".into(), guild_json_with(bot_role)),
            ("/guilds/20/members/3".into(), member_json(3, &[102])),
            ("/guilds/20/members/4".into(), member_json(4, &[101])),
            ("/guilds/20/members/5".into(), member_json(5, &[102])),
            ("/guilds/20/members/1".into(), member_json(1, &[])),
            // 書き込み系は、空の応答を返す。
            ("/guilds/20/bans/4".into(), "{}".into()),
            ("/guilds/20/bans/99".into(), "{}".into()),
            ("/guilds/20/bans/1".into(), "{}".into()),
            ("/guilds/20/members/4/roles/101".into(), "{}".into()),
        ])
        .await
    }

    fn manage<'a>(
        http: &'a twilight_http::Client,
        resolver: &'a PermissionResolver,
        interaction: &'a twilight_model::application::interaction::Interaction,
    ) -> Manage<'a> {
        Manage::new(Access::for_interaction(http, resolver, interaction))
    }

    fn user(id: u64) -> Id<UserMarker> {
        Id::new(id)
    }

    fn is_bot_missing(result: &Result<()>, expected: Permissions) -> bool {
        matches!(
            result,
            Err(Error::Check(CheckFailure::BotMissingPermissions(missing))) if *missing == expected
        )
    }

    fn is_hierarchy(result: &Result<()>, expected: HierarchyDenied) -> bool {
        matches!(
            result,
            Err(Error::Check(CheckFailure::Hierarchy(reason))) if *reason == expected
        )
    }

    #[tokio::test]
    async fn kick_checks_the_bots_permission_and_hierarchy_before_sending() {
        let mock = mock(Permissions::empty()).await;
        let http = mock.client();
        let resolver = PermissionResolver::default();

        // Bot が KICK_MEMBERS を持たなければ、通信せずに断る。
        let interaction = slash_interaction(true, 0, Permissions::empty().bits());
        let manage_without = manage(&http, &resolver, &interaction);
        let denied = manage_without.kick_member(user(4), "理由").await;
        assert!(
            is_bot_missing(&denied, Permissions::KICK_MEMBERS),
            "{denied:?}"
        );
        assert_eq!(mock.count("DELETE /guilds/20/members/4"), 0);

        let interaction = slash_interaction(true, 0, Permissions::KICK_MEMBERS.bits());
        let manage = manage(&http, &resolver, &interaction);
        // Bot より低いロールの相手は、キックできる。
        assert!(manage.kick_member(user(4), "理由").await.is_ok());
        assert_eq!(mock.count("DELETE /guilds/20/members/4"), 1);
        // Bot と同位のロール、所有者、Bot 自身は、送らずに断る。
        let same = manage.kick_member(user(5), "理由").await;
        assert!(is_hierarchy(&same, HierarchyDenied::BotTooLow), "{same:?}");
        let owner = manage.kick_member(user(1), "理由").await;
        assert!(
            is_hierarchy(&owner, HierarchyDenied::TargetIsOwner),
            "{owner:?}"
        );
        let itself = manage.kick_member(user(3), "理由").await;
        assert!(
            is_hierarchy(&itself, HierarchyDenied::TargetIsBot),
            "{itself:?}"
        );
        // 断った相手（5、1、3）には、送られていない。
        for refused in [5, 1, 3] {
            assert_eq!(
                mock.count(&format!("DELETE /guilds/20/members/{refused}")),
                0
            );
        }
    }

    #[tokio::test]
    async fn ban_allows_users_who_left_and_limits_the_message_window() {
        let mock = mock(Permissions::empty()).await;
        let http = mock.client();
        let resolver = PermissionResolver::default();
        let interaction = slash_interaction(true, 0, Permissions::BAN_MEMBERS.bits());
        let manage = manage(&http, &resolver, &interaction);

        assert!(
            manage
                .ban_member(user(4), Duration::from_secs(3600), "理由")
                .await
                .is_ok()
        );
        let body = mock
            .body_of("PUT /guilds/20/bans/4")
            .unwrap_or_else(|| panic!("本文がありません"));
        assert_eq!(body["delete_message_seconds"], 3600);

        // サーバーにいないユーザー（メンバーが 404）は、階層の確認を省いて BAN できる。
        assert!(
            manage
                .ban_member(user(99), Duration::ZERO, "理由")
                .await
                .is_ok()
        );
        assert_eq!(mock.count("PUT /guilds/20/bans/99"), 1);

        // 所有者は BAN できない。
        let owner = manage.ban_member(user(1), Duration::ZERO, "理由").await;
        assert!(
            is_hierarchy(&owner, HierarchyDenied::TargetIsOwner),
            "{owner:?}"
        );

        // 8 日分の削除は、通信せずに断る。
        let long = manage
            .ban_member(user(4), Duration::from_secs(8 * 24 * 3600), "理由")
            .await;
        assert!(matches!(
            long,
            Err(Error::InvalidArgument(
                InvalidArgument::BanMessageWindowTooLong
            ))
        ));
        // 断った 2 件（所有者と 8 日分）は送られていない。
        assert_eq!(mock.count("PUT /guilds/20/bans/4"), 1);
        assert_eq!(mock.count("PUT /guilds/20/bans/1"), 0);
    }

    #[tokio::test]
    async fn unban_needs_only_the_bots_permission() {
        let mock = mock(Permissions::empty()).await;
        let http = mock.client();
        let resolver = PermissionResolver::default();
        let none = slash_interaction(true, 0, 0);
        let denied = manage(&http, &resolver, &none)
            .unban_member(user(4), "理由")
            .await;
        assert!(
            is_bot_missing(&denied, Permissions::BAN_MEMBERS),
            "{denied:?}"
        );
        let ok = slash_interaction(true, 0, Permissions::BAN_MEMBERS.bits());
        assert!(
            manage(&http, &resolver, &ok)
                .unban_member(user(4), "理由")
                .await
                .is_ok()
        );
        assert_eq!(mock.count("DELETE /guilds/20/bans/4"), 1);
    }

    #[tokio::test]
    async fn timeout_is_limited_to_28_days_and_zero_clears_it() {
        let mock = mock(Permissions::empty()).await;
        let http = mock.client();
        let resolver = PermissionResolver::default();
        let interaction = slash_interaction(true, 0, Permissions::MODERATE_MEMBERS.bits());
        let manage = manage(&http, &resolver, &interaction);

        let too_long = manage
            .timeout_member(user(4), MAX_TIMEOUT + Duration::from_secs(1), "理由")
            .await;
        assert!(matches!(
            too_long,
            Err(Error::InvalidArgument(InvalidArgument::TimeoutTooLong))
        ));
        assert_eq!(mock.count("PATCH /guilds/20/members/4"), 0);

        assert!(
            manage
                .timeout_member(user(4), Duration::from_secs(600), "理由")
                .await
                .is_ok()
        );
        let body = mock
            .body_of("PATCH /guilds/20/members/4")
            .unwrap_or_else(|| panic!("本文がありません"));
        assert!(body["communication_disabled_until"].is_string(), "{body}");

        assert!(
            manage
                .timeout_member(user(4), Duration::ZERO, "理由")
                .await
                .is_ok()
        );
        assert_eq!(mock.count("PATCH /guilds/20/members/4"), 2);
        let denied = manage
            .timeout_member(user(5), Duration::from_secs(60), "理由")
            .await;
        assert!(
            is_hierarchy(&denied, HierarchyDenied::BotTooLow),
            "{denied:?}"
        );
    }

    #[tokio::test]
    async fn roles_must_be_below_the_bots_top_role() {
        let mock = mock(Permissions::empty()).await;
        let http = mock.client();
        let resolver = PermissionResolver::default();
        let interaction = slash_interaction(true, 0, Permissions::MANAGE_ROLES.bits());
        let manage = manage(&http, &resolver, &interaction);

        // ロール 101（位置 3）は、Bot（位置 9）より低いので、付け外しできる。
        assert!(manage.add_role(user(4), Id::new(101), "理由").await.is_ok());
        assert_eq!(mock.count("PUT /guilds/20/members/4/roles/101"), 1);
        assert!(
            manage
                .remove_role(user(4), Id::new(101), "理由")
                .await
                .is_ok()
        );
        assert_eq!(mock.count("DELETE /guilds/20/members/4/roles/101"), 1);
        // Bot と同位のロール 102 と、存在しないロールは、送らずに断る。
        for role in [102, 999] {
            let denied = manage.add_role(user(4), Id::new(role), "理由").await;
            assert!(
                is_hierarchy(&denied, HierarchyDenied::BotTooLow),
                "{role}: {denied:?}"
            );
        }
        assert_eq!(mock.count("PUT /guilds/20/members/4/roles/102"), 0);
        assert_eq!(mock.count("PUT /guilds/20/members/4/roles/999"), 0);
    }

    #[tokio::test]
    async fn nickname_and_voice_operations_send_one_request_each() {
        let mock = mock(Permissions::empty()).await;
        let http = mock.client();
        let resolver = PermissionResolver::default();
        let all = Permissions::MANAGE_NICKNAMES
            | Permissions::MOVE_MEMBERS
            | Permissions::MUTE_MEMBERS
            | Permissions::DEAFEN_MEMBERS;
        let interaction = slash_interaction(true, 0, all.bits());
        let manage = manage(&http, &resolver, &interaction);

        assert!(
            manage
                .set_nickname(user(4), Some("新しい名前"), "理由")
                .await
                .is_ok()
        );
        assert!(
            manage
                .move_member(user(4), Some(Id::new(55)), "理由")
                .await
                .is_ok()
        );
        assert!(manage.move_member(user(4), None, "理由").await.is_ok());
        assert!(manage.set_voice_mute(user(4), true, "理由").await.is_ok());
        assert!(manage.set_voice_deafen(user(4), true, "理由").await.is_ok());
        assert_eq!(mock.count("PATCH /guilds/20/members/4"), 5);

        // 権限が足りなければ、送らずに断る。
        let weak = slash_interaction(true, 0, Permissions::MOVE_MEMBERS.bits());
        let weak = Manage::new(Access::for_interaction(&http, &resolver, &weak));
        let denied = weak.set_voice_mute(user(4), true, "理由").await;
        assert!(
            is_bot_missing(&denied, Permissions::MUTE_MEMBERS),
            "{denied:?}"
        );
        assert_eq!(mock.count("PATCH /guilds/20/members/4"), 5);
    }

    #[tokio::test]
    async fn everything_fails_closed_outside_a_guild() {
        let http = offline_http();
        let resolver = PermissionResolver::default();
        let dm = slash_interaction(false, 0, 0);
        let manage = manage(&http, &resolver, &dm);
        assert!(matches!(
            manage.kick_member(user(4), "理由").await,
            Err(Error::Check(CheckFailure::GuildOnly))
        ));
        assert!(matches!(
            manage.set_voice_mute(user(4), true, "理由").await,
            Err(Error::Check(CheckFailure::GuildOnly))
        ));
    }
}
