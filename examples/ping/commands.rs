use std::time::Duration;

use disnana_akari::model::id::{Id, marker::GuildMarker};
use disnana_akari::{
    CommandContext, Commands, HybridContext, Result, SlashCommands, SlashContext, Timing,
    register_hybrid,
};

// pub は、この関数を別モジュール（今回は main.rs）から呼べるようにします。
pub fn register(guild_id: Option<Id<GuildMarker>>) -> Result<(Commands, Option<SlashCommands>)> {
    // mut を付けると、作った変数の内容をあとから変更できます。
    let mut commands = Commands::new("!")?;

    // () なしで関数名を書くと、その関数自体を渡せます。
    // 例として、Discord に !ping が来たときに初めて ping が呼ばれます。
    commands.register("ping", ping)?;
    commands.register("echo", echo)?;

    // Option の map は、Some に入っている値にだけ関数を適用します。
    // サーバー ID が None なら、スラッシュコマンド一覧も None です。
    let mut slash_commands = guild_id.map(SlashCommands::for_guild);
    if let Some(slash) = slash_commands.as_mut() {
        // 3 番目の引数は Discord に表示する入力欄の一覧です。vec![] は空の一覧です。
        slash.register("ping", "Bot の応答を確認する", vec![], slash_ping)?;

        // true は必須入力、Some(2000) は上限を 2000 文字に設定するという意味です。
        let mut text = SlashCommands::string_option("text", "返信する文章", true);
        text.max_length = Some(2000);
        slash.register("echo", "入力した文章を返信する", vec![text], slash_echo)?;
    }

    // 一つの関数を !latency と /latency の両方に登録します。
    // as_mut() で一覧全体を渡さず、Some の中身だけを一時的に借りて渡します。
    // None のときは !latency だけが登録されます。
    register_hybrid(
        &mut commands,
        slash_commands.as_mut(),
        "latency",
        "Bot の通信と処理時間を確認する",
        vec![],
        latency,
    )?;

    // 丸括弧で二つの値をまとめ、main.rs に返します。
    Ok((commands, slash_commands))
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

async fn latency(ctx: &mut HybridContext<'_>) -> Result<()> {
    // HybridContext を使うと、どちらのコマンドからも同じ書き方で計測や返信ができます。
    // ctx.source() で呼び出し方を判別でき、ctx.author_id() はユーザーごとの連打制限に使えます。
    let response = latency_report(ctx.timing());
    // &response で文字列を一時的に借り、返信にかかった時間は測定に含めません。
    ctx.reply(&response).await
}

fn latency_report(timing: Timing) -> String {
    // Option は、まだ Gateway からハートビートの応答が来ていない場合も表せます。
    let gateway = match timing.gateway_rtt() {
        Some(duration) => milliseconds(duration),
        None => String::from("未計測"),
    };

    // format! は値を埋め込んだ文字列（String）を作ります。\n は改行です。
    format!(
        "Gateway RTT（直近のハートビート往復） {}\nキュー待ち {}\n処理時間（返信直前まで） {}",
        gateway,
        milliseconds(timing.queue_wait()),
        milliseconds(timing.handler_elapsed()),
    )
}

fn milliseconds(duration: Duration) -> String {
    // 1 秒は 1,000 ミリ秒です。{:.3} は小数点以下 3 桁で表示します。
    format!("{:.3} ms", duration.as_secs_f64() * 1_000.0)
}
