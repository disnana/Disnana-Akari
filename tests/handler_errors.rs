//! ハンドラーが、任意のエラー型を返せることを、公開 API だけで確かめる。
//! 登録できること（コンパイルできること）が、このテストの中心。

use std::{error::Error as StdError, io, num::ParseIntError};

use disnana_akari::model::id::Id;
use disnana_akari::{
    Bot, ButtonContext, CommandContext, Commands, Context, Error, Events, HybridContext,
    ModalContext, Ready, Result, SlashCommands, SlashContext, register_hybrid,
};

// Akari の Result を返す（これまでどおり）。Akari 自身のエラーは ? でそのまま通る。
async fn akari_result(ctx: CommandContext<'_>) -> Result<()> {
    ctx.reply("ok").await?;
    Ok(())
}

// 標準のエラーを、そのまま ? で返せる。
async fn parse_error(ctx: CommandContext<'_>) -> Result<(), ParseIntError> {
    let _count: i64 = ctx.args().parse()?;
    Ok(())
}

async fn io_error(_: CommandContext<'_>) -> Result<(), io::Error> {
    let _text = std::fs::read_to_string("does-not-exist.txt")?;
    Ok(())
}

// Box<dyn Error> を返す関数にも、Akari のエラーにも ? が使える。
async fn boxed(ctx: CommandContext<'_>) -> Result<(), Box<dyn StdError + Send + Sync>> {
    let _count: i64 = ctx.args().parse()?;
    ctx.reply("ok").await?;
    Ok(())
}

// メッセージだけのエラー。
async fn message_only(_: CommandContext<'_>) -> Result<()> {
    Err(Error::msg("使い方: !echo 文章"))
}

async fn slash_io(_: &mut SlashContext<'_>) -> Result<(), io::Error> {
    Ok(())
}

async fn hybrid_parse(ctx: &mut HybridContext<'_>) -> Result<(), ParseIntError> {
    let _count: i64 = ctx.args().unwrap_or("0").parse()?;
    Ok(())
}

async fn button_io(_: &mut ButtonContext<'_>) -> Result<(), io::Error> {
    Ok(())
}

async fn modal_boxed(_: &mut ModalContext<'_>) -> Result<(), Box<dyn StdError + Send + Sync>> {
    Ok(())
}

async fn ready_io(_: &Ready) -> Result<(), io::Error> {
    Ok(())
}

async fn message_handler(_: Context<'_>) -> Result<(), ParseIntError> {
    Ok(())
}

#[test]
fn handlers_can_return_any_error_type() -> Result<()> {
    let mut commands = Commands::new("!")?;
    commands.register("akari", akari_result)?;
    commands.register("parse", parse_error)?;
    commands.register("io", io_error)?;
    commands.register("boxed", boxed)?;
    commands.register("message", message_only)?;
    // クロージャは、戻り値の型を書けば、任意のエラー型を返せる。
    commands.register(
        "closure",
        async move |_: CommandContext<'_>| -> Result<(), io::Error> { Ok(()) },
    )?;

    let mut slash = SlashCommands::for_guild(Id::new(1));
    slash.register("slash", "説明", vec![], slash_io)?;
    register_hybrid(
        &mut commands,
        Some(&mut slash),
        "both",
        "説明",
        vec![],
        hybrid_parse,
    )?;

    let mut events = Events::new();
    events.on_ready(ready_io);
    events.on_button("akari:test:button", button_io)?;
    events.on_modal("akari:test:modal", modal_boxed)?;

    // Bot::run のメッセージ用の関数も、任意のエラー型を返せる。
    let bot = Bot::new("token")?.with_events(events);
    drop(bot.run(message_handler));
    Ok(())
}

#[test]
fn duplicate_names_are_still_rejected_for_any_error_type() -> Result<()> {
    let mut commands = Commands::new("!")?;
    commands.register("same", parse_error)?;
    assert!(matches!(
        commands.register("same", io_error),
        Err(Error::Registration(_))
    ));
    Ok(())
}
