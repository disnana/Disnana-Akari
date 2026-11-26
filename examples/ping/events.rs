use disnana_akari::model::channel::Message;
use disnana_akari::{Events, Ready, Result};

use crate::{components, ui};

pub fn register() -> Result<Events> {
    let mut events = Events::new();

    // コマンド名の代わりに「いつ呼ぶか」を指定して、関数を登録します。
    events.on_ready(on_ready);
    events.on_message(on_message);
    // ボタンの ID と関数を組にして登録します。押された ID の関数が呼ばれます。
    events.on_button(ui::BUTTON_ID, ui::click)?;
    events.on_button(ui::REFRESH_BUTTON_ID, ui::refresh)?;
    events.on_button(ui::SLOW_REFRESH_BUTTON_ID, ui::slow_refresh)?;
    events.on_select(components::SELECT_ID, components::choose)?;
    events.on_button(components::OPEN_MODAL_ID, components::open_modal)?;
    events.on_modal(components::MODAL_ID, components::submit)?;
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
