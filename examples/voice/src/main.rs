use std::{env, path::PathBuf, sync::Arc, time::Duration};

use disnana_akari::{
    Bot, Events, Ready, Result, VoiceConnection, VoiceConnectionInfo, VoiceConnectionStatus,
    VoiceGateway,
    model::{
        gateway::Intents,
        id::{
            Id,
            marker::{ChannelMarker, GuildMarker},
        },
    },
};
use songbird::{
    Config, ConnectionInfo, CoreEvent, Driver, Event,
    driver::{DecodeConfig, DecodeMode},
    input::{
        File, Input,
        codecs::{get_codec_registry, get_probe},
    },
};
use tokio::{
    sync::{Notify, oneshot},
    time::timeout,
};

mod control;
mod manual;
mod multitrack;
mod receive;
mod speaker;

const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);
const LEAVE_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> Result<()> {
    let guild = read_id("AKARI_VOICE_GUILD")?;
    if let Some(directory) = env::var_os("AKARI_VOICE_RECORD_DIR") {
        return manual::run(guild, directory.into()).await;
    }
    let channel = read_id("AKARI_VOICE_CHANNEL")?;
    let mode = match (
        env::var_os("AKARI_VOICE_WAV"),
        env::var_os("AKARI_VOICE_RECORD_WAV"),
    ) {
        (Some(input), None) => Mode::Playback(input.into()),
        (None, Some(output)) => Mode::Record(output.into()),
        _ => {
            return Err(disnana_akari::Error::msg(
                "AKARI_VOICE_WAV と AKARI_VOICE_RECORD_WAV のどちらか一方を設定してください",
            ));
        }
    };
    if let Mode::Playback(wav) = &mode
        && !tokio::fs::metadata(wav)
            .await
            .map_err(disnana_akari::Error::handler)?
            .is_file()
    {
        return Err(disnana_akari::Error::msg(
            "音声のパスはファイルを指定してください",
        ));
    }

    let ready = Arc::new(Notify::new());
    let mut events = Events::new();
    let seen_ready = Arc::clone(&ready);
    events.on_ready(async move |_: &Ready| -> Result<()> {
        seen_ready.notify_one();
        Ok(())
    });
    let (bot, voice) = Bot::from_env()?
        .with_events(events)
        .add_intents(Intents::GUILD_VOICE_STATES)
        .with_voice_gateway();
    let (bot, shutdown) = bot.with_shutdown();

    let (gateway_stopped, stop_player) = oneshot::channel();
    let runtime = bot.start();
    let player = play(&voice, &ready, guild, channel, &mode, stop_player);
    tokio::pin!(runtime, player);
    tokio::select! {
        result = &mut runtime => {
            let _ = gateway_stopped.send(());
            let finalized = player.await;
            result?;
            finalized
        }
        result = &mut player => {
            shutdown.request();
            runtime.await?;
            result
        }
    }
}

fn read_id<M>(name: &str) -> Result<Id<M>> {
    let id = env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(Id::new_checked);
    id.ok_or_else(|| {
        disnana_akari::Error::msg(format!("{name} に Discord の ID を指定してください"))
    })
}

enum Mode {
    Playback(PathBuf),
    Record(PathBuf),
}

async fn play(
    voice: &VoiceGateway,
    ready: &Notify,
    guild: Id<GuildMarker>,
    channel: Id<ChannelMarker>,
    mode: &Mode,
    stop_player: oneshot::Receiver<()>,
) -> Result<()> {
    let decode = match mode {
        Mode::Playback(_) => DecodeMode::Pass,
        Mode::Record(_) => DecodeMode::Decode(DecodeConfig::default()),
    };
    let mut driver = Driver::new(Config::default().decode_mode(decode));
    let mut recording = None;
    let mut join_requested = false;
    let mut gateway_ended = false;
    let result = tokio::select! {
        result = async {
            ready.notified().await;
            println!("Discord にボイスチャンネル {channel} への参加を要求します");
            join_requested = true;
            let mut connection = voice.join(guild, channel, CONNECTION_TIMEOUT).await?;
            if let Mode::Record(output) = mode {
                let output = output.clone();
                let (active, receiver) = tokio::task::spawn_blocking(move || receive::Recording::start(&output))
                    .await.map_err(disnana_akari::Error::handler)?
                    .map_err(disnana_akari::Error::handler)?;
                recording = Some(active);
                driver.add_global_event(Event::Core(CoreEvent::VoiceTick), receiver);
            }
            let follow = follow_allocations(&mut driver, &mut connection, mode);
            if let Some(active) = &mut recording {
                tokio::select! {
                    result = follow => result?,
                    _ = active.stopped() => return Ok(()),
                }
            } else {
                follow.await?;
            }
            join_requested = false;
            Ok(())
        } => result,
        signal = tokio::signal::ctrl_c() => signal.map_err(disnana_akari::Error::handler),
        _ = stop_player => { gateway_ended = true; Ok(()) },
    };
    driver.remove_all_global_events();
    driver.stop();
    driver.leave();

    let finalized = match recording {
        Some(active) => active
            .finish()
            .await
            .map(Some)
            .map_err(disnana_akari::Error::handler),
        None => Ok(None),
    };
    let cleanup = if join_requested && !gateway_ended {
        let result = voice.leave(guild, LEAVE_TIMEOUT).await;
        if result.is_ok() {
            println!("Discord からボイスチャンネルの退出を確認しました");
        }
        result
    } else {
        Ok(())
    };
    result?;
    if let Some(stats) = finalized? {
        println!(
            "録音フレーム {} 件、混雑で破棄 {} 件",
            stats.ticks_written, stats.ticks_dropped
        );
    }
    cleanup?;
    Ok(())
}

async fn follow_allocations(
    driver: &mut Driver,
    connection: &mut VoiceConnection,
    mode: &Mode,
) -> Result<()> {
    let mut status = connection.status();
    loop {
        match status {
            VoiceConnectionStatus::Ready(info) => {
                println!(
                    "ボイスチャンネル {} の音声サーバーへ接続中です",
                    info.channel_id
                );
                let ready_to_play = async {
                    let input = match mode {
                        Mode::Playback(wav) => {
                            Some(load_audio(File::new(wav.clone()).into()).await?)
                        }
                        Mode::Record(_) => None,
                    };
                    timeout(CONNECTION_TIMEOUT, driver.connect(songbird_info(&info)))
                        .await
                        .map_err(|_| {
                            disnana_akari::Error::msg("音声サーバーへの接続が時間切れです")
                        })?
                        .map_err(|_| {
                            disnana_akari::Error::msg("音声サーバーに接続できませんでした")
                        })?;
                    Ok::<_, disnana_akari::Error>(input)
                };
                let input = tokio::select! {
                    result = ready_to_play => result?,
                    replacement = connection.changed() => {
                        driver.leave();
                        status = replacement?;
                        continue;
                    }
                };
                println!("音声サーバーへ接続しました");
                if let Some(input) = input {
                    driver.play_only_input(input);
                }
            }
            VoiceConnectionStatus::AwaitingServer => {
                println!("音声サーバーの再割り当てを待っています")
            }
            VoiceConnectionStatus::Disconnected => {
                println!("ボイスチャンネルの接続管理を終了しました");
                return Ok(());
            }
            _ => {}
        }

        status = connection.changed().await?;
        // 接続が変わった場合は、古い再生を止め、新しい割り当てで先頭から再生します。
        driver.stop();
        driver.leave();
    }
}

async fn load_audio(input: Input) -> Result<Input> {
    input
        .make_playable_async(get_codec_registry(), get_probe())
        .await
        .map_err(|_| disnana_akari::Error::msg("WAV ファイルを読み取れませんでした"))
}

// Songbird の Twilight 統合を有効にせず、Akari が取得した接続情報だけを渡します。
fn songbird_info(info: &VoiceConnectionInfo) -> ConnectionInfo {
    ConnectionInfo {
        channel_id: info.channel_id.into_nonzero().into(),
        guild_id: info.guild_id.into_nonzero().into(),
        user_id: info.user_id.into_nonzero().into(),
        endpoint: info.endpoint.clone(),
        session_id: info.session_id().to_owned(),
        token: info.token().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pcm_wav_is_decoded_offline_and_corrupt_input_is_rejected() -> Result<()> {
        let mut wav = b"RIFF\x64\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x80\xBB\0\0\0\x77\x01\0\x02\0\x10\0data\x40\0\0\0".to_vec();
        wav.extend([0_u8; 64]);
        assert!(load_audio(Input::from(wav)).await?.is_playable());
        assert!(
            load_audio(Input::from(b"not audio".as_slice()))
                .await
                .is_err()
        );
        Ok(())
    }
}
