use std::time::{Duration, SystemTime, UNIX_EPOCH};

use disnana_akari::{
    ButtonContext, Result, SlashContext,
    ui::{Button, Embed, UiMessage},
};

// const は変更しない値です。送信とイベント登録で同じ ID を使います。
pub const BUTTON_ID: &str = "akari:sample:ping";
pub const REFRESH_BUTTON_ID: &str = "akari:sample:refresh";
pub const SLOW_REFRESH_BUTTON_ID: &str = "akari:sample:slow-refresh";

pub async fn show(ctx: &mut SlashContext<'_>) -> Result<()> {
    ctx.reply_ui(&panel("更新待ち")).await
}

fn panel(status: &str) -> UiMessage {
    // .description(...) のように、作った値へ順番に設定を足していきます。
    // 0x から始まる数字は 16 進数で、ここではカードの色を表します。
    let embed = Embed::new("Akari Playground")
        .description("Ping は押した人にだけ返信します。更新ボタンはこのカードを書き換えます。")
        .color(0x3970C4)
        .field("状態", status, false);

    // vec![...] で、一行に表示するボタンの一覧を作ります。
    // 最初の返信と更新時に同じ関数を使うので、更新後もボタンが残ります。
    UiMessage::new().embed(embed).buttons(vec![
        Button::primary(BUTTON_ID, "Ping"),
        Button::primary(REFRESH_BUTTON_ID, "更新"),
        Button::primary(SLOW_REFRESH_BUTTON_ID, "遅延更新"),
    ])
}

pub async fn click(ctx: &mut ButtonContext<'_>) -> Result<()> {
    // /ui の返信とは別に、ボタンを押す操作にも Discord への応答が必要です。
    // ephemeral は押した本人の画面にだけ表示する、という意味です。
    ctx.reply_ephemeral("Pong!").await
}

pub async fn refresh(ctx: &mut ButtonContext<'_>) -> Result<()> {
    // ボタンを押したメッセージを、その場で書き換えます。
    let message = refreshed_panel("更新しました。")?;
    ctx.update_message(&message).await
}

pub async fn slow_refresh(ctx: &mut ButtonContext<'_>) -> Result<()> {
    // 先に Discord へ応答して、更新が後から届くことを伝えます。
    ctx.defer_update().await?;
    // .await で待っている間、ほかの非同期処理は進められます。
    tokio::time::sleep(Duration::from_secs(4)).await;

    let message = refreshed_panel("4 秒待ってから更新しました。")?;
    ctx.update_message(&message).await?;
    // 追加の返信は、ボタンを押した人にだけ表示します。
    ctx.followup_ephemeral("遅延更新が完了しました。").await
}

fn refreshed_panel(status: &str) -> Result<UiMessage> {
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    // <t:秒:T> は Discord で見る人の時刻表示に変換されます。
    Ok(panel(&format!("{status}\n更新時刻 <t:{seconds}:T>")))
}
