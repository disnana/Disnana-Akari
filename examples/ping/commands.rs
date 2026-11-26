use disnana_akari::{CommandContext, Commands, Result};

pub fn register() -> Result<Commands> {
    let mut commands = Commands::new("!")?;
    commands.register("ping", ping)?;
    commands.register("echo", echo)?;
    Ok(commands)
}

async fn ping(ctx: CommandContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}

async fn echo(ctx: CommandContext<'_>) -> Result<()> {
    let text = ctx.args();
    if text.is_empty() {
        return ctx.reply("使い方 `!echo <文章>`").await;
    }

    ctx.reply(text).await
}
