use disnana_akari::model::channel::Message;
use disnana_akari::{Events, Ready, Result};

use crate::ui;

pub fn register() -> Result<Events> {
    let mut events = Events::new();

    // コマンド名の代わりに「いつ呼ぶか」を指定して、関数を登録します。
    events.on_ready(on_ready);
    events.on_message(on_message);
    // ボタンの ID が一致したときにだけ、ui.rs の click 関数を呼びます。
    events.on_button(ui::BUTTON_ID, ui::click)?;
    Ok(events)
}

// &Ready は、接続完了イベントの情報を借りて読むための型です。
async fn on_ready(ready: &Ready) -> Result<()> {
    println!("{} のイベントハンドラーが準備できました。", ready.user.name);
    Ok(())
}

async fn on_message(message: &Message) -> Result<()> {
    // 新着メッセージのイベントには、どのサーバーから届いたかの情報があります。
    // サンプルでは読み取った本文や送信者の名前を表示せず、コマンドの候補だけ数えられるようにします。
    if message.content.starts_with('!') {
        println!("テキストコマンド候補を受信しました。");
    }
    Ok(())
}
