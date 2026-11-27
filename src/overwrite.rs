//! チャンネルの権限の上書き設定を、許可・拒否・継承の 3 値で扱う型。

use twilight_model::{
    channel::permission_overwrite::{PermissionOverwrite, PermissionOverwriteType},
    guild::Permissions,
    http::permission_overwrite::{
        PermissionOverwrite as RequestOverwrite, PermissionOverwriteType as RequestKind,
    },
    id::{
        Id,
        marker::{RoleMarker, UserMarker},
    },
};

/// 権限 1 つの上書きの状態です。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Tri {
    /// 許可します。
    Allow,
    /// 拒否します。
    Deny,
    /// 上書きせず、ロールなどの設定を引き継ぎます。
    Inherit,
}

/// チャンネルの権限を上書きする対象です。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum OverwriteTarget {
    /// ロールです。`@everyone` は、サーバーの ID と同じ値のロールです。
    Role(Id<RoleMarker>),
    /// メンバーです。
    Member(Id<UserMarker>),
}

impl OverwriteTarget {
    pub(crate) fn kind(self) -> PermissionOverwriteType {
        match self {
            Self::Role(_) => PermissionOverwriteType::Role,
            Self::Member(_) => PermissionOverwriteType::Member,
        }
    }

    fn request_kind(self) -> RequestKind {
        match self {
            Self::Role(_) => RequestKind::Role,
            Self::Member(_) => RequestKind::Member,
        }
    }

    pub(crate) fn raw_id(self) -> u64 {
        match self {
            Self::Role(id) => id.get(),
            Self::Member(id) => id.get(),
        }
    }
}

/// 1 つの対象に対する、チャンネルの権限の上書きです。
///
/// 権限ごとに、許可、拒否、継承のどれかを指定します。同じ権限を後から指定し直すと、前の指定を置き換えます。
///
/// ```
/// use disnana_akari::{Overwrite, Tri, model::guild::Permissions};
///
/// let overwrite = Overwrite::new()
///     .allow(Permissions::VIEW_CHANNEL)
///     .deny(Permissions::SEND_MESSAGES)
///     .inherit(Permissions::VIEW_CHANNEL);
/// assert_eq!(overwrite.state(Permissions::VIEW_CHANNEL), Some(Tri::Inherit));
/// assert_eq!(overwrite.state(Permissions::SEND_MESSAGES), Some(Tri::Deny));
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Overwrite {
    allow: Permissions,
    deny: Permissions,
}

impl Default for Overwrite {
    fn default() -> Self {
        Self::new()
    }
}

impl Overwrite {
    /// 何も上書きしない設定を作ります。
    pub const fn new() -> Self {
        Self {
            allow: Permissions::empty(),
            deny: Permissions::empty(),
        }
    }

    pub(crate) fn from_twilight(overwrite: &PermissionOverwrite) -> Self {
        // 許可と拒否の両方に入っている権限は、Discord では拒否が優先される。
        Self {
            allow: overwrite.allow - overwrite.deny,
            deny: overwrite.deny,
        }
    }

    /// 指定した権限を許可します。
    #[must_use]
    pub const fn allow(mut self, permissions: Permissions) -> Self {
        self.allow = self.allow.union(permissions);
        self.deny = self.deny.difference(permissions);
        self
    }

    /// 指定した権限を拒否します。
    #[must_use]
    pub const fn deny(mut self, permissions: Permissions) -> Self {
        self.deny = self.deny.union(permissions);
        self.allow = self.allow.difference(permissions);
        self
    }

    /// 指定した権限の上書きを外し、継承に戻します。
    #[must_use]
    pub const fn inherit(mut self, permissions: Permissions) -> Self {
        self.allow = self.allow.difference(permissions);
        self.deny = self.deny.difference(permissions);
        self
    }

    /// 許可している権限を返します。
    pub const fn allowed(&self) -> Permissions {
        self.allow
    }

    /// 拒否している権限を返します。
    pub const fn denied(&self) -> Permissions {
        self.deny
    }

    /// 許可も拒否もしていないかを返します。
    pub const fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }

    /// 指定した権限の状態を返します。複数の権限を渡したとき、状態が揃っていなければ `None` です。
    /// 何も渡さなければ `None` です。
    pub fn state(&self, permissions: Permissions) -> Option<Tri> {
        if permissions.is_empty() {
            return None;
        }
        if self.allow.contains(permissions) {
            Some(Tri::Allow)
        } else if self.deny.contains(permissions) {
            Some(Tri::Deny)
        } else if (self.allow | self.deny).intersects(permissions) {
            None
        } else {
            Some(Tri::Inherit)
        }
    }

    pub(crate) fn touched(&self) -> Permissions {
        self.allow | self.deny
    }

    /// Discord に送る形にする。許可も拒否も空のときは、空の権限として送る。
    pub(crate) fn into_request(self, target: OverwriteTarget) -> RequestOverwrite {
        RequestOverwrite {
            allow: Some(self.allow),
            deny: Some(self.deny),
            id: Id::new(target.raw_id()),
            kind: target.request_kind(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_calls_replace_earlier_ones_for_the_same_permission() {
        let overwrite = Overwrite::new()
            .allow(Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES)
            .deny(Permissions::SEND_MESSAGES)
            .inherit(Permissions::ADD_REACTIONS);
        assert_eq!(overwrite.allowed(), Permissions::VIEW_CHANNEL);
        assert_eq!(overwrite.denied(), Permissions::SEND_MESSAGES);
        assert_eq!(overwrite.state(Permissions::VIEW_CHANNEL), Some(Tri::Allow));
        assert_eq!(overwrite.state(Permissions::SEND_MESSAGES), Some(Tri::Deny));
        assert_eq!(
            overwrite.state(Permissions::ADD_REACTIONS),
            Some(Tri::Inherit)
        );
        let reallowed = overwrite.allow(Permissions::SEND_MESSAGES);
        assert_eq!(reallowed.denied(), Permissions::empty());
    }

    #[test]
    fn mixed_or_empty_queries_have_no_single_state() {
        let overwrite = Overwrite::new()
            .allow(Permissions::VIEW_CHANNEL)
            .deny(Permissions::SEND_MESSAGES);
        assert_eq!(
            overwrite.state(Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES),
            None
        );
        assert_eq!(overwrite.state(Permissions::empty()), None);
        assert_eq!(
            overwrite.state(Permissions::VIEW_CHANNEL | Permissions::ADD_REACTIONS),
            None
        );
        assert!(Overwrite::new().is_empty());
    }

    #[test]
    fn deny_wins_when_discord_reports_a_permission_in_both() {
        let both = PermissionOverwrite {
            allow: Permissions::SEND_MESSAGES | Permissions::VIEW_CHANNEL,
            deny: Permissions::SEND_MESSAGES,
            id: Id::new(1),
            kind: PermissionOverwriteType::Role,
        };
        let overwrite = Overwrite::from_twilight(&both);
        assert_eq!(overwrite.state(Permissions::SEND_MESSAGES), Some(Tri::Deny));
        assert_eq!(overwrite.state(Permissions::VIEW_CHANNEL), Some(Tri::Allow));
    }

    #[test]
    fn targets_convert_to_requests() {
        let overwrite = Overwrite::new().deny(Permissions::CONNECT);
        let role = overwrite.into_request(OverwriteTarget::Role(Id::new(7)));
        assert_eq!(role.kind, RequestKind::Role);
        assert_eq!(role.id.get(), 7);
        assert_eq!(role.deny, Some(Permissions::CONNECT));
        assert_eq!(role.allow, Some(Permissions::empty()));
        let member = overwrite.into_request(OverwriteTarget::Member(Id::new(9)));
        assert_eq!(member.kind, RequestKind::Member);
        assert_eq!(member.id.get(), 9);
    }
}
