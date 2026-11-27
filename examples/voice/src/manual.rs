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
    control::{self, Control, MAX_UPLOAD_BYTES},
    receive::Recording,
    songbird_info,
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
    use disnana_akari::http::error::ErrorType;

    let source = report.source;
    let name = report
        .name
        .filter(|name| matches!(*name, "vc-join" | "vc-record" | "vc-stop" | "vc-leave"))
        .unwrap_or("VC");
    match report.error {
        Error::CommandUnavailable => eprintln!("[{source}] {name} に一致する登録経路がありません"),
        Error::Http(error) => {
            if let ErrorType::Response { status, .. } = error.kind() {
                eprintln!("[{source}] {name} の Discord HTTP ステータスは {status}");
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
    connection: Option<VoiceConnection>,
    recording: Option<(PathBuf, Recording)>,
    completed: Option<PathBuf>,
}

async fn run_session(
    voice: VoiceGateway,
    guild: Id<GuildMarker>,
    directory: &Path,
    mut receiver: mpsc::Receiver<Control>,
    mut gateway_stopped: oneshot::Receiver<()>,
) -> Result<()> {
    let mut session = Session {
        voice,
        guild,
        // Songbird 0.6.1 の接続中の設定変更は Decode の有無を切り替えられません。
        // 受信できる状態で接続し、録音ファイルへ渡すハンドラーだけを明示的に付け外しします。
        driver: Driver::new(
            Config::default().decode_mode(DecodeMode::Decode(DecodeConfig::default())),
        ),
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

async fn watch_recording(recording: Option<&mut (PathBuf, Recording)>) {
    match recording {
        Some((_, recording)) => recording.stopped().await,
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
            Control::Record { channel, reply } if !reply.is_closed() => {
                let message = match self.start_recording(channel, directory).await {
                    Ok(()) => "録音を開始しました。/vc-stop で終了してファイルを送信します。",
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
                    Ok(()) => "退出しました。録音があればローカルに保存しました。",
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
                if self.connect_driver(&info).await.is_err() {
                    let _ = self.disconnect(true).await;
                }
            }
            Some(VoiceConnectionStatus::AwaitingServer) => {
                self.driver.leave();
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
    ) -> std::result::Result<(), &'static str> {
        if self.recording.is_some() || self.completed.is_some() {
            return Err("先に /vc-stop で今の録音を確定してください。");
        }
        self.join(channel).await?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "録音ファイル名を作成できませんでした。")?
            .as_nanos();
        let path = directory.join(format!(
            "akari-voice-{timestamp}-{}.wav",
            std::process::id()
        ));
        let output = path.clone();
        let (recording, receiver) = tokio::task::spawn_blocking(move || {
            Recording::start_limited(&output, MAX_UPLOAD_BYTES)
        })
        .await
        .map_err(|_| "録音タスクを起動できませんでした。")?
        .map_err(|_| "録音ファイルを作成できませんでした。")?;

        self.driver
            .add_global_event(Event::Core(CoreEvent::VoiceTick), receiver);
        self.recording = Some((path, recording));
        Ok(())
    }

    async fn finish_recording(&mut self) -> Result<Option<PathBuf>> {
        let Some((path, recording)) = self.recording.take() else {
            return Ok(None);
        };
        self.driver.remove_all_global_events();
        let stats = recording.finish().await.map_err(Error::handler)?;
        println!(
            "録音フレーム {} 件、混雑で破棄 {} 件",
            stats.ticks_written, stats.ticks_dropped
        );
        Ok(Some(path))
    }

    async fn disconnect(&mut self, leave: bool) -> Result<()> {
        let connection = self.connection.take();
        self.driver.leave();
        let recording = self.finish_recording().await;
        self.completed = None;
        if leave && connection.is_some() {
            self.voice.leave(self.guild, LEAVE_TIMEOUT).await?;
        }
        recording?;
        Ok(())
    }
}
