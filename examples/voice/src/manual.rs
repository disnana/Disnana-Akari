use std::{
    future::pending,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use disnana_akari::{
    Bot, Error, ErrorReport, InfoEvent, Messages, Result, VoiceConnection, VoiceConnectionStatus,
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
    Config, CoreEvent, Driver, Event,
    driver::{DecodeConfig, DecodeMode},
};
use tokio::{
    sync::{mpsc, oneshot},
    time::timeout,
};

use super::{
    CONNECTION_TIMEOUT, LEAVE_TIMEOUT,
    control::{self, Control, MAX_UPLOAD_BYTES, RecordingOptions},
    multitrack::{Recorder, TrackFile},
    playback::test_tone,
    songbird_info,
    speaker::Speakers,
};

pub(super) async fn run(guild: Id<GuildMarker>, directory: PathBuf) -> Result<()> {
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(Error::handler)?;
    let (commands, receiver) = control::commands(guild)?;
    let mut messages = Messages::default();
    messages.slash_command_unavailable = Some(
        "このコマンドを利用できませんでした。Discord のコマンド一覧を開き直して選び直してください。".into(),
    );
    let (bot, voice) = Bot::from_env()?
        .with_intents(Intents::GUILDS | Intents::GUILD_VOICE_STATES)
        .on_info(|event| match event {
            InfoEvent::SlashCommandsSynced { count, .. } => {
                println!("VC コマンドを {count} 件同期しました")
            }
            InfoEvent::GatewayReady => println!("Playground VC の Gateway が準備できました"),
            _ => {}
        })
        .with_messages(messages)
        .on_error(log_runtime_error)
        .with_slash_commands(commands)
        .with_voice_gateway();
    let (bot, shutdown) = bot.with_shutdown();
    let (gateway_stopped, stop_session) = oneshot::channel();
    let bot = bot.start();
    let session = run_session(voice, guild, &directory, receiver, stop_session);
    tokio::pin!(bot, session);
    tokio::select! {
        result = &mut bot => {
            let _ = gateway_stopped.send(());
            let cleanup = session.await;
            result?;
            cleanup
        }
        result = &mut session => {
            shutdown.request();
            bot.await?;
            result
        }
    }
}

fn log_runtime_error(report: &ErrorReport<'_>) {
    use disnana_akari::http::{api_error::ApiError, error::ErrorType};

    let source = report.source;
    let name = report
        .name
        .filter(|name| {
            matches!(
                *name,
                "vc-join" | "vc-record" | "vc-stop" | "vc-leave" | "vc-playtest"
            )
        })
        .unwrap_or("VC");
    match report.error {
        Error::CommandUnavailable => eprintln!("[{source}] {name} に一致する登録経路がありません"),
        Error::Http(error) => {
            if let ErrorType::Response { status, error, .. } = error.kind() {
                if let ApiError::General(error) = error {
                    eprintln!(
                        "[{source}] {name} の Discord HTTP {status}、エラー番号 {}",
                        error.code
                    );
                } else {
                    eprintln!("[{source}] {name} の Discord HTTP ステータスは {status}");
                }
            } else {
                eprintln!("[{source}] {name} の Discord HTTP 通信に失敗しました");
            }
        }
        _ => eprintln!("[{source}] {name} の操作に失敗しました"),
    }
}

struct Session {
    voice: VoiceGateway,
    guild: Id<GuildMarker>,
    driver: Driver,
    speakers: Speakers,
    connection: Option<VoiceConnection>,
    recording: Option<Recorder>,
    completed: Option<Vec<TrackFile>>,
}

fn voice_driver() -> (Driver, Speakers) {
    // Songbird 0.6.1 では既存の接続で Decode の有無を切り替えられません。
    let mut driver =
        Driver::new(Config::default().decode_mode(DecodeMode::Decode(DecodeConfig::default())));
    let speakers = Speakers::install(&mut driver);
    (driver, speakers)
}

async fn run_session(
    voice: VoiceGateway,
    guild: Id<GuildMarker>,
    directory: &Path,
    mut receiver: mpsc::Receiver<Control>,
    mut gateway_stopped: oneshot::Receiver<()>,
) -> Result<()> {
    let (driver, speakers) = voice_driver();
    let mut session = Session {
        voice,
        guild,
        driver,
        speakers,
        connection: None,
        recording: None,
        completed: None,
    };
    println!("VC 操作コマンドを登録します");
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    loop {
        tokio::select! {
            command = receiver.recv() => match command {
                Some(command) => session.handle(command, directory).await,
                None => break,
            },
            status = watch_connection(session.connection.as_mut()) => {
                session.apply_status(status).await;
            }
            () = watch_recording(session.recording.as_mut()) => {
                match session.finish_recording().await {
                    Ok(file) => {
                        session.completed = file;
                        println!("録音が上限に達しました。/vc-stop で送信できます");
                    }
                    Err(error) => eprintln!("録音が停止しました: {error}"),
                }
            }
            result = &mut interrupt => {
                let cleanup = session.disconnect(true).await;
                result.map_err(Error::handler)?;
                return cleanup;
            }
            _ = &mut gateway_stopped => return session.disconnect(false).await,
        }
    }
    session.disconnect(true).await
}

async fn watch_connection(
    connection: Option<&mut VoiceConnection>,
) -> Option<VoiceConnectionStatus> {
    match connection {
        Some(connection) => connection.changed().await.ok(),
        None => pending().await,
    }
}

async fn watch_recording(recording: Option<&mut Recorder>) {
    match recording {
        Some(recording) => recording.stopped().await,
        None => pending().await,
    }
}

impl Session {
    async fn handle(&mut self, command: Control, directory: &Path) {
        match command {
            Control::Join { channel, reply } if !reply.is_closed() => {
                let message = match self.join(channel).await {
                    Ok(()) => "ボイスチャンネルに接続しました。",
                    Err(message) => message,
                };
                let _ = reply.send(message);
            }
            Control::Record {
                channel,
                options,
                reply,
            } if !reply.is_closed() => {
                let message = match self.start_recording(channel, directory, options).await {
                    Ok(()) => "録音を開始しました。/vc-stop で終了してファイルを送信します。",
                    Err(message) => message,
                };
                let _ = reply.send(message);
            }
            Control::PlayTest { channel, reply } if !reply.is_closed() => {
                let message = match self.join(channel).await {
                    Ok(()) => match test_tone().await {
                        Ok(input) => {
                            self.driver.play_only_input(input);
                            "短いテスト音を再生します。"
                        }
                        Err(_) => "テスト音を準備できませんでした。",
                    },
                    Err(message) => message,
                };
                let _ = reply.send(message);
            }
            Control::Stop { reply } if !reply.is_closed() => {
                let file = match self.finish_recording().await {
                    Ok(file) => file
                        .or_else(|| self.completed.take())
                        .ok_or("録音中の音声がありません。"),
                    Err(_) => Err("録音の確定に失敗しました。ローカルファイルを確認してください。"),
                };
                let _ = reply.send(file);
            }
            Control::Leave { reply } if !reply.is_closed() => {
                let message = match self.disconnect(true).await {
                    Ok(()) => "退出しました。録音が残っていれば /vc-stop で送信できます。",
                    Err(_) => "退出または録音の確定を確認できませんでした。",
                };
                let _ = reply.send(message);
            }
            _ => {}
        }
    }

    async fn join(&mut self, channel: Id<ChannelMarker>) -> std::result::Result<(), &'static str> {
        if let Some(connection) = &self.connection {
            return match connection.status() {
                VoiceConnectionStatus::Ready(info) if info.channel_id == channel => Ok(()),
                _ => {
                    Err("別の VC に接続中、または再接続中です。先に /vc-leave を実行してください。")
                }
            };
        }

        let connection = self
            .voice
            .join(self.guild, channel, CONNECTION_TIMEOUT)
            .await;
        if let Ok(connection) = connection {
            let status = connection.status();
            self.connection = Some(connection);
            if let VoiceConnectionStatus::Ready(info) = status
                && self.connect_driver(&info).await.is_ok()
            {
                return Ok(());
            }
        } else {
            // 参加要求後に時間切れになった場合も、遅れて参加したまま残らないよう退出を要求します。
            let _ = self.voice.leave(self.guild, LEAVE_TIMEOUT).await;
        }
        let _ = self.disconnect(true).await;
        Err("VC に接続できませんでした。Bot の接続権限や Gateway の状態を確認してください。")
    }

    async fn connect_driver(&mut self, info: &super::VoiceConnectionInfo) -> Result<()> {
        (self.driver, self.speakers) = voice_driver();
        timeout(CONNECTION_TIMEOUT, self.driver.connect(songbird_info(info)))
            .await
            .map_err(|_| Error::msg("音声サーバーへの接続が時間切れです"))?
            .map_err(|_| Error::msg("音声サーバーに接続できませんでした"))?;
        println!("音声サーバーへ接続しました。参加先 {}", info.channel_id);
        Ok(())
    }

    async fn apply_status(&mut self, status: Option<VoiceConnectionStatus>) {
        match status {
            Some(VoiceConnectionStatus::Ready(info)) => {
                self.driver.leave();
                self.preserve_recording().await;
                if self.connect_driver(&info).await.is_err() {
                    let _ = self.disconnect(true).await;
                }
            }
            Some(VoiceConnectionStatus::AwaitingServer) => {
                self.driver.leave();
                self.preserve_recording().await;
                println!("音声サーバーの再割り当てを待っています");
            }
            _ => {
                let _ = self.disconnect(false).await;
                println!("ボイスチャンネルから切断されました");
            }
        }
    }

    async fn start_recording(
        &mut self,
        channel: Id<ChannelMarker>,
        directory: &Path,
        options: RecordingOptions,
    ) -> std::result::Result<(), &'static str> {
        if self.recording.is_some() || self.completed.is_some() {
            return Err("先に /vc-stop で今の録音を確定してください。");
        }
        self.join(channel).await?;
        let destination = if options.memory {
            None
        } else {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| "録音ファイル名を作成できませんでした。")?
                .as_nanos();
            Some(directory.join(format!("akari-voice-{timestamp}-{}", std::process::id())))
        };
        let speakers = self.speakers.clone();
        let (recording, receiver) = tokio::task::spawn_blocking(move || {
            Recorder::start(
                destination.as_deref(),
                options.mix,
                options.users,
                MAX_UPLOAD_BYTES,
                options.format,
                speakers,
            )
        })
        .await
        .map_err(|_| "録音タスクを起動できませんでした。")?
        .map_err(|_| "録音先を準備できませんでした。")?;

        self.driver
            .add_global_event(Event::Core(CoreEvent::VoiceTick), receiver);
        self.recording = Some(recording);
        Ok(())
    }

    async fn finish_recording(&mut self) -> Result<Option<Vec<TrackFile>>> {
        let Some(recording) = self.recording.take() else {
            return Ok(None);
        };
        let stats = recording.finish().await.map_err(Error::handler)?;
        println!(
            "録音フレーム {} 件、混雑で破棄 {} 件",
            stats.ticks_written, stats.ticks_dropped
        );
        Ok((!stats.tracks.is_empty()).then_some(stats.tracks))
    }

    async fn preserve_recording(&mut self) {
        match self.finish_recording().await {
            Ok(Some(tracks)) => self.completed = Some(tracks),
            Ok(None) => {}
            Err(error) => eprintln!("録音の確定に失敗しました: {error}"),
        }
    }

    async fn disconnect(&mut self, leave: bool) -> Result<()> {
        let connection = self.connection.take();
        self.driver.leave();
        let recording = self.finish_recording().await.map(|tracks| {
            if tracks.is_some() {
                self.completed = tracks;
            }
        });
        if leave && connection.is_some() {
            self.voice.leave(self.guild, LEAVE_TIMEOUT).await?;
        }
        recording
    }
}
