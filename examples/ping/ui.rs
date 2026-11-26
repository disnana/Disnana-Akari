use disnana_akari::{
    ButtonContext, Result, SlashContext,
    ui::{Button, Embed, UiMessage},
};

// const は変更しない値です。送信とイベント登録で同じ ID を使います。
pub const BUTTON_ID: &str = "akari:sample:ping";

pub async fn show(ctx: &mut SlashContext<'_>) -> Result<()> {
    // .description(...) のように、作った値へ順番に設定を足していきます。
    // 0x から始まる数字は 16 進数で、ここではカードの色を表します。
    let embed = Embed::new("Akari Playground")
        .description("ボタンを押すと、押した人にだけ返信が届きます。")
        .color(0x3970C4);

    // vec![...] は一覧を作ります。ここでは一行にボタンを一つだけ載せます。
    let message = UiMessage::new()
        .embed(embed)
        .buttons(vec![Button::primary(BUTTON_ID, "Ping")]);
    ctx.reply_ui(&message).await
}

pub async fn click(ctx: &mut ButtonContext<'_>) -> Result<()> {
    // /ui の返信とは別に、ボタンを押す操作にも Discord への応答が必要です。
    // ephemeral は押した本人の画面にだけ表示する、という意味です。
    ctx.reply_ephemeral("Pong!").await
}
