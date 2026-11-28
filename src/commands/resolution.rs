use std::sync::Arc;

use twilight_model::{
    channel::Channel,
    guild::{Member, Role},
    id::{Id, marker::GuildMarker},
    user::User,
};

use crate::{
    ArgumentResolutionError, MemberCacheHandle, resolve_channel, resolve_member, resolve_role,
    resolve_user,
};

use super::CommandContext;

impl CommandContext<'_> {
    /// ID またはメンションで Discord ユーザーを取得します。DM でも使えます。
    ///
    /// # エラー
    ///
    /// 入力が無効な場合、Discord にユーザーが存在しない場合、通信に失敗した場合に失敗します。
    pub async fn resolve_user(&self, argument: &str) -> Result<User, ArgumentResolutionError> {
        resolve_user(self.http(), argument).await
    }

    /// このサーバーのメンバーを ID、メンション、ユーザー名またはニックネームで取得します。
    ///
    /// # エラー
    ///
    /// DM、無効な引数、候補の不在・重複、取得失敗の場合に失敗します。
    pub async fn resolve_member(
        &self,
        argument: &str,
        cache: Option<&MemberCacheHandle>,
    ) -> Result<Arc<Member>, ArgumentResolutionError> {
        resolve_member(self.http(), self.resolution_guild()?, cache, argument).await
    }

    /// このサーバーのロールを ID、メンションまたは名前で取得します。
    ///
    /// # エラー
    ///
    /// DM、無効な引数、候補の不在・重複、取得失敗の場合に失敗します。
    pub async fn resolve_role(&self, argument: &str) -> Result<Role, ArgumentResolutionError> {
        resolve_role(self.http(), self.resolution_guild()?, argument).await
    }

    /// このサーバーのチャンネルを ID、メンションまたは名前で取得します。
    /// スレッドは ID またはメンションで指定します。
    ///
    /// # エラー
    ///
    /// DM、無効な引数、候補の不在・重複、取得失敗の場合に失敗します。
    pub async fn resolve_channel(
        &self,
        argument: &str,
    ) -> Result<Channel, ArgumentResolutionError> {
        resolve_channel(self.http(), self.resolution_guild()?, argument).await
    }

    fn resolution_guild(&self) -> Result<Id<GuildMarker>, ArgumentResolutionError> {
        self.guild_id().ok_or(ArgumentResolutionError::GuildOnly)
    }
}
