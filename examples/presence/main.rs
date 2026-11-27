//! 管理者だけが、Bot のステータスとアクティビティを変えられるコマンドのサンプル。
use std::env;

use disnana_akari::{
    Bot, Error, Presence, PresenceHandle, Result, SlashCommands, SlashContext,
    model::{
        gateway::presence::{ActivityType, MinimalActivity, Status},
        guild::Permissions,
        id::Id,
    },
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let guild_id = env::var("DISCORD_GUILD_ID")
        .map_err(Error::handler)?
        .parse::<Id<_>>()
        .map_err(Error::handler)?;
    let presence = PresenceHandle::new(Presence::new(Status::Online));
    let mut slash = SlashCommands::for_guild(guild_id);
    let mut status = SlashCommands::string_option("status", "変更後のオンライン状態", true);
    status.choices = Some(
        ["online", "idle", "dnd", "invisible"]
            .into_iter()
            .map(|value| SlashCommands::string_choice(value, value))
            .collect(),
    );
    let updates = presence.clone();
    slash.register_with_permissions(
        "akari-status",
        "Bot のステータスを変更します",
        vec![
            status,
            SlashCommands::string_option("activity", "プレイ中に表示する名前", false),
            SlashCommands::boolean_option("clear_activity", "アクティビティを消す", false),
        ],
        Permissions::ADMINISTRATOR,
        async move |ctx: &mut SlashContext<'_>| -> Result<()> {
            let status = match ctx.string("status") {
                Some("online") => Status::Online,
                Some("idle") => Status::Idle,
                Some("dnd") => Status::DoNotDisturb,
                Some("invisible") => Status::Invisible,
                _ => return ctx.reply_ephemeral("ステータスを選んでください。").await,
            };
            let activity = ctx
                .string("activity")
                .filter(|name| !name.trim().is_empty())
                .map(|name| MinimalActivity {
                    kind: ActivityType::Playing,
                    name: name.to_owned(),
                    url: None,
                });
            match (activity, ctx.boolean("clear_activity") == Some(true)) {
                (Some(_), true) => {
                    return ctx
                        .reply_ephemeral("activity と clear_activity は同時に指定できません。")
                        .await;
                }
                (Some(activity), false) => {
                    updates.set(Presence::new(status).with_activity(activity))
                }
                (None, true) => updates.set(Presence::new(status)),
                (None, false) => updates.set_status(status),
            }
            ctx.reply_ephemeral("ステータス変更を受け付けました。")
                .await
        },
    )?;

    Bot::from_env()?
        .with_presence(presence)
        .with_slash_commands(slash)
        .start()
        .await
}
