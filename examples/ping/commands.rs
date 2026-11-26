use disnana_akari::model::id::{Id, marker::GuildMarker};
use disnana_akari::{CommandContext, Commands, Result, SlashCommands, SlashContext};

// pub は、この関数を別モジュール（今回は main.rs）から呼べるようにします。
pub fn register() -> Result<Commands> {
    // mut を付けると、作った変数の内容をあとから変更できます。
    let mut commands = Commands::new("!")?;

    // () なしで関数名を書くと、その関数自体を渡せます。
    // 例として、Discord に !ping が来たときに初めて ping が呼ばれます。
    commands.register("ping", ping)?;
    commands.register("echo", echo)?;
    Ok(commands)
}

// CommandContext は、Akari がコマンドを受け取ったときに渡す情報です。
// <'_> は借りているデータの有効期間を Rust に推論させる書き方です。
async fn ping(ctx: CommandContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}

async fn echo(ctx: CommandContext<'_>) -> Result<()> {
    // !echo こんにちは の場合、args() は本文から「こんにちは」を借りて返します。
    let text = ctx.args();
    if text.is_empty() {
        // return を使うと、ここで返信した結果を返して関数を終えます。
        return ctx.reply("使い方 `!echo <文章>`").await;
    }

    ctx.reply(text).await
}

pub fn register_slash(guild_id: Id<GuildMarker>) -> Result<SlashCommands> {
    let mut commands = SlashCommands::for_guild(guild_id);

    // 3 番目の引数はDiscord に表示する入力欄の一覧です。vec![] は空の一覧です。
    commands.register("ping", "Bot の応答を確認する", vec![], slash_ping)?;

    // true は必須入力、Some(2000) は上限を 2000 文字に設定するという意味です。
    let mut text = SlashCommands::string_option("text", "返信する文章", true);
    text.max_length = Some(2000);
    commands.register("echo", "入力した文章を返信する", vec![text], slash_echo)?;
    Ok(commands)
}

// &mut は値を借りながら変更できる参照です。返信すると ctx に返信済みの印が付きます。
async fn slash_ping(ctx: &mut SlashContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}

async fn slash_echo(ctx: &mut SlashContext<'_>) -> Result<()> {
    // let ... else は、値が取れたときだけ先へ進めたいときに使います。
    let Some(text) = ctx.string("text") else {
        return ctx.reply_ephemeral("文章を入力してください。").await;
    };
    // text は SlashContext が借りている Discord のデータを参照します。
    ctx.reply(text).await
}
