//! Discord の既定・サーバー固有のサウンドボード音を取得し、再生します。

use serde::{Deserialize, Serialize};
use twilight_http::{
    Client,
    request::{Method, RequestBuilder},
};
use twilight_model::id::{
    Id,
    marker::{ChannelMarker, GuildMarker},
};

use crate::{Result, SoundboardSound, SoundboardSoundId};

/// Discord の既定のサウンドボード音を取得します。
///
/// Bot が共有している HTTP クライアントを渡せます。キャッシュは使いません。
pub async fn default_sounds(http: &Client) -> Result<Vec<SoundboardSound>> {
    let request =
        RequestBuilder::raw(Method::Get, "soundboard-default-sounds".to_owned()).build()?;
    Ok(http
        .request::<Vec<SoundboardSound>>(request)
        .await?
        .model()
        .await?)
}

/// 指定したサーバーのサウンドボード音を取得します。
///
/// `CREATE_GUILD_EXPRESSIONS` または `MANAGE_GUILD_EXPRESSIONS` 権限があれば、
/// Discord は音の作成者も返します。
pub async fn guild_sounds(http: &Client, guild: Id<GuildMarker>) -> Result<Vec<SoundboardSound>> {
    #[derive(Deserialize)]
    struct GuildSounds {
        items: Vec<SoundboardSound>,
    }

    let request =
        RequestBuilder::raw(Method::Get, format!("guilds/{guild}/soundboard-sounds")).build()?;
    Ok(http
        .request::<GuildSounds>(request)
        .await?
        .model()
        .await?
        .items)
}

/// Bot が接続中のボイスチャンネルで既存のサウンドボード音を再生します。
///
/// 同じサーバーの音や既定音を再生するときは `source_guild` に `None` を渡します。
/// 別サーバーの音には、その音があるサーバーの ID を渡します。
/// Discord は `SPEAK` と `USE_SOUNDBOARD` 権限を確認します。別サーバーの音には
/// `USE_EXTERNAL_SOUNDS` も必要です。Bot がミュート、スピーカーミュート、または発言抑制中の
/// 場合、Discord は再生を拒否します。
pub async fn play(
    http: &Client,
    channel: Id<ChannelMarker>,
    sound: SoundboardSoundId,
    source_guild: Option<Id<GuildMarker>>,
) -> Result<()> {
    #[derive(Serialize)]
    struct Play {
        sound_id: SoundboardSoundId,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_guild_id: Option<Id<GuildMarker>>,
    }

    let request = RequestBuilder::raw(
        Method::Post,
        format!("channels/{channel}/send-soundboard-sound"),
    )
    .json(&Play {
        sound_id: sound,
        source_guild_id: source_guild,
    })
    .build()?;
    http.request::<()>(request).await?;
    Ok(())
}

#[cfg(test)]
mod tests;
