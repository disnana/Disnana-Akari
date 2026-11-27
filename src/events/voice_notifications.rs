use serde::Deserialize;
use twilight_gateway::Intents;
use twilight_model::{
    channel::message::EmojiReactionType,
    id::{
        Id,
        marker::{ChannelMarker, EmojiMarker, GuildMarker, UserMarker},
    },
    user::User,
};

use super::{Events, Handler, Reporter, Result, adapt_handler, dispatch};
use crate::{
    Error,
    voice_channel::{ChannelInfo, VoiceChannelStartTimeUpdate, VoiceChannelStatusUpdate},
};

/// サウンドボード音の [`Id`] に使うマーカーです。
pub enum SoundboardSoundMarker {}

/// サウンドボード音の ID です。Discord の数値と文字列の両方の形式を読み取れます。
pub type SoundboardSoundId = Id<SoundboardSoundMarker>;

/// ボイスチャンネルで再生された絵文字アニメーションの種類です。
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(from = "u64")]
#[non_exhaustive]
pub enum VoiceEffectAnimation {
    /// Nitro のアニメーション。
    Premium,
    /// 標準のアニメーション。
    Basic,
    /// 未知の種類を表す、Discord が送った数値。
    Unknown(u64),
}

impl From<u64> for VoiceEffectAnimation {
    fn from(value: u64) -> Self {
        match value {
            0 => Self::Premium,
            1 => Self::Basic,
            value => Self::Unknown(value),
        }
    }
}

/// Bot が接続中のボイスチャンネルで、絵文字やサウンドボード音が再生された通知です。
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct VoiceChannelEffectSend {
    /// エフェクトが送信されたサーバーの ID。
    pub guild_id: Id<GuildMarker>,
    /// エフェクトが送信されたボイスチャンネルの ID。
    pub channel_id: Id<ChannelMarker>,
    /// エフェクトを送信したユーザーの ID。
    pub user_id: Id<UserMarker>,
    /// Discord が送った絵文字。
    pub emoji: Option<EmojiReactionType>,
    /// 絵文字アニメーションの種類。
    pub animation_type: Option<VoiceEffectAnimation>,
    /// 絵文字アニメーションの ID。
    pub animation_id: Option<u64>,
    /// サウンドボード音を使った場合の音の ID。
    pub sound_id: Option<SoundboardSoundId>,
    /// Discord が送った音量（0 から 1）。
    pub sound_volume: Option<f64>,
}

/// Discord のサウンドボード音です。
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct SoundboardSound {
    /// サウンドボード音の ID。
    pub sound_id: SoundboardSoundId,
    /// ユーザーに表示される名前。
    pub name: String,
    /// Discord が送った音量（0 から 1）。
    pub volume: f64,
    /// カスタム絵文字の ID。
    pub emoji_id: Option<Id<EmojiMarker>>,
    /// Unicode 絵文字の名前。
    pub emoji_name: Option<String>,
    /// サーバー固有の音に Discord が付けるサーバー ID。
    pub guild_id: Option<Id<GuildMarker>>,
    /// 現在使用できる音かどうか。
    pub available: bool,
    /// Discord が送った作成者。
    pub user: Option<User>,
}

/// サーバーに追加されたサウンドボード音の通知です。
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[non_exhaustive]
#[serde(transparent)]
pub struct GuildSoundboardSoundCreate {
    /// 追加された音。
    pub sound: SoundboardSound,
}

/// サーバーのサウンドボード音が更新された通知です。
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[non_exhaustive]
#[serde(transparent)]
pub struct GuildSoundboardSoundUpdate {
    /// 更新後の音。
    pub sound: SoundboardSound,
}

/// サーバーのサウンドボード音が削除された通知です。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
pub struct GuildSoundboardSoundDelete {
    /// 音があったサーバーの ID。
    pub guild_id: Id<GuildMarker>,
    /// 削除された音の ID。
    pub sound_id: SoundboardSoundId,
}

/// サーバーの複数のサウンドボード音が更新された通知です。
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct GuildSoundboardSoundsUpdate {
    /// 更新があったサーバーの ID。
    pub guild_id: Id<GuildMarker>,
    /// Discord が通知した音の一覧。
    pub soundboard_sounds: Vec<SoundboardSound>,
}

/// Gateway のサウンドボード一覧要求に対する、サーバーごとの応答です。
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct SoundboardSounds {
    /// 一覧の対象サーバーの ID。
    pub guild_id: Id<GuildMarker>,
    /// サーバーの音の一覧。
    pub soundboard_sounds: Vec<SoundboardSound>,
}

// Twilight 0.18 がまだ扱わない Dispatch を、登録された種類だけ読み取ります。
macro_rules! notifications {
    ($($(#[$doc:meta])* $method:ident, $field:ident, $payload:ident, $name:literal, $intent:expr;)+) => {
        #[derive(Clone, Copy, Default)]
        pub(crate) struct VoiceNotificationTypes(u16);

        #[repr(u8)]
        enum Kind {
            $($payload,)+
        }

        pub(crate) enum VoiceNotification {
            $($payload(Box<$payload>),)+
        }

        #[derive(Default)]
        pub(super) struct VoiceNotificationHandlers {
            $($field: Vec<Box<dyn Handler<$payload>>>,)+
        }

        impl std::fmt::Debug for VoiceNotificationHandlers {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct("VoiceNotificationHandlers")
                    $(.field($name, &self.$field.len()))+
                    .finish()
            }
        }

        impl Events {
            $(
                $(#[$doc])*
                pub fn $method<E>(
                    &mut self,
                    handler: impl AsyncFn(&$payload) -> std::result::Result<(), E> + Send + Sync + 'static,
                ) where
                    E: Into<Box<dyn std::error::Error + Send + Sync>>,
                {
                    self.voice_notifications.$field.push(Box::new(adapt_handler!(handler, event: &$payload)));
                }
            )+
        }

        impl VoiceNotificationHandlers {
            pub(super) fn types(&self, intents: Intents) -> Result<VoiceNotificationTypes> {
                let mut types = 0;
                $(
                    if !self.$field.is_empty() {
                        let required = $intent;
                        if !intents.contains(required) {
                            return Err(Error::MissingEventIntents { any_of: required });
                        }
                        types |= 1 << Kind::$payload as u8;
                    }
                )+
                Ok(VoiceNotificationTypes(types))
            }

            pub(super) async fn dispatch(&self, event: &VoiceNotification, reporter: &Reporter) {
                match event {
                    $(VoiceNotification::$payload(payload) => dispatch(&self.$field, payload.as_ref(), $name, reporter).await,)+
                }
            }
        }

        impl VoiceNotificationTypes {
            pub(crate) const fn is_empty(self) -> bool {
                self.0 == 0
            }

            pub(crate) fn decode(self, name: &str, json: &str) -> Option<Result<VoiceNotification>> {
                #[derive(Deserialize)]
                struct Dispatch<T> {
                    d: Box<T>,
                }

                match name {
                    $($name if self.0 & (1 << Kind::$payload as u8) != 0 => {
                        let decoded = serde_json::from_str::<Dispatch<$payload>>(json)
                            .map(|payload| VoiceNotification::$payload(payload.d))
                            .map_err(|source| Error::GatewayNotificationDecode { event: $name, source });
                        Some(decoded)
                    })+
                    _ => None,
                }
            }
        }

        impl VoiceNotification {
            pub(crate) const fn name(&self) -> &'static str {
                match self {
                    $(Self::$payload(_) => $name,)+
                }
            }
        }
    };
}

notifications! {
    /// ボイスチャンネルのエフェクト送信の通知を受け取ります。
    ///
    /// `GUILD_VOICE_STATES` Intent と、そのチャンネルへの Bot の接続が必要です。
    on_voice_channel_effect_send, effect, VoiceChannelEffectSend, "VOICE_CHANNEL_EFFECT_SEND", Intents::GUILD_VOICE_STATES;
    /// サーバーのサウンドボード音の追加通知を受け取ります。
    ///
    /// Twilight の `GUILD_EMOJIS_AND_STICKERS`（Discord の `GUILD_EXPRESSIONS`）Intent が必要です。
    on_guild_soundboard_sound_create, create, GuildSoundboardSoundCreate, "GUILD_SOUNDBOARD_SOUND_CREATE", Intents::GUILD_EMOJIS_AND_STICKERS;
    /// サーバーのサウンドボード音の更新通知を受け取ります。
    ///
    /// 必要な Intent は [`Self::on_guild_soundboard_sound_create`] と同じです。
    on_guild_soundboard_sound_update, update, GuildSoundboardSoundUpdate, "GUILD_SOUNDBOARD_SOUND_UPDATE", Intents::GUILD_EMOJIS_AND_STICKERS;
    /// サーバーのサウンドボード音の削除通知を受け取ります。
    ///
    /// 必要な Intent は [`Self::on_guild_soundboard_sound_create`] と同じです。
    on_guild_soundboard_sound_delete, delete, GuildSoundboardSoundDelete, "GUILD_SOUNDBOARD_SOUND_DELETE", Intents::GUILD_EMOJIS_AND_STICKERS;
    /// サーバーの複数のサウンドボード音の更新通知を受け取ります。
    ///
    /// 必要な Intent は [`Self::on_guild_soundboard_sound_create`] と同じです。
    on_guild_soundboard_sounds_update, update_all, GuildSoundboardSoundsUpdate, "GUILD_SOUNDBOARD_SOUNDS_UPDATE", Intents::GUILD_EMOJIS_AND_STICKERS;
    /// サウンドボード一覧を要求したときの応答を受け取ります。
    ///
    /// 登録だけでは一覧を要求しません。
    on_soundboard_sounds, list, SoundboardSounds, "SOUNDBOARD_SOUNDS", Intents::empty();
    /// ボイスチャンネルのステータス変更の通知を受け取ります。
    ///
    /// `GUILDS` Intent が必要です。
    on_voice_channel_status_update, channel_status, VoiceChannelStatusUpdate, "VOICE_CHANNEL_STATUS_UPDATE", Intents::GUILDS;
    /// ボイスチャンネルのセッション開始時刻の変更通知を受け取ります。
    ///
    /// `GUILDS` Intent が必要です。
    on_voice_channel_start_time_update, channel_start, VoiceChannelStartTimeUpdate, "VOICE_CHANNEL_START_TIME_UPDATE", Intents::GUILDS;
    /// Gateway の `Request Channel Info` に対する応答を受け取ります。
    ///
    /// 登録だけでは情報を要求しません。
    on_channel_info, channel_info, ChannelInfo, "CHANNEL_INFO", Intents::empty();
}

#[cfg(test)]
mod tests;
