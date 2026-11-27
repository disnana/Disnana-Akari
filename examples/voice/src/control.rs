use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use disnana_akari::{
    Result, SlashCommands, SlashContext,
    http::{error::ErrorType, response::StatusCode},
    model::{
        application::interaction::application_command::CommandOptionValue,
        channel::{ChannelType, message::AllowedMentions},
        guild::Permissions,
        http::attachment::Attachment,
        id::{
            Id,
            marker::{ChannelMarker, GuildMarker},
        },
    },
    validation::AttachmentLimits,
};
use tokio::{
    io::AsyncReadExt,
    sync::{mpsc, oneshot},
    time::timeout,
};

pub(super) const MAX_UPLOAD_BYTES: u32 = 10 * 1024 * 1024;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(70);
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

type ControlResult<T> = std::result::Result<T, &'static str>;

pub(super) enum Control {
    Join {
        channel: Id<ChannelMarker>,
        reply: oneshot::Sender<&'static str>,
    },
    Record {
        channel: Id<ChannelMarker>,
        reply: oneshot::Sender<&'static str>,
    },
    Stop {
        reply: oneshot::Sender<ControlResult<PathBuf>>,
    },
    Leave {
        reply: oneshot::Sender<&'static str>,
    },
}

pub(super) fn commands(guild: Id<GuildMarker>) -> Result<(SlashCommands, mpsc::Receiver<Control>)> {
    let (sender, receiver) = mpsc::channel(1);
    let mut commands = SlashCommands::for_guild(guild);

    for (name, description, record) in [
        ("vc-join", "ボイスチャンネルに参加します", false),
        ("vc-record", "ボイスチャンネルの録音を開始します", true),
    ] {
        let sender = sender.clone();
        let mut channel = SlashCommands::channel_option(
            "channel",
            "省略すると自分が参加中のボイスチャンネルを使います",
            false,
        );
        channel.channel_types = Some(vec![ChannelType::GuildVoice]);
        commands.register_with_permissions(
            name,
            description,
            vec![channel],
            Permissions::MANAGE_GUILD,
            async move |ctx: &mut SlashContext<'_>| join(ctx, &sender, record).await,
        )?;
    }
    let stop_sender = sender.clone();
    commands.register_with_permissions(
        "vc-stop",
        "録音を終了して、このチャンネルに WAV を送信します",
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
            ctx.defer_ephemeral().await?;
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
    record: bool,
) -> Result<()> {
    ctx.defer_ephemeral().await?;
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
        if record {
            Control::Record { channel, reply }
        } else {
            Control::Join { channel, reply }
        }
    })
    .await;
    ctx.reply_ephemeral(reply.unwrap_or_else(|message| message))
        .await
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

async fn stop(ctx: &mut SlashContext<'_>, sender: &mpsc::Sender<Control>) -> Result<()> {
    let Some(channel) = ctx.channel_id() else {
        return ctx
            .reply_ephemeral("WAV を送信するチャンネルを確認できませんでした。")
            .await;
    };
    ctx.defer_ephemeral().await?;
    let result = request(sender, |reply| Control::Stop { reply })
        .await
        .and_then(|result| result);
    let path = match result {
        Ok(path) => path,
        Err(message) => return ctx.reply_ephemeral(message).await,
    };
    let feedback = upload_recording(ctx, channel, &path)
        .await
        .unwrap_or_else(|message| message);
    ctx.reply_ephemeral(feedback).await
}

async fn upload_recording(
    ctx: &SlashContext<'_>,
    channel: Id<ChannelMarker>,
    path: &Path,
) -> ControlResult<&'static str> {
    const READ_FAILED: &str =
        "録音を停止しました。WAV を読み取れなかったため、ローカルの保存先を確認してください。";
    const TOO_LARGE: &str =
        "録音を停止しました。WAV は送信上限の 10 MiB を超えたため、ローカルに保存しました。";
    let file = tokio::fs::File::open(path).await.map_err(|_| READ_FAILED)?;
    let metadata = file.metadata().await.map_err(|_| READ_FAILED)?;
    if !metadata.is_file() {
        return Err(READ_FAILED);
    }
    let max_bytes = u64::from(MAX_UPLOAD_BYTES);
    let limits = AttachmentLimits::default().max_file_bytes(Some(max_bytes));
    if limits.validate(0, [metadata.len()], None).is_err() {
        return Err(TOO_LARGE);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| READ_FAILED)?;
    if limits.validate(0, [bytes.len() as u64], None).is_err() {
        return Err(TOO_LARGE);
    }
    let files = [Attachment::from_bytes("akari-voice.wav".into(), bytes, 0)];
    ctx.http()
        .create_message(channel)
        .attachments(&files)
        .allowed_mentions(Some(&AllowedMentions::default()))
        .await
        .map_err(
            |_| "録音を停止しました。WAV をチャンネルに送れなかったため、ローカルに保存しました。",
        )?;
    Ok("録音を停止し、このチャンネルに WAV を送信しました。")
}
