use std::{
    path::Path,
    time::{Duration, Instant},
};

use disnana_akari::{
    Result, SlashCommands, SlashContext,
    http::{error::ErrorType, response::StatusCode},
    model::{
        application::command::{CommandOptionChoice, CommandOptionChoiceValue},
        application::interaction::application_command::CommandOptionValue,
        channel::{ChannelType, message::AllowedMentions},
        guild::Permissions,
        http::attachment::Attachment,
        id::{
            Id,
            marker::{ChannelMarker, GuildMarker, UserMarker},
        },
    },
    validation::AttachmentLimits,
};
use tokio::{
    fs::File,
    io::AsyncReadExt,
    sync::{mpsc, oneshot},
    time::timeout,
};

use super::multitrack::{Format, TrackFile};

pub(super) const MAX_UPLOAD_BYTES: u32 = 10 * 1024 * 1024;
pub(super) const MAX_RECORD_USERS: usize = 4;
const USER_OPTIONS: [&str; MAX_RECORD_USERS] = ["user1", "user2", "user3", "user4"];
const CONTROL_TIMEOUT: Duration = Duration::from_secs(70);
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);
const READ_FAILED: &str = "録音を停止しました。録音ファイル を読み取れませんでした。ディスク録音は保存先に残ります。メモリ録音にはローカルファイルがありません。";
const TOO_LARGE: &str = "録音を停止しました。録音ファイル の合計が送信上限の 10 MiB を超えました。ディスク録音は保存先に残ります。メモリ録音にはローカルファイルがありません。";

type ControlResult<T> = std::result::Result<T, &'static str>;

pub(super) enum Control {
    Join {
        channel: Id<ChannelMarker>,
        reply: oneshot::Sender<&'static str>,
    },
    Record {
        channel: Id<ChannelMarker>,
        options: RecordingOptions,
        reply: oneshot::Sender<&'static str>,
    },
    PlayTest {
        channel: Id<ChannelMarker>,
        reply: oneshot::Sender<&'static str>,
    },
    Stop {
        reply: oneshot::Sender<ControlResult<Vec<TrackFile>>>,
    },
    Leave {
        reply: oneshot::Sender<&'static str>,
    },
}

#[derive(Clone, Copy)]
enum CommandMode {
    Join,
    Record,
    PlayTest,
}

impl CommandMode {
    fn label(self) -> &'static str {
        match self {
            Self::Join => "vc-join",
            Self::Record => "vc-record",
            Self::PlayTest => "vc-playtest",
        }
    }
}

pub(super) fn commands(guild: Id<GuildMarker>) -> Result<(SlashCommands, mpsc::Receiver<Control>)> {
    let (sender, receiver) = mpsc::channel(1);
    let mut commands = SlashCommands::for_guild(guild);

    for (mode, description) in [
        (CommandMode::Join, "ボイスチャンネルに参加します"),
        (CommandMode::Record, "ボイスチャンネルの録音を開始します"),
        (
            CommandMode::PlayTest,
            "ボイスチャンネルで再生テストを行います",
        ),
    ] {
        let name = mode.label();
        let sender = sender.clone();
        let mut channel = SlashCommands::channel_option(
            "channel",
            "省略すると自分が参加中のボイスチャンネルを使います",
            false,
        );
        channel.channel_types = Some(vec![ChannelType::GuildVoice]);
        let mut options = vec![channel];
        if matches!(mode, CommandMode::Record) {
            options.extend(USER_OPTIONS.iter().enumerate().map(|(index, name)| {
                SlashCommands::user_option(
                    name,
                    &format!("個別に録音するユーザー {}", index + 1),
                    false,
                )
            }));
            options.push(SlashCommands::boolean_option(
                "mix",
                "全体のミックスを録音します（既定は有効）",
                false,
            ));
            options.push(SlashCommands::boolean_option(
                "memory",
                "メモリに録音します（既定はディスクに保存）",
                false,
            ));
            let mut format =
                SlashCommands::string_option("format", "保存形式（既定は WAV）", false);
            format.choices = Some(
                ["WAV", "FLAC"]
                    .map(|name| CommandOptionChoice {
                        name: name.into(),
                        name_localizations: None,
                        value: CommandOptionChoiceValue::String(name.to_ascii_lowercase()),
                    })
                    .to_vec(),
            );
            options.push(format);
        }
        commands.register_with_permissions(
            name,
            description,
            options,
            Permissions::MANAGE_GUILD,
            async move |ctx: &mut SlashContext<'_>| join(ctx, &sender, mode).await,
        )?;
    }
    let stop_sender = sender.clone();
    commands.register_with_permissions(
        "vc-stop",
        "録音を終了して、このチャンネルに 録音ファイル を送信します",
        vec![],
        Permissions::MANAGE_GUILD,
        async move |ctx: &mut SlashContext<'_>| stop(ctx, &stop_sender).await,
    )?;
    commands.register_with_permissions(
        "vc-leave",
        "ボイスチャンネルから退出します",
        vec![],
        Permissions::MANAGE_GUILD,
        async move |ctx: &mut SlashContext<'_>| -> Result<()> {
            defer_control(ctx, "vc-leave").await?;
            let reply = request(&sender, |reply| Control::Leave { reply }).await;
            ctx.reply_ephemeral(reply.unwrap_or_else(|message| message))
                .await
        },
    )?;
    Ok((commands, receiver))
}

async fn join(
    ctx: &mut SlashContext<'_>,
    sender: &mpsc::Sender<Control>,
    mode: CommandMode,
) -> Result<()> {
    let recording = if matches!(mode, CommandMode::Record) {
        match record_options(|name| ctx.option(name)) {
            Ok(options) => Some(options),
            Err(message) => return ctx.reply_ephemeral(message).await,
        }
    } else {
        None
    };
    defer_control(ctx, mode.label()).await?;
    let channel = match timeout(LOOKUP_TIMEOUT, target_channel(ctx)).await {
        Ok(Ok(channel)) => channel,
        Ok(Err(message)) => return ctx.reply_ephemeral(message).await,
        Err(_) => {
            return ctx
                .reply_ephemeral("ボイスチャンネルの確認が時間切れになりました。")
                .await;
        }
    };
    let reply = request(sender, |reply| {
        if let Some(options) = recording {
            Control::Record {
                channel,
                options,
                reply,
            }
        } else if matches!(mode, CommandMode::PlayTest) {
            Control::PlayTest { channel, reply }
        } else {
            Control::Join { channel, reply }
        }
    })
    .await;
    ctx.reply_ephemeral(reply.unwrap_or_else(|message| message))
        .await
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct RecordingOptions {
    pub(super) mix: bool,
    pub(super) users: Vec<Id<UserMarker>>,
    pub(super) memory: bool,
    pub(super) format: Format,
}

fn record_options<'a>(
    mut option: impl FnMut(&str) -> Option<&'a CommandOptionValue>,
) -> ControlResult<RecordingOptions> {
    let mix = match option("mix") {
        Some(CommandOptionValue::Boolean(value)) => *value,
        None => true,
        Some(_) => return Err("mix には true または false を指定してください。"),
    };
    let memory = match option("memory") {
        Some(CommandOptionValue::Boolean(value)) => *value,
        None => false,
        Some(_) => return Err("memory には true または false を指定してください。"),
    };
    let format = match option("format") {
        Some(CommandOptionValue::String(value)) if value == "flac" => Format::Flac,
        Some(CommandOptionValue::String(value)) if value == "wav" => Format::Wav,
        None => Format::Wav,
        Some(_) => return Err("format には WAV または FLAC を指定してください。"),
    };
    let mut users = Vec::new();
    for name in USER_OPTIONS {
        match option(name) {
            Some(CommandOptionValue::User(user)) if !users.contains(user) => users.push(*user),
            Some(CommandOptionValue::User(_)) => {
                return Err("同じユーザーが複数回指定されています。");
            }
            Some(_) => return Err("録音するユーザーを選択してください。"),
            None => {}
        }
    }
    if !mix && users.is_empty() {
        return Err("mix を無効にする場合は、録音するユーザーを選択してください。");
    }
    Ok(RecordingOptions {
        mix,
        users,
        memory,
        format,
    })
}

async fn target_channel(ctx: &SlashContext<'_>) -> ControlResult<Id<ChannelMarker>> {
    if let Some(option) = ctx.option("channel") {
        let CommandOptionValue::Channel(channel) = option else {
            return Err("ボイスチャンネルを指定してください。");
        };
        if ctx
            .resolved()
            .and_then(|value| value.channels.get(channel))
            .is_some_and(|value| value.kind != ChannelType::GuildVoice)
        {
            return Err("通常のボイスチャンネルを指定してください。");
        }
        return Ok(*channel);
    }

    const JOIN_FIRST: &str = "ボイスチャンネルに参加するか、channel を指定してください。";
    const LOOKUP_FAILED: &str = "参加中のボイスチャンネルを確認できませんでした。";
    let guild = ctx.guild_id().ok_or(LOOKUP_FAILED)?;
    let user = ctx.author_id().ok_or(LOOKUP_FAILED)?;
    let response = match ctx.http().user_voice_state(guild, user).await {
        Ok(response) => response,
        Err(error) if matches!(error.kind(), ErrorType::Response { status, .. } if *status == StatusCode::NOT_FOUND) =>
        {
            return Err(JOIN_FIRST);
        }
        Err(_) => return Err(LOOKUP_FAILED),
    };
    let state = response.model().await.map_err(|_| LOOKUP_FAILED)?;
    let channel = state.channel_id.ok_or(JOIN_FIRST)?;
    let actual = ctx
        .http()
        .channel(channel)
        .await
        .map_err(|_| LOOKUP_FAILED)?
        .model()
        .await
        .map_err(|_| LOOKUP_FAILED)?;
    if actual.kind != ChannelType::GuildVoice {
        return Err("通常のボイスチャンネルに参加するか、channel を指定してください。");
    }
    Ok(channel)
}

async fn request<T>(
    sender: &mpsc::Sender<Control>,
    command: impl FnOnce(oneshot::Sender<T>) -> Control,
) -> ControlResult<T> {
    let (reply, completed) = oneshot::channel();
    match sender.try_send(command(reply)) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            return Err("別の音声操作を受け付けているため、少し待ってから再実行してください。");
        }
        Err(mpsc::error::TrySendError::Closed(_)) => return Err("音声機能を利用できません。"),
    }
    match timeout(CONTROL_TIMEOUT, completed).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(_)) => Err("音声操作の結果を取得できませんでした。"),
        Err(_) => Err("音声操作の完了を確認できませんでした。"),
    }
}

async fn defer_control(ctx: &mut SlashContext<'_>, operation: &'static str) -> Result<()> {
    let timing = ctx.timing();
    let before = timing.handler_elapsed();
    let started = Instant::now();
    let result = ctx.defer_ephemeral().await;
    if result.is_err() {
        eprintln!(
            "[{operation}] defer に失敗しました。キュー待機 {} ms、defer 前 {} ms、defer {} ms",
            timing.queue_wait().as_millis(),
            before.as_millis(),
            started.elapsed().as_millis(),
        );
    }
    result
}

async fn stop(ctx: &mut SlashContext<'_>, sender: &mpsc::Sender<Control>) -> Result<()> {
    let Some(channel) = ctx.channel_id() else {
        return ctx
            .reply_ephemeral("録音ファイル を送信するチャンネルを確認できませんでした。")
            .await;
    };
    defer_control(ctx, "vc-stop").await?;
    let result = request(sender, |reply| Control::Stop { reply })
        .await
        .and_then(|result| result);
    let tracks = match result {
        Ok(tracks) => tracks,
        Err(message) => return ctx.reply_ephemeral(message).await,
    };
    let feedback = upload_recordings(ctx, channel, tracks)
        .await
        .unwrap_or_else(|message| message);
    ctx.reply_ephemeral(feedback).await
}

async fn upload_recordings(
    ctx: &SlashContext<'_>,
    channel: Id<ChannelMarker>,
    tracks: Vec<TrackFile>,
) -> ControlResult<&'static str> {
    let files = load_recordings(tracks).await?;
    ctx.http()
        .create_message(channel)
        .attachments(&files)
        .allowed_mentions(Some(&AllowedMentions::default()))
        .await
        .map_err(
            |_| "録音を停止しました。録音ファイル をチャンネルに送れませんでした。ディスク録音は保存先に残ります。メモリ録音にはローカルファイルがありません。",
        )?;
    Ok("録音を停止し、このチャンネルに 録音ファイル を送信しました。")
}

struct RecordingFile {
    name: String,
    data: RecordingData,
}

enum RecordingData {
    Disk { file: File, length: u64 },
    Memory(Vec<u8>),
}

impl RecordingData {
    fn length(&self) -> u64 {
        match self {
            Self::Disk { length, .. } => *length,
            Self::Memory(bytes) => bytes.len() as u64,
        }
    }

    async fn bytes(self) -> ControlResult<Vec<u8>> {
        match self {
            Self::Memory(bytes) => Ok(bytes),
            Self::Disk { mut file, length } => {
                let mut bytes = vec![0; length as usize];
                // サイズ確認後にファイルが変わっても、元の長さ + 1 バイトで読み取りを止める。
                file.read_exact(&mut bytes).await.map_err(|_| READ_FAILED)?;
                if file.read(&mut [0]).await.map_err(|_| READ_FAILED)? != 0 {
                    return Err(READ_FAILED);
                }
                Ok(bytes)
            }
        }
    }
}

async fn load_recordings(tracks: Vec<TrackFile>) -> ControlResult<Vec<Attachment>> {
    if tracks.is_empty() || tracks.len() > MAX_RECORD_USERS + 1 {
        return Err(READ_FAILED);
    }
    let mut opened: Vec<RecordingFile> = Vec::with_capacity(tracks.len());
    for track in tracks {
        let entry = match track {
            TrackFile::Memory { name, bytes } if valid_recording_name(&name) => RecordingFile {
                name,
                data: RecordingData::Memory(bytes),
            },
            TrackFile::Memory { .. } => return Err(READ_FAILED),
            TrackFile::Disk(path) => open_recording(&path).await?,
        };
        if opened.iter().any(|existing| existing.name == entry.name) {
            return Err(READ_FAILED);
        }
        opened.push(entry);
    }
    AttachmentLimits::default()
        .max_total_bytes(Some(u64::from(MAX_UPLOAD_BYTES)))
        .validate(0, opened.iter().map(|entry| entry.data.length()), None)
        .map_err(|_| TOO_LARGE)?;

    let mut files = Vec::with_capacity(opened.len());
    for (index, entry) in opened.into_iter().enumerate() {
        let bytes = entry.data.bytes().await?;
        files.push(Attachment::from_bytes(entry.name, bytes, index as u64));
    }
    Ok(files)
}

async fn open_recording(path: &Path) -> ControlResult<RecordingFile> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| valid_recording_name(name))
        .ok_or(READ_FAILED)?;
    let file_type = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|_| READ_FAILED)?
        .file_type();
    if !file_type.is_file() {
        return Err(READ_FAILED);
    }
    let file = File::open(path).await.map_err(|_| READ_FAILED)?;
    let metadata = file.metadata().await.map_err(|_| READ_FAILED)?;
    if !metadata.is_file() {
        return Err(READ_FAILED);
    }
    Ok(RecordingFile {
        name: name.into(),
        data: RecordingData::Disk {
            file,
            length: metadata.len(),
        },
    })
}

fn valid_recording_name(name: &str) -> bool {
    let Some((stem, "wav" | "flac")) = name.rsplit_once('.') else {
        return false;
    };
    if stem == "mix" {
        return true;
    }
    stem.strip_prefix("user-").is_some_and(|user| {
        !user.starts_with('0')
            && user.bytes().all(|b| b.is_ascii_digit())
            && user.parse::<u64>().is_ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_defaults_and_user_selection_are_checked() {
        assert_eq!(
            record_options(|_| None),
            Ok(RecordingOptions {
                mix: true,
                users: vec![],
                memory: false,
                format: Format::Wav,
            })
        );
        let disabled = CommandOptionValue::Boolean(false);
        let enabled = CommandOptionValue::Boolean(true);
        let flac = CommandOptionValue::String("flac".into());
        let one = CommandOptionValue::User(Id::new(7));
        let two = CommandOptionValue::User(Id::new(9));
        let result = record_options(|name| match name {
            "mix" => Some(&disabled),
            "memory" => Some(&enabled),
            "format" => Some(&flac),
            "user2" => Some(&two),
            "user4" => Some(&one),
            _ => None,
        });
        assert_eq!(
            result,
            Ok(RecordingOptions {
                mix: false,
                users: vec![Id::new(9), Id::new(7)],
                memory: true,
                format: Format::Flac,
            })
        );
        assert!(record_options(|name| (name == "mix").then_some(&disabled)).is_err());
        assert!(record_options(|name| matches!(name, "user1" | "user3").then_some(&one)).is_err());
        assert!(record_options(|name| (name == "user1").then_some(&enabled)).is_err());
    }

    fn memory(name: &str, bytes: Vec<u8>) -> TrackFile {
        TrackFile::Memory {
            name: name.into(),
            bytes,
        }
    }

    #[tokio::test]
    async fn upload_uses_safe_filenames_and_moves_memory_buffers() {
        let bytes = vec![1, 2, 3];
        let original = bytes.as_ptr();
        let uploads = load_recordings(vec![
            memory("mix.wav", bytes),
            memory("user-7.flac", vec![4]),
        ])
        .await
        .unwrap();
        assert_eq!(
            uploads
                .iter()
                .map(|file| (&*file.filename, file.id))
                .collect::<Vec<_>>(),
            [("mix.wav", 0), ("user-7.flac", 1)]
        );
        assert_eq!(uploads[0].file.as_ptr(), original);
        for invalid in [
            "../mix.wav",
            "user-0.wav",
            "user-07.wav",
            "user-18446744073709551616.wav",
            "mix.mp3",
        ] {
            assert!(
                load_recordings(vec![memory(invalid, vec![1])])
                    .await
                    .is_err()
            );
        }
        assert!(
            load_recordings(vec![memory("mix.wav", vec![1]), memory("mix.wav", vec![2])])
                .await
                .is_err()
        );
        assert!(load_recordings(vec![]).await.is_err());
        let tracks = (1..=(MAX_RECORD_USERS + 2))
            .map(|user| memory(&format!("user-{user}.wav"), vec![0]))
            .collect();
        assert!(load_recordings(tracks).await.is_err());
    }

    #[tokio::test]
    async fn aggregate_limit_covers_disk_and_memory_and_keeps_the_disk_file() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("akari-control-{}-{suffix}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("mix.wav");
        std::fs::write(&path, [1, 2, 3]).unwrap();

        let too_large = load_recordings(vec![
            TrackFile::Disk(path.clone()),
            memory("user-7.wav", vec![0; MAX_UPLOAD_BYTES as usize]),
        ])
        .await;
        let existing = std::fs::read(&path).unwrap();
        let valid = load_recordings(vec![
            TrackFile::Disk(path.clone()),
            memory("user-7.wav", vec![4]),
        ])
        .await;
        let changed = open_recording(&path).await.unwrap();
        std::fs::write(&path, [1, 2, 3, 4]).unwrap();
        let changed = changed.data.bytes().await;
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&root).unwrap();

        assert_eq!(too_large.unwrap_err(), TOO_LARGE);
        assert_eq!(existing, [1, 2, 3]);
        assert_eq!(valid.unwrap()[0].file, [1, 2, 3]);
        assert_eq!(changed.unwrap_err(), READ_FAILED);
    }
}
