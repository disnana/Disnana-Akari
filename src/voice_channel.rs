//! ボイスチャンネルのステータスや開始時刻を受け取るための Gateway ペイロードです。

use serde::Deserialize;
use twilight_model::id::{
    Id,
    marker::{ChannelMarker, GuildMarker},
};

/// ボイスチャンネルのステータスが変更された通知です。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
pub struct VoiceChannelStatusUpdate {
    /// 更新されたチャンネルの ID。
    #[serde(rename = "id")]
    pub channel_id: Id<ChannelMarker>,
    /// チャンネルのサーバー ID。
    pub guild_id: Id<GuildMarker>,
    /// 設定後のステータス。解除された場合は `None`。
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub status: Option<String>,
}

/// ボイスチャンネルのセッション開始時刻が変更された通知です。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
pub struct VoiceChannelStartTimeUpdate {
    /// 更新されたチャンネルの ID。
    #[serde(rename = "id")]
    pub channel_id: Id<ChannelMarker>,
    /// チャンネルのサーバー ID。
    pub guild_id: Id<GuildMarker>,
    /// 現在の開始時刻。Discord が送った Unix 時刻（秒）です。
    pub voice_start_time: Option<i64>,
}

/// Gateway のチャンネル情報要求に対するサーバーごとの応答です。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
pub struct ChannelInfo {
    /// 要求対象のサーバー ID。
    pub guild_id: Id<GuildMarker>,
    /// 応答に含まれるチャンネルの情報。
    pub channels: Vec<VoiceChannelInfo>,
}

/// チャンネル情報要求で Discord が返す一時的なデータです。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
pub struct VoiceChannelInfo {
    /// チャンネルの ID。
    #[serde(rename = "id")]
    pub channel_id: Id<ChannelMarker>,
    /// Discord が返した現在のステータス。
    pub status: Option<String>,
    /// Discord が返した現在の開始時刻。Unix 時刻（秒）です。
    pub voice_start_time: Option<i64>,
}
