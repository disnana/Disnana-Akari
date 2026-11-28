//! メンバー、ロール、VC の管理操作。
//!
//! 操作ごとに、事前に判定できる Bot の権限やロールの階層を確認する。VC 操作は Discord で権限を判定する。
//! **実行した人への確認は、しない。** 呼ぶ前に、`Access::require_permissions` や
//! `Access::require_can_moderate` で確認する。1 つの操作は、Discord への 1 回のリクエスト。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use twilight_http::request::AuditLogReason;
use twilight_model::{
    channel::{Channel, ChannelType},
    guild::{Permissions, Role},
    id::{
        Id,
        marker::{ChannelMarker, GuildMarker, RoleMarker, UserMarker},
    },
    util::datetime::Timestamp,
};

use crate::{
    Access, CheckFailure, HierarchyDenied, InvalidArgument, NewCategory, NewRole, NewTextChannel,
    NewVoiceChannel, Overwrite, OverwriteTarget, Result, RoleEdit, TextChannelEdit,
    VoiceChannelEdit, access::check_bot_can_grant,
};

/// タイムアウトの最長（28 日）。Discord の上限。
pub const MAX_TIMEOUT: Duration = Duration::from_secs(28 * 24 * 60 * 60);

/// 参加者の BAN で、一緒に削除できるメッセージの最長（7 日）。Discord の上限。
pub const MAX_BAN_MESSAGE_DELETE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// メンバー、ロール、VC を管理する操作です。
///
/// `ctx.manage()` で取得します。操作ごとに、Discord へ変更を送る前に確認できる権限や階層をチェックします。
/// VC の移動・ミュートなどは、Discord が対象の VC に対する権限を判定します。
/// **実行した人が操作してよいかは、確認しません。**
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
        self.access
            .require_bot_can_moderate(target, allow_absent)
            .await?;
        Ok(guild_id)
    }

    /// 作成するチャンネルの権限の上書きに必要な、Bot の権限。
    fn overwrite_requirements(overwrites: &[(OverwriteTarget, Overwrite)]) -> Permissions {
        overwrites
            .iter()
            .fold(Permissions::empty(), |required, (_, overwrite)| {
                required | overwrite.allowed() | overwrite.denied()
            })
    }

    async fn check_creating_channel(
        &self,
        overwrites: &[(OverwriteTarget, Overwrite)],
    ) -> Result<Id<GuildMarker>> {
        let guild_id = self.access.require_guild()?;
        let held = self.access.bot_guild_permissions().await?;
        let overwrites = Self::overwrite_requirements(overwrites);
        let mut required = Permissions::MANAGE_CHANNELS | overwrites;
        if overwrites.contains(Permissions::MANAGE_ROLES) {
            required |= Permissions::ADMINISTRATOR;
        }
        check_bot_can_grant(held, required)?;
        Ok(guild_id)
    }

    /// テキストチャンネルを作成します。必要な Bot の権限: `MANAGE_CHANNELS`
    /// （上書きする権限も必要。`MANAGE_ROLES` 自体を上書きする場合は `ADMINISTRATOR` が必要）。
    ///
    /// # エラー
    ///
    /// 名前やトピックなどが Discord の範囲に入っていないときは、通信せずに検証のエラーを返します。
    /// Bot の権限が足りないときは [`CheckFailure::BotMissingPermissions`]、送信に失敗したときは
    /// HTTP のエラーを返します。
    pub async fn create_text_channel(&self, new: NewTextChannel, reason: &str) -> Result<Channel> {
        new.validate()?;
        let guild_id = self.check_creating_channel(new.overwrites()).await?;
        let overwrites: Vec<_> = new
            .overwrites()
            .iter()
            .map(|(target, overwrite)| overwrite.into_model(*target))
            .collect();
        let mut request = self
            .access
            .http()
            .create_guild_channel(guild_id, new.name())
            .kind(ChannelType::GuildText)
            .nsfw(new.nsfw_value())
            .permission_overwrites(&overwrites);
        if let Some(topic) = new.topic_value() {
            request = request.topic(topic);
        }
        if let Some(seconds) = new.slowmode_value() {
            request = request.rate_limit_per_user(seconds);
        }
        if let Some(category) = new.category_value() {
            request = request.parent_id(category);
        }
        Ok(request.reason(reason).await?.model().await?)
    }

    /// ボイスチャンネルを作成します。必要な Bot の権限は、[`create_text_channel`](Self::create_text_channel) と同じです。
    ///
    /// # エラー
    ///
    /// [`create_text_channel`](Self::create_text_channel) と同じです。
    pub async fn create_voice_channel(
        &self,
        new: NewVoiceChannel,
        reason: &str,
    ) -> Result<Channel> {
        new.validate()?;
        let guild_id = self.check_creating_channel(new.overwrites()).await?;
        let overwrites: Vec<_> = new
            .overwrites()
            .iter()
            .map(|(target, overwrite)| overwrite.into_model(*target))
            .collect();
        let mut request = self
            .access
            .http()
            .create_guild_channel(guild_id, new.name())
            .kind(ChannelType::GuildVoice)
            .permission_overwrites(&overwrites);
        if let Some(bitrate) = new.bitrate_value() {
            request = request.bitrate(bitrate);
        }
        if let Some(limit) = new.user_limit_value() {
            request = request.user_limit(limit);
        }
        if let Some(category) = new.category_value() {
            request = request.parent_id(category);
        }
        Ok(request.reason(reason).await?.model().await?)
    }

    /// カテゴリーを作成します。必要な Bot の権限は、[`create_text_channel`](Self::create_text_channel) と同じです。
    ///
    /// # エラー
    ///
    /// [`create_text_channel`](Self::create_text_channel) と同じです。
    pub async fn create_category(&self, new: NewCategory, reason: &str) -> Result<Channel> {
        new.validate()?;
        let guild_id = self.check_creating_channel(new.overwrites()).await?;
        let overwrites: Vec<_> = new
            .overwrites()
            .iter()
            .map(|(target, overwrite)| overwrite.into_model(*target))
            .collect();
        let request = self
            .access
            .http()
            .create_guild_channel(guild_id, new.name())
            .kind(ChannelType::GuildCategory)
            .permission_overwrites(&overwrites);
        Ok(request.reason(reason).await?.model().await?)
    }

    async fn check_managing_channel(
        &self,
        channel_id: Id<ChannelMarker>,
        required: Permissions,
    ) -> Result<()> {
        let guild_id = self.access.require_guild()?;
        let held = self.access.bot_permissions_in(guild_id, channel_id).await?;
        check_bot_can_grant(held, required)?;
        Ok(())
    }

    /// テキストチャンネルを編集します。指定した項目だけを変更します。
    /// 必要な Bot の権限: そのチャンネルでの `MANAGE_CHANNELS`。
    ///
    /// # エラー
    ///
    /// 何も指定していないときは [`InvalidArgument::NothingToChange`]、範囲外の値は検証のエラー、
    /// 種類が合わないときは [`InvalidArgument::WrongChannelKind`]、Bot の権限が足りないときは
    /// [`CheckFailure::BotMissingPermissions`] を返します。送信に失敗したときは HTTP のエラーです。
    pub async fn edit_text_channel(
        &self,
        channel_id: Id<ChannelMarker>,
        edit: TextChannelEdit,
        reason: &str,
    ) -> Result<Channel> {
        if edit.is_empty() {
            return Err(InvalidArgument::NothingToChange.into());
        }
        edit.validate()?;
        if !matches!(
            self.access.managed_channel_kind(channel_id).await?,
            ChannelType::GuildText | ChannelType::GuildAnnouncement
        ) {
            return Err(InvalidArgument::WrongChannelKind.into());
        }
        self.check_managing_channel(channel_id, Permissions::MANAGE_CHANNELS)
            .await?;
        let mut request = self.access.http().update_channel(channel_id);
        if let Some(name) = edit.name_value() {
            request = request.name(name);
        }
        if let Some(topic) = edit.topic_value() {
            request = request.topic(topic);
        }
        if let Some(nsfw) = edit.nsfw_value() {
            request = request.nsfw(nsfw);
        }
        if let Some(seconds) = edit.slowmode_value() {
            request = request.rate_limit_per_user(seconds);
        }
        if let Some(category) = edit.category_value() {
            request = request.parent_id(category);
        }
        Ok(request.reason(reason).await?.model().await?)
    }

    /// ボイスチャンネルを編集します。指定した項目だけを変更します。
    /// 必要な Bot の権限: そのチャンネルでの `MANAGE_CHANNELS`。
    ///
    /// # エラー
    ///
    /// [`edit_text_channel`](Self::edit_text_channel) と同じです。
    pub async fn edit_voice_channel(
        &self,
        channel_id: Id<ChannelMarker>,
        edit: VoiceChannelEdit,
        reason: &str,
    ) -> Result<Channel> {
        if edit.is_empty() {
            return Err(InvalidArgument::NothingToChange.into());
        }
        edit.validate()?;
        if self.access.managed_channel_kind(channel_id).await? != ChannelType::GuildVoice {
            return Err(InvalidArgument::WrongChannelKind.into());
        }
        self.check_managing_channel(channel_id, Permissions::MANAGE_CHANNELS)
            .await?;
        let mut request = self.access.http().update_channel(channel_id);
        if let Some(name) = edit.name_value() {
            request = request.name(name);
        }
        if let Some(bitrate) = edit.bitrate_value() {
            request = request.bitrate(bitrate);
        }
        if let Some(limit) = edit.user_limit_value() {
            request = request.user_limit(limit);
        }
        if let Some(category) = edit.category_value() {
            request = request.parent_id(category);
        }
        Ok(request.reason(reason).await?.model().await?)
    }

    /// チャンネルを削除します。元に戻せません。Bot には通常 `MANAGE_CHANNELS`、
    /// スレッドでは `MANAGE_THREADS` が必要です。
    ///
    /// # エラー
    ///
    /// Bot の権限が足りないときは [`CheckFailure::BotMissingPermissions`]、そのほかは HTTP のエラーを返します。
    pub async fn delete_channel(&self, channel_id: Id<ChannelMarker>, reason: &str) -> Result<()> {
        let kind = self.access.managed_channel_kind(channel_id).await?;
        let required = if kind.is_thread() {
            Permissions::MANAGE_THREADS
        } else {
            Permissions::MANAGE_CHANNELS
        };
        self.check_managing_channel(channel_id, required).await?;
        self.access
            .http()
            .delete_channel(channel_id)
            .reason(reason)
            .await?;
        Ok(())
    }

    /// ロールを作成します。必要な Bot の権限: `MANAGE_ROLES`。付ける権限は、Bot が持つものだけです
    /// （管理者は除く）。
    ///
    /// # エラー
    ///
    /// Bot の権限が足りないときは [`CheckFailure::BotMissingPermissions`]、送信に失敗したときは
    /// HTTP のエラーを返します。
    pub async fn create_role(&self, new: NewRole, reason: &str) -> Result<Role> {
        let guild_id = self.access.require_guild()?;
        let held = self.access.bot_guild_permissions().await?;
        check_bot_can_grant(held, Permissions::MANAGE_ROLES | new.permissions_value())?;
        let mut request = self
            .access
            .http()
            .create_role(guild_id)
            .name(new.name())
            .hoist(new.hoist_value())
            .mentionable(new.mentionable_value())
            .permissions(new.permissions_value());
        if let Some(color) = new.color_value() {
            request = request.color(color);
        }
        Ok(request.reason(reason).await?.model().await?)
    }

    /// ロールを編集します。指定した項目だけを変更します。必要な Bot の権限: `MANAGE_ROLES`。
    /// ロールは Bot の最上位ロールより低い必要があり、権限は Bot が持つものだけを付けられます。
    ///
    /// # エラー
    ///
    /// 何も指定していないときは [`InvalidArgument::NothingToChange`]、Bot の権限が足りないときは
    /// [`CheckFailure::BotMissingPermissions`]、ロールの位置が足りないか、ロールが存在しないときは
    /// [`CheckFailure::Hierarchy`] を返します。いずれも通信する前に返ります。
    pub async fn edit_role(
        &self,
        role_id: Id<RoleMarker>,
        edit: RoleEdit,
        reason: &str,
    ) -> Result<Role> {
        if edit.is_empty() {
            return Err(InvalidArgument::NothingToChange.into());
        }
        let guild_id = self.access.require_guild()?;
        let (held, outranks) = self.access.bot_role_standing(role_id).await?;
        let wanted = edit.permissions_value().unwrap_or_else(Permissions::empty);
        check_bot_can_grant(held, Permissions::MANAGE_ROLES | wanted)?;
        if !outranks {
            return Err(CheckFailure::Hierarchy(HierarchyDenied::BotTooLow).into());
        }
        let mut request = self.access.http().update_role(guild_id, role_id);
        if let Some(name) = edit.name_value() {
            request = request.name(Some(name));
        }
        if let Some(color) = edit.color_value() {
            request = request.color(color);
        }
        if let Some(hoist) = edit.hoist_value() {
            request = request.hoist(hoist);
        }
        if let Some(mentionable) = edit.mentionable_value() {
            request = request.mentionable(mentionable);
        }
        if let Some(permissions) = edit.permissions_value() {
            request = request.permissions(permissions);
        }
        Ok(request.reason(reason).await?.model().await?)
    }

    /// ロールを削除します。元に戻せません。必要な Bot の権限: `MANAGE_ROLES`。
    /// ロールは Bot の最上位ロールより低い必要があります。
    ///
    /// # エラー
    ///
    /// [`edit_role`](Self::edit_role) と同じです。
    pub async fn delete_role(&self, role_id: Id<RoleMarker>, reason: &str) -> Result<()> {
        let guild_id = self.access.require_guild()?;
        let (held, outranks) = self.access.bot_role_standing(role_id).await?;
        check_bot_can_grant(held, Permissions::MANAGE_ROLES)?;
        if !outranks {
            return Err(CheckFailure::Hierarchy(HierarchyDenied::BotTooLow).into());
        }
        self.access
            .http()
            .delete_role(guild_id, role_id)
            .reason(reason)
            .await?;
        Ok(())
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
        let guild_id = self.prepare_role_change(role_id).await?;
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
        let guild_id = self.prepare_role_change(role_id).await?;
        self.access
            .http()
            .remove_guild_member_role(guild_id, user_id, role_id)
            .reason(reason)
            .await?;
        Ok(())
    }

    async fn prepare_role_change(&self, role_id: Id<RoleMarker>) -> Result<Id<GuildMarker>> {
        let guild_id = self.access.require_guild()?;
        let (held, outranks) = self.access.bot_role_standing(role_id).await?;
        check_bot_can_grant(held, Permissions::MANAGE_ROLES)?;
        if !outranks {
            return Err(CheckFailure::Hierarchy(HierarchyDenied::BotTooLow).into());
        }
        Ok(guild_id)
    }

    /// メンバーを、別のボイスチャンネルに移動します（`None` で切断）。
    /// Discord が Bot の `MOVE_MEMBERS` と移動先への `CONNECT` を判定します。
    /// メンバーは、ボイスチャンネルにいる必要があります。
    ///
    /// # エラー
    ///
    /// サーバー外では [`CheckFailure::GuildOnly`]、Discord の権限拒否などは HTTP のエラーを返します。
    pub async fn move_member(
        &self,
        user_id: Id<UserMarker>,
        channel_id: Option<Id<ChannelMarker>>,
        reason: &str,
    ) -> Result<()> {
        let guild_id = self.access.require_guild()?;
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
    use crate::Error;
    use crate::{
        Access,
        access::tests::{
            Mock, guild_json_with, member_json, offline_http, role_json_with, slash_interaction,
        },
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
        Mock::start_with_errors(
            vec![
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
                // チャンネルとロール。
                (
                    "/guilds/20/channels".into(),
                    r#"{"id": "77", "type": 0, "guild_id": "20", "name": "new"}"#.into(),
                ),
                (
                    "/channels/30".into(),
                    r#"{"id": "30", "type": 0, "guild_id": "20", "name": "edited"}"#.into(),
                ),
                (
                    "/channels/32".into(),
                    r#"{"id": "32", "type": 2, "guild_id": "20", "name": "edited"}"#.into(),
                ),
                (
                    "/guilds/20/roles".into(),
                    role_json_with(200, 1, Permissions::SEND_MESSAGES),
                ),
                (
                    "/guilds/20/roles/101".into(),
                    role_json_with(101, 3, Permissions::SEND_MESSAGES),
                ),
                (
                    "/guilds/20/roles/102".into(),
                    role_json_with(102, 9, Permissions::empty()),
                ),
            ],
            vec![
                (
                    "/guilds/20/members/99",
                    "404 Not Found",
                    r#"{"code":10007,"message":"Unknown Member"}"#,
                ),
                (
                    "/guilds/20/members/98",
                    "404 Not Found",
                    r#"{"code":10004,"message":"Unknown Guild"}"#,
                ),
            ],
        )
        .await
    }

    /// 変数名 `mock` に隠れないように、別名の入り口を用意する。
    async fn make_mock(bot_role: Permissions) -> Mock {
        mock(bot_role).await
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
        let mock = mock(Permissions::MANAGE_ROLES).await;
        let http = mock.client();
        let resolver = PermissionResolver::default();
        let interaction = slash_interaction(true, 0, 0);
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
        assert_eq!(mock.count("GET /guilds/20/members/4"), 0);
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
        // 呼び出し元には音声の権限がない。Discord は操作対象の VC で判定する。
        let all = Permissions::MANAGE_NICKNAMES;
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

        assert_eq!(mock.count("GET /channels/55"), 0);
    }

    #[tokio::test]
    async fn voice_management_propagates_discords_permission_denial() {
        let mock = Mock::start_with_errors(
            vec![],
            vec![(
                "/guilds/20/members/4",
                "403 Forbidden",
                r#"{"code":50013,"message":"Missing Permissions"}"#,
            )],
        )
        .await;
        let http = mock.client();
        let resolver = PermissionResolver::default();
        let interaction = slash_interaction(true, 0, Permissions::ADMINISTRATOR.bits());
        let manage = manage(&http, &resolver, &interaction);
        let denied = manage.set_voice_mute(user(4), true, "理由").await;
        assert!(matches!(denied, Err(Error::Http(_))), "{denied:?}");
        assert_eq!(mock.count("PATCH /guilds/20/members/4"), 1);
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

    fn denied_validation(result: &Result<impl std::fmt::Debug>) -> bool {
        matches!(result, Err(Error::ChannelValidation(_)))
    }

    #[tokio::test]
    async fn creating_channels_validates_then_checks_the_bots_guild_permissions() {
        use crate::{NewCategory, NewTextChannel, NewVoiceChannel};

        let mock = mock(
            Permissions::MANAGE_CHANNELS | Permissions::MANAGE_ROLES | Permissions::VIEW_CHANNEL,
        )
        .await;
        let http = mock.client();
        let resolver = PermissionResolver::default();
        let interaction = slash_interaction(true, 0, 0);
        let manage = manage(&http, &resolver, &interaction);

        // 範囲外の値は、通信せずに断る。
        for bad in [
            NewTextChannel::new(""),
            NewTextChannel::new("a").topic("あ".repeat(1025)),
            NewTextChannel::new("a").slowmode(Duration::from_secs(21_601)),
        ] {
            assert!(denied_validation(
                &manage.create_text_channel(bad, "理由").await
            ));
        }
        assert!(denied_validation(
            &manage
                .create_voice_channel(NewVoiceChannel::new("v").user_limit(100), "理由")
                .await
        ));
        assert_eq!(mock.count("POST /guilds/20/channels"), 0);

        // 正しい指定は、種類ごとの本文で 1 回だけ送られる。
        let overwrite = Overwrite::new().deny(Permissions::VIEW_CHANNEL);
        let new = NewTextChannel::new("お知らせ")
            .topic("トピック")
            .slowmode(Duration::from_secs(10))
            .category(Id::new(9))
            .overwrite(OverwriteTarget::Role(Id::new(101)), overwrite);
        assert!(manage.create_text_channel(new, "理由").await.is_ok());
        let body = mock
            .body_of("POST /guilds/20/channels")
            .unwrap_or_else(|| panic!("本文がありません"));
        assert_eq!(body["name"], "お知らせ");
        assert_eq!(body["type"], 0);
        assert_eq!(body["topic"], "トピック");
        assert_eq!(body["rate_limit_per_user"], 10);
        assert_eq!(body["parent_id"], "9");
        assert_eq!(body["permission_overwrites"][0]["id"], "101");
        assert_eq!(
            body["permission_overwrites"][0]["deny"],
            Permissions::VIEW_CHANNEL.bits().to_string()
        );

        assert!(
            manage
                .create_voice_channel(
                    NewVoiceChannel::new("雑談").user_limit(10).bitrate(64_000),
                    "理由"
                )
                .await
                .is_ok()
        );
        assert!(
            manage
                .create_category(NewCategory::new("分類"), "理由")
                .await
                .is_ok()
        );
        assert_eq!(mock.count("POST /guilds/20/channels"), 3);
    }

    #[tokio::test]
    async fn creating_channels_needs_manage_channels_and_the_overwritten_permissions() {
        use crate::NewTextChannel;

        // Bot のロールは MANAGE_CHANNELS だけ。
        let mock = mock(Permissions::MANAGE_CHANNELS).await;
        let http = mock.client();
        let resolver = PermissionResolver::default();
        let interaction = slash_interaction(true, 0, 0);
        let manage = manage(&http, &resolver, &interaction);
        assert!(
            manage
                .create_text_channel(NewTextChannel::new("a"), "理由")
                .await
                .is_ok()
        );

        // 上書き対象の権限が必要。MANAGE_ROLES 自体を上書きするなら管理者が必要。
        let with_overwrite = NewTextChannel::new("b").overwrite(
            OverwriteTarget::Role(Id::new(101)),
            Overwrite::new().deny(Permissions::SEND_MESSAGES),
        );
        let denied = manage.create_text_channel(with_overwrite, "理由").await;
        assert!(
            matches!(
                &denied,
                Err(Error::Check(CheckFailure::BotMissingPermissions(missing)))
                    if *missing == Permissions::SEND_MESSAGES
            ),
            "{denied:?}"
        );
        assert_eq!(mock.count("POST /guilds/20/channels"), 1);

        let none = slash_interaction(true, 0, 0);
        let weak_mock = make_mock(Permissions::empty()).await;
        let http = weak_mock.client();
        let weak = Manage::new(Access::for_interaction(&http, &resolver, &none));
        let denied = weak
            .create_text_channel(NewTextChannel::new("c"), "理由")
            .await;
        assert!(matches!(
            denied,
            Err(Error::Check(CheckFailure::BotMissingPermissions(missing)))
                if missing == Permissions::MANAGE_CHANNELS
        ));
        assert_eq!(weak_mock.count("POST /guilds/20/channels"), 0);
    }

    #[tokio::test]
    async fn editing_a_channel_sends_only_the_given_fields() {
        use crate::{TextChannelEdit, VoiceChannelEdit};

        let mock = mock(Permissions::MANAGE_CHANNELS | Permissions::VIEW_CHANNEL).await;
        let http = mock.client();
        let resolver = PermissionResolver::default();
        let interaction = slash_interaction(true, 0, Permissions::MANAGE_CHANNELS.bits());
        let manage = manage(&http, &resolver, &interaction);

        let nothing = manage
            .edit_text_channel(Id::new(30), TextChannelEdit::new(), "理由")
            .await;
        assert!(matches!(
            nothing,
            Err(Error::InvalidArgument(InvalidArgument::NothingToChange))
        ));
        let nothing = manage
            .edit_voice_channel(Id::new(30), VoiceChannelEdit::new(), "理由")
            .await;
        assert!(matches!(
            nothing,
            Err(Error::InvalidArgument(InvalidArgument::NothingToChange))
        ));
        assert!(denied_validation(
            &manage
                .edit_text_channel(Id::new(30), TextChannelEdit::new().name(""), "理由")
                .await
        ));
        assert_eq!(mock.count("PATCH /channels/30"), 0);

        assert!(
            manage
                .edit_text_channel(
                    Id::new(30),
                    TextChannelEdit::new()
                        .name("新しい名前")
                        .slowmode(Duration::ZERO)
                        .category(None),
                    "理由"
                )
                .await
                .is_ok()
        );
        let body = mock
            .body_of("PATCH /channels/30")
            .unwrap_or_else(|| panic!("本文がありません"));
        assert_eq!(body["name"], "新しい名前");
        assert_eq!(body["rate_limit_per_user"], 0);
        assert!(body["parent_id"].is_null(), "{body}");
        assert!(
            body.get("topic").is_none() && body.get("bitrate").is_none(),
            "{body}"
        );

        assert!(
            manage
                .edit_voice_channel(
                    Id::new(32),
                    VoiceChannelEdit::new().bitrate(96_000).user_limit(5),
                    "理由"
                )
                .await
                .is_ok()
        );
        let wrong_kind = manage
            .edit_voice_channel(Id::new(30), VoiceChannelEdit::new().name("VC"), "理由")
            .await;
        assert!(matches!(
            wrong_kind,
            Err(Error::InvalidArgument(InvalidArgument::WrongChannelKind))
        ));
        assert_eq!(mock.count("PATCH /channels/30"), 1);
        assert_eq!(mock.count("PATCH /channels/32"), 1);

        // MANAGE_CHANNELS がなければ、変更も削除も、送らずに断る。
        let weak = slash_interaction(true, 0, Permissions::VIEW_CHANNEL.bits());
        let weak = Manage::new(Access::for_interaction(&http, &resolver, &weak));
        let denied = weak
            .edit_text_channel(Id::new(30), TextChannelEdit::new().nsfw(true), "理由")
            .await;
        assert!(matches!(
            denied,
            Err(Error::Check(CheckFailure::BotMissingPermissions(missing)))
                if missing == Permissions::MANAGE_CHANNELS
        ));
        assert!(weak.delete_channel(Id::new(30), "理由").await.is_err());
        assert_eq!(mock.count("DELETE /channels/30"), 0);
        assert!(manage.delete_channel(Id::new(30), "理由").await.is_ok());
        assert_eq!(mock.count("DELETE /channels/30"), 1);
    }

    #[tokio::test]
    async fn thread_deletion_checks_manage_threads() {
        let mock = Mock::start(vec![(
            "/channels/30".into(),
            r#"{"id":"30","type":11,"guild_id":"20","parent_id":"31","name":"thread"}"#.into(),
        )])
        .await;
        let http = mock.client();
        let resolver = PermissionResolver::default();
        let thread = Id::new(30);
        let wrong = slash_interaction(true, 0, Permissions::MANAGE_CHANNELS.bits());
        assert!(is_bot_missing(
            &manage(&http, &resolver, &wrong)
                .delete_channel(thread, "理由")
                .await,
            Permissions::MANAGE_THREADS
        ));
        assert_eq!(mock.count("DELETE /channels/30"), 0);
        let right = slash_interaction(true, 0, Permissions::MANAGE_THREADS.bits());
        assert!(
            manage(&http, &resolver, &right)
                .delete_channel(thread, "理由")
                .await
                .is_ok()
        );
        assert_eq!(mock.count("DELETE /channels/30"), 1);
    }

    #[tokio::test]
    async fn roles_are_created_edited_and_deleted_within_the_bots_reach() {
        use crate::{NewRole, RoleEdit};

        let bot_role = Permissions::MANAGE_ROLES | Permissions::SEND_MESSAGES;
        let mock = mock(bot_role).await;
        let http = mock.client();
        let resolver = PermissionResolver::default();
        let interaction = slash_interaction(true, 0, 0);
        let manage = manage(&http, &resolver, &interaction);

        assert!(
            manage
                .create_role(
                    NewRole::new("認証済み")
                        .color(0x002e_cc71)
                        .hoist(true)
                        .permissions(Permissions::SEND_MESSAGES),
                    "理由"
                )
                .await
                .is_ok()
        );
        let body = mock
            .body_of("POST /guilds/20/roles")
            .unwrap_or_else(|| panic!("本文がありません"));
        assert_eq!(body["name"], "認証済み");
        assert_eq!(body["color"], 0x002e_cc71);
        assert_eq!(body["hoist"], true);
        // Bot が持たない権限は、付けられない。
        let denied = manage
            .create_role(
                NewRole::new("x").permissions(Permissions::BAN_MEMBERS),
                "理由",
            )
            .await;
        assert!(matches!(
            denied,
            Err(Error::Check(CheckFailure::BotMissingPermissions(missing)))
                if missing == Permissions::BAN_MEMBERS
        ));
        assert_eq!(mock.count("POST /guilds/20/roles"), 1);

        // ロール 101（位置 3）は、Bot（位置 9）より低いので、編集も削除もできる。
        let edited = manage
            .edit_role(
                Id::new(101),
                RoleEdit::new()
                    .name("新名")
                    .color(None)
                    .permissions(Permissions::SEND_MESSAGES),
                "理由",
            )
            .await;
        assert!(edited.is_ok(), "{edited:?}");
        assert_eq!(mock.count("PATCH /guilds/20/roles/101"), 1);
        assert!(manage.delete_role(Id::new(101), "理由").await.is_ok());
        assert_eq!(mock.count("DELETE /guilds/20/roles/101"), 1);

        // Bot と同位のロール、存在しないロールは、送らずに断る。何も指定しない編集も断る。
        for role in [102, 999] {
            let edit = manage
                .edit_role(Id::new(role), RoleEdit::new().hoist(true), "理由")
                .await;
            assert!(
                is_hierarchy(&edit.map(|_| ()), HierarchyDenied::BotTooLow),
                "{role}"
            );
            let delete = manage.delete_role(Id::new(role), "理由").await;
            assert!(is_hierarchy(&delete, HierarchyDenied::BotTooLow), "{role}");
        }
        assert!(matches!(
            manage
                .edit_role(Id::new(101), RoleEdit::new(), "理由")
                .await,
            Err(Error::InvalidArgument(InvalidArgument::NothingToChange))
        ));
        assert_eq!(mock.count("PATCH /guilds/20/roles/102"), 0);
        assert_eq!(mock.count("DELETE /guilds/20/roles/102"), 0);
    }

    #[tokio::test]
    async fn a_missing_bot_member_does_not_skip_the_hierarchy_check_for_a_ban() {
        // 対象の Unknown Member を許しても、Bot 自身の取得エラーは許さない。
        let mock = Mock::start_with_errors(
            vec![
                ("/users/@me".into(), user_json(3)),
                ("/guilds/20".into(), guild_json_with(Permissions::empty())),
                ("/guilds/20/bans/99".into(), "{}".into()),
            ],
            vec![
                (
                    "/guilds/20/members/99",
                    "404 Not Found",
                    r#"{"code":10007,"message":"Unknown Member"}"#,
                ),
                (
                    "/guilds/20/members/3",
                    "404 Not Found",
                    r#"{"code":10007,"message":"Unknown Member"}"#,
                ),
            ],
        )
        .await;
        let http = mock.client();
        let resolver = PermissionResolver::default();
        let interaction = slash_interaction(true, 0, Permissions::BAN_MEMBERS.bits());
        let manage = manage(&http, &resolver, &interaction);
        let result = manage.ban_member(user(99), Duration::ZERO, "理由").await;
        assert!(matches!(result, Err(Error::Http(_))), "{result:?}");
        assert_eq!(mock.count("PUT /guilds/20/bans/99"), 0);
    }

    #[tokio::test]
    async fn an_unrelated_not_found_error_does_not_allow_a_ban() {
        let mock = mock(Permissions::empty()).await;
        let http = mock.client();
        let resolver = PermissionResolver::default();
        let interaction = slash_interaction(true, 0, Permissions::BAN_MEMBERS.bits());
        let manage = manage(&http, &resolver, &interaction);
        let result = manage.ban_member(user(98), Duration::ZERO, "理由").await;
        assert!(matches!(result, Err(Error::Http(_))), "{result:?}");
        assert_eq!(mock.count("PUT /guilds/20/bans/98"), 0);
    }
}
