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
            let mut next = Presence::new(status);
            if let Some(name) = ctx
                .string("activity")
                .filter(|name| !name.trim().is_empty())
            {
                next = next.with_activity(MinimalActivity {
                    kind: ActivityType::Playing,
                    name: name.to_owned(),
                    url: None,
                });
            }
            updates.set(next);
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
