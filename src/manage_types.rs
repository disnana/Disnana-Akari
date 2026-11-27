//! チャンネルとロールの作成・編集に渡す、種類ごとの型。
//!
//! 種類ごとに型を分けて、成り立たない組み合わせ（テキストチャンネルのビットレートなど）を書けなくする。
//! 値の範囲は、Discord に送る前に検証する。

use std::time::Duration;

use twilight_model::{
    guild::Permissions,
    id::{Id, marker::ChannelMarker},
};
use twilight_validate::channel as validate;

use crate::{Overwrite, OverwriteTarget, Result};

/// 作成するチャンネルの、共通の設定。
#[derive(Clone, Debug, Default)]
struct Common {
    category: Option<Id<ChannelMarker>>,
    overwrites: Vec<(OverwriteTarget, Overwrite)>,
}

fn slowmode_seconds(slowmode: Duration) -> u16 {
    u16::try_from(slowmode.as_secs()).unwrap_or(u16::MAX)
}

/// 作成するテキストチャンネルです。
///
/// ```
/// use std::time::Duration;
/// use disnana_akari::NewTextChannel;
///
/// let channel = NewTextChannel::new("お知らせ")
///     .topic("サーバーからのお知らせ")
///     .slowmode(Duration::from_secs(10));
/// # let _ = channel;
/// ```
#[derive(Clone, Debug)]
pub struct NewTextChannel {
    name: String,
    topic: Option<String>,
    nsfw: bool,
    slowmode: Option<Duration>,
    common: Common,
}

impl NewTextChannel {
    /// 名前を指定して作ります。名前は 1〜100 文字です。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            topic: None,
            nsfw: false,
            slowmode: None,
            common: Common::default(),
        }
    }

    /// トピックを指定します。1024 文字以内です。
    #[must_use]
    pub fn topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    /// 年齢制限のあるチャンネルにします。
    #[must_use]
    pub fn nsfw(mut self, nsfw: bool) -> Self {
        self.nsfw = nsfw;
        self
    }

    /// スローモードの間隔を指定します。6 時間以内です。
    #[must_use]
    pub fn slowmode(mut self, interval: Duration) -> Self {
        self.slowmode = Some(interval);
        self
    }

    /// 入れるカテゴリーを指定します。
    #[must_use]
    pub fn category(mut self, category: Id<ChannelMarker>) -> Self {
        self.common.category = Some(category);
        self
    }

    /// 対象の権限の上書きを追加します。
    #[must_use]
    pub fn overwrite(mut self, target: OverwriteTarget, overwrite: Overwrite) -> Self {
        self.common.overwrites.push((target, overwrite));
        self
    }

    pub(crate) fn validate(&self) -> Result<()> {
        validate::name(&self.name)?;
        if let Some(topic) = &self.topic {
            validate::topic(topic)?;
        }
        if let Some(slowmode) = self.slowmode {
            validate::rate_limit_per_user(slowmode_seconds(slowmode))?;
        }
        Ok(())
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn topic_value(&self) -> Option<&str> {
        self.topic.as_deref()
    }

    pub(crate) const fn nsfw_value(&self) -> bool {
        self.nsfw
    }

    pub(crate) fn slowmode_value(&self) -> Option<u16> {
        self.slowmode.map(slowmode_seconds)
    }

    pub(crate) const fn category_value(&self) -> Option<Id<ChannelMarker>> {
        self.common.category
    }

    pub(crate) fn overwrites(&self) -> &[(OverwriteTarget, Overwrite)] {
        &self.common.overwrites
    }
}

/// 作成するボイスチャンネルです。
///
/// ```
/// use disnana_akari::NewVoiceChannel;
///
/// let channel = NewVoiceChannel::new("雑談").user_limit(10).bitrate(64_000);
/// # let _ = channel;
/// ```
#[derive(Clone, Debug)]
pub struct NewVoiceChannel {
    name: String,
    bitrate: Option<u32>,
    user_limit: Option<u16>,
    common: Common,
}

impl NewVoiceChannel {
    /// 名前を指定して作ります。名前は 1〜100 文字です。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            bitrate: None,
            user_limit: None,
            common: Common::default(),
        }
    }

    /// ビットレート（bps）を指定します。8000 以上です。上限はサーバーのブーストの段階で決まるので、
    /// Discord の応答に従います。
    #[must_use]
    pub fn bitrate(mut self, bitrate: u32) -> Self {
        self.bitrate = Some(bitrate);
        self
    }

    /// 同時に参加できる人数を指定します。0 は無制限、上限は 99 です。
    #[must_use]
    pub fn user_limit(mut self, limit: u16) -> Self {
        self.user_limit = Some(limit);
        self
    }

    /// 入れるカテゴリーを指定します。
    #[must_use]
    pub fn category(mut self, category: Id<ChannelMarker>) -> Self {
        self.common.category = Some(category);
        self
    }

    /// 対象の権限の上書きを追加します。
    #[must_use]
    pub fn overwrite(mut self, target: OverwriteTarget, overwrite: Overwrite) -> Self {
        self.common.overwrites.push((target, overwrite));
        self
    }

    pub(crate) fn validate(&self) -> Result<()> {
        validate::name(&self.name)?;
        if let Some(bitrate) = self.bitrate {
            validate::bitrate(bitrate)?;
        }
        if let Some(limit) = self.user_limit {
            validate::user_limit(limit)?;
        }
        Ok(())
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) const fn bitrate_value(&self) -> Option<u32> {
        self.bitrate
    }

    pub(crate) const fn user_limit_value(&self) -> Option<u16> {
        self.user_limit
    }

    pub(crate) const fn category_value(&self) -> Option<Id<ChannelMarker>> {
        self.common.category
    }

    pub(crate) fn overwrites(&self) -> &[(OverwriteTarget, Overwrite)] {
        &self.common.overwrites
    }
}

/// 作成するカテゴリーです。
#[derive(Clone, Debug)]
pub struct NewCategory {
    name: String,
    common: Common,
}

impl NewCategory {
    /// 名前を指定して作ります。名前は 1〜100 文字です。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            common: Common::default(),
        }
    }

    /// 対象の権限の上書きを追加します。
    #[must_use]
    pub fn overwrite(mut self, target: OverwriteTarget, overwrite: Overwrite) -> Self {
        self.common.overwrites.push((target, overwrite));
        self
    }

    pub(crate) fn validate(&self) -> Result<()> {
        validate::name(&self.name)?;
        Ok(())
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn overwrites(&self) -> &[(OverwriteTarget, Overwrite)] {
        &self.common.overwrites
    }
}

/// テキストチャンネルの編集です。指定した項目だけを変更します。
#[derive(Clone, Debug, Default)]
pub struct TextChannelEdit {
    name: Option<String>,
    topic: Option<String>,
    nsfw: Option<bool>,
    slowmode: Option<Duration>,
    category: Option<Option<Id<ChannelMarker>>>,
}

impl TextChannelEdit {
    /// 何も変更しない編集を作ります。
    pub fn new() -> Self {
        Self::default()
    }

    /// 名前を変更します。1〜100 文字です。
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// トピックを変更します。1024 文字以内です。
    #[must_use]
    pub fn topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    /// 年齢制限の有無を変更します。
    #[must_use]
    pub fn nsfw(mut self, nsfw: bool) -> Self {
        self.nsfw = Some(nsfw);
        self
    }

    /// スローモードの間隔を変更します（`Duration::ZERO` で解除）。6 時間以内です。
    #[must_use]
    pub fn slowmode(mut self, interval: Duration) -> Self {
        self.slowmode = Some(interval);
        self
    }

    /// 入れるカテゴリーを変更します（`None` でカテゴリーから出します）。
    #[must_use]
    pub fn category(mut self, category: Option<Id<ChannelMarker>>) -> Self {
        self.category = Some(category);
        self
    }

    /// 何も変更しないかを返します。
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.topic.is_none()
            && self.nsfw.is_none()
            && self.slowmode.is_none()
            && self.category.is_none()
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if let Some(name) = &self.name {
            validate::name(name)?;
        }
        if let Some(topic) = &self.topic {
            validate::topic(topic)?;
        }
        if let Some(slowmode) = self.slowmode {
            validate::rate_limit_per_user(slowmode_seconds(slowmode))?;
        }
        Ok(())
    }

    pub(crate) fn name_value(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub(crate) fn topic_value(&self) -> Option<&str> {
        self.topic.as_deref()
    }

    pub(crate) const fn nsfw_value(&self) -> Option<bool> {
        self.nsfw
    }

    pub(crate) fn slowmode_value(&self) -> Option<u16> {
        self.slowmode.map(slowmode_seconds)
    }

    pub(crate) const fn category_value(&self) -> Option<Option<Id<ChannelMarker>>> {
        self.category
    }
}

/// ボイスチャンネルの編集です。指定した項目だけを変更します。
#[derive(Clone, Debug, Default)]
pub struct VoiceChannelEdit {
    name: Option<String>,
    bitrate: Option<u32>,
    user_limit: Option<u16>,
    category: Option<Option<Id<ChannelMarker>>>,
}

impl VoiceChannelEdit {
    /// 何も変更しない編集を作ります。
    pub fn new() -> Self {
        Self::default()
    }

    /// 名前を変更します。1〜100 文字です。
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// ビットレート（bps）を変更します。8000 以上です。
    #[must_use]
    pub fn bitrate(mut self, bitrate: u32) -> Self {
        self.bitrate = Some(bitrate);
        self
    }

    /// 同時に参加できる人数を変更します。0 は無制限、上限は 99 です。
    #[must_use]
    pub fn user_limit(mut self, limit: u16) -> Self {
        self.user_limit = Some(limit);
        self
    }

    /// 入れるカテゴリーを変更します（`None` でカテゴリーから出します）。
    #[must_use]
    pub fn category(mut self, category: Option<Id<ChannelMarker>>) -> Self {
        self.category = Some(category);
        self
    }

    /// 何も変更しないかを返します。
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.bitrate.is_none()
            && self.user_limit.is_none()
            && self.category.is_none()
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if let Some(name) = &self.name {
            validate::name(name)?;
        }
        if let Some(bitrate) = self.bitrate {
            validate::bitrate(bitrate)?;
        }
        if let Some(limit) = self.user_limit {
            validate::user_limit(limit)?;
        }
        Ok(())
    }

    pub(crate) fn name_value(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub(crate) const fn bitrate_value(&self) -> Option<u32> {
        self.bitrate
    }

    pub(crate) const fn user_limit_value(&self) -> Option<u16> {
        self.user_limit
    }

    pub(crate) const fn category_value(&self) -> Option<Option<Id<ChannelMarker>>> {
        self.category
    }
}

/// 作成するロールです。
///
/// ```
/// use disnana_akari::{NewRole, model::guild::Permissions};
///
/// let role = NewRole::new("認証済み").color(0x2ecc71).hoist(true).permissions(Permissions::SEND_MESSAGES);
/// # let _ = role;
/// ```
#[derive(Clone, Debug)]
pub struct NewRole {
    name: String,
    color: Option<u32>,
    hoist: bool,
    mentionable: bool,
    permissions: Permissions,
}

impl NewRole {
    /// 名前を指定して作ります。権限は、何も付けません。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            color: None,
            hoist: false,
            mentionable: false,
            permissions: Permissions::empty(),
        }
    }

    /// 色を `0xRRGGBB` で指定します。
    #[must_use]
    pub fn color(mut self, color: u32) -> Self {
        self.color = Some(color);
        self
    }

    /// メンバー一覧で、別に表示するかを指定します。
    #[must_use]
    pub fn hoist(mut self, hoist: bool) -> Self {
        self.hoist = hoist;
        self
    }

    /// 誰でもメンションできるかを指定します。
    #[must_use]
    pub fn mentionable(mut self, mentionable: bool) -> Self {
        self.mentionable = mentionable;
        self
    }

    /// 付ける権限を指定します。Bot が持つ権限だけを付けられます。
    #[must_use]
    pub fn permissions(mut self, permissions: Permissions) -> Self {
        self.permissions = permissions;
        self
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) const fn color_value(&self) -> Option<u32> {
        self.color
    }

    pub(crate) const fn hoist_value(&self) -> bool {
        self.hoist
    }

    pub(crate) const fn mentionable_value(&self) -> bool {
        self.mentionable
    }

    pub(crate) const fn permissions_value(&self) -> Permissions {
        self.permissions
    }
}

/// ロールの編集です。指定した項目だけを変更します。
#[derive(Clone, Debug, Default)]
pub struct RoleEdit {
    name: Option<String>,
    color: Option<Option<u32>>,
    hoist: Option<bool>,
    mentionable: Option<bool>,
    permissions: Option<Permissions>,
}

impl RoleEdit {
    /// 何も変更しない編集を作ります。
    pub fn new() -> Self {
        Self::default()
    }

    /// 名前を変更します。
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// 色を `0xRRGGBB` で変更します（`None` で色を外します）。
    #[must_use]
    pub fn color(mut self, color: Option<u32>) -> Self {
        self.color = Some(color);
        self
    }

    /// メンバー一覧で別に表示するかを変更します。
    #[must_use]
    pub fn hoist(mut self, hoist: bool) -> Self {
        self.hoist = Some(hoist);
        self
    }

    /// 誰でもメンションできるかを変更します。
    #[must_use]
    pub fn mentionable(mut self, mentionable: bool) -> Self {
        self.mentionable = Some(mentionable);
        self
    }

    /// 権限を、指定した値に置き換えます。現在の値を読み、足したい権限だけを足して渡してください。
    #[must_use]
    pub fn permissions(mut self, permissions: Permissions) -> Self {
        self.permissions = Some(permissions);
        self
    }

    /// 何も変更しないかを返します。
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.color.is_none()
            && self.hoist.is_none()
            && self.mentionable.is_none()
            && self.permissions.is_none()
    }

    pub(crate) fn name_value(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub(crate) const fn color_value(&self) -> Option<Option<u32>> {
        self.color
    }

    pub(crate) const fn hoist_value(&self) -> Option<bool> {
        self.hoist
    }

    pub(crate) const fn mentionable_value(&self) -> Option<bool> {
        self.mentionable
    }

    pub(crate) const fn permissions_value(&self) -> Option<Permissions> {
        self.permissions
    }
}
