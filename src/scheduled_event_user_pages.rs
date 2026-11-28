use std::num::NonZeroU16;

use serde::{Deserialize, Deserializer};
use twilight_http::{
    Client, Error as HttpError,
    request::TryIntoRequest,
    response::{DeserializeBodyError, marker::ListBody},
};
use twilight_model::{
    guild::{Member, MemberFlags, scheduled_event::GuildScheduledEventUser},
    id::{
        Id,
        marker::{GuildMarker, RoleMarker, ScheduledEventMarker, UserMarker},
    },
    user::{AvatarDecorationData, User},
    util::{ImageHash, Timestamp},
};

use crate::rest_pagination::advance_after;

const MAX_PAGE_SIZE: u16 = 100;

/// Discord REST で予定イベントの参加予定者を必要なページだけ取得します。
///
/// 取得結果はキャッシュに保存しません。取得中にも参加予定者が変動します。
pub struct ScheduledEventUserPages<'http> {
    http: &'http Client,
    guild: Id<GuildMarker>,
    event: Id<ScheduledEventMarker>,
    cursor: Cursor,
    with_member: bool,
    page_size: u16,
    remaining: Option<usize>,
    exhausted: bool,
}

#[derive(Clone, Copy)]
enum Cursor {
    After(Option<Id<UserMarker>>),
    Before(Id<UserMarker>),
}

impl<'http> ScheduledEventUserPages<'http> {
    /// 通信せずに取得を準備します。先頭からユーザー ID の昇順で読み進めます。
    #[must_use]
    pub fn new(
        http: &'http Client,
        guild: Id<GuildMarker>,
        event: Id<ScheduledEventMarker>,
    ) -> Self {
        Self {
            http,
            guild,
            event,
            cursor: Cursor::After(None),
            with_member: false,
            page_size: MAX_PAGE_SIZE,
            remaining: None,
            exhausted: false,
        }
    }

    /// 指定したユーザー ID より大きい ID から取得します。
    /// 以前に指定した `before` または `after` は置き換わります。
    #[must_use]
    pub fn after(mut self, user: Id<UserMarker>) -> Self {
        self.cursor = Cursor::After(Some(user));
        self
    }

    /// 指定したユーザー ID より小さい ID から取得し、より小さい ID の範囲へ進みます。
    /// 以前に指定した `before` または `after` は置き換わります。
    #[must_use]
    pub fn before(mut self, user: Id<UserMarker>) -> Self {
        self.cursor = Cursor::Before(user);
        self
    }

    /// Discord にサーバーメンバー情報も要求するかを指定します。既定は `false` です。
    /// `true` でも、情報が存在するユーザーにのみメンバー情報が付きます。
    #[must_use]
    pub fn with_member(mut self, enabled: bool) -> Self {
        self.with_member = enabled;
        self
    }

    /// 返す参加予定者の総件数の上限を設定します。0 件の場合はリクエストを送りません。
    #[must_use]
    pub fn limit(mut self, limit: usize) -> Self {
        self.remaining = Some(limit);
        self
    }

    /// リクエスト 1 回あたりの上限を設定します。既定は 100 件で、100 件が上限です。
    #[must_use]
    pub fn page_size(mut self, size: NonZeroU16) -> Self {
        self.page_size = size.get().min(MAX_PAGE_SIZE);
        self
    }

    /// 次のページを返します。取得が終わった場合や総件数の上限に達した場合は `None` です。
    ///
    /// 各ページ内はユーザー ID の昇順です。`before` を指定すると、次のページに
    /// 前のページより小さい ID が入ります。ユーザー ID は参加予定を登録した順を表しません。
    ///
    /// # エラー
    ///
    /// 通信、応答の読み取り、ページの検証に失敗するとエラーを返します。
    /// エラー時はカーソルと残り件数を保持します。
    pub async fn next_page(
        &mut self,
    ) -> Result<Option<Vec<GuildScheduledEventUser>>, ScheduledEventUserPagesError> {
        let limit = self
            .remaining
            .and_then(|remaining| u16::try_from(remaining).ok())
            .map_or(self.page_size, |remaining| self.page_size.min(remaining));
        if self.exhausted || limit == 0 {
            return Ok(None);
        }

        let request = self
            .http
            .guild_scheduled_event_users(self.guild, self.event)
            .limit(limit)
            .with_member(self.with_member);
        let request = match self.cursor {
            Cursor::After(None) => request,
            Cursor::After(Some(id)) => request.after(id),
            Cursor::Before(id) => request.before(id),
        };
        let mut users: Vec<_> = self
            .http
            .request::<ListBody<Subscriber>>(request.try_into_request()?)
            .await?
            .models()
            .await?
            .into_iter()
            .map(|subscriber| subscriber.0)
            .collect();
        let limit = usize::from(limit);
        if users.len() > limit
            || users.iter().any(|entry| {
                entry.guild_scheduled_event_id != self.event
                    || entry
                        .member
                        .as_ref()
                        .is_some_and(|member| member.user.id != entry.user.id)
            })
        {
            return Err(ScheduledEventUserPagesError::InvalidPage);
        }
        let cursor = self.cursor.advance(&mut users, limit)?;
        self.exhausted = users.len() < limit;
        if let Some(remaining) = &mut self.remaining {
            *remaining -= users.len();
        }
        self.cursor = cursor;
        Ok((!users.is_empty()).then_some(users))
    }
}

struct Subscriber(GuildScheduledEventUser);

impl<'de> Deserialize<'de> for Subscriber {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            guild_scheduled_event_id: Id<ScheduledEventMarker>,
            user: User,
            member: Option<MemberWire>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Ok(Self(GuildScheduledEventUser {
            guild_scheduled_event_id: wire.guild_scheduled_event_id,
            member: wire.member.map(|member| member.into_member(&wire.user)),
            user: wire.user,
        }))
    }
}

// A scheduled-event member can omit its user. Read fields directly to preserve
// pending (which PartialMember omits) without buffering flattened fields.
#[derive(Deserialize)]
struct MemberWire {
    avatar: Option<ImageHash>,
    avatar_decoration_data: Option<AvatarDecorationData>,
    banner: Option<ImageHash>,
    communication_disabled_until: Option<Timestamp>,
    deaf: bool,
    flags: MemberFlags,
    joined_at: Option<Timestamp>,
    mute: bool,
    nick: Option<String>,
    #[serde(default)]
    pending: bool,
    premium_since: Option<Timestamp>,
    roles: Vec<Id<RoleMarker>>,
    user: Option<User>,
}

impl MemberWire {
    fn into_member(self, subscriber: &User) -> Member {
        Member {
            avatar: self.avatar,
            avatar_decoration_data: self.avatar_decoration_data,
            banner: self.banner,
            communication_disabled_until: self.communication_disabled_until,
            deaf: self.deaf,
            flags: self.flags,
            joined_at: self.joined_at,
            mute: self.mute,
            nick: self.nick,
            pending: self.pending,
            premium_since: self.premium_since,
            roles: self.roles,
            user: self.user.unwrap_or_else(|| subscriber.clone()),
        }
    }
}

impl Cursor {
    fn advance(
        self,
        users: &mut [GuildScheduledEventUser],
        limit: usize,
    ) -> Result<Self, ScheduledEventUserPagesError> {
        let after = match self {
            Self::After(after) => after,
            Self::Before(_) => None,
        };
        let newest = advance_after(users, limit, after, |entry| entry.user.id)
            .ok_or(ScheduledEventUserPagesError::InvalidPage)?;
        match self {
            Self::After(_) => Ok(Self::After(newest)),
            Self::Before(before) => {
                if newest.is_some_and(|id| id >= before) {
                    return Err(ScheduledEventUserPagesError::InvalidPage);
                }
                Ok(Self::Before(
                    users.first().map_or(before, |entry| entry.user.id),
                ))
            }
        }
    }
}

/// REST で予定イベントの参加予定者をページ取得できなかった理由です。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ScheduledEventUserPagesError {
    /// Discord へのリクエストが失敗しました。
    #[error("予定イベントの参加予定者を取得できませんでした")]
    Http(#[from] HttpError),
    /// Discord の応答を参加予定者の一覧として読み取れませんでした。
    #[error("予定イベントの参加予定者の応答を読み取れませんでした")]
    HttpBody(#[from] DeserializeBodyError),
    /// 応答に過剰な件数、不一致のイベント ID やメンバー情報、重複または範囲外の ID が含まれます。
    #[error("予定イベントの参加予定者のページが無効です")]
    InvalidPage,
}

#[cfg(test)]
mod tests;
