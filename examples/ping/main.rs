// use は、クレート（Rust のライブラリ）が公開している名前を、このファイルで使えるようにします。
use disnana_akari::model::gateway::Intents;
use disnana_akari::{InfoEvent, Result};

// mod は、別ファイルに分けたコードをこのプログラムの一部として読み込みます。
mod commands;
mod components;
mod config;
mod entities;
mod events;
mod ui;

// この属性は、Discord とやりとりする非同期処理を Tokio 上で実行できるようにします。
// async fn は「途中で通信の完了などを待てる関数」です。
// Result<()> は「成功時は値を返さず、失敗時はエラーを返す」を表します。
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    // let は変数を作ります。ここでは Settings が持つ値を同じ名前の変数として取り出します。
    // ? は成功した値を取り出し、エラーならこの関数をそこで終了して呼び出し元に返します。
    let config::Settings {
        bot,
        guild_id,
        replace_guild_commands,
    } = config::load()?;
    let (text_commands, slash_commands) = commands::register(guild_id)?;

    // Akari は Intent を自動では追加しません。テキストコマンドは、メッセージの本文を読むので、
    // GUILD_MESSAGES と MESSAGE_CONTENT が必要です（MESSAGE_CONTENT は Developer Portal でも有効にします）。
    let bot = bot.add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT);
    // on_error は、ハンドラーのエラーや Gateway のエラーを一件ずつ受け取ります。
    // 設定しない場合、Akari は同じ内容を標準エラー出力に書きます。
    let bot = bot.on_error(|report| eprintln!("エラー: {report}"));
    // on_info は指定したときだけ動きます。ここでは初回応答の所要時間を表示します。
    let bot = bot.on_info(|event| {
        if let InfoEvent::InteractionResponded {
            elapsed,
            rest_elapsed,
            ..
        } = event
        {
            eprintln!("初回応答 {elapsed:?}（REST {rest_elapsed:?}）");
        }
    });
    // with_events はイベント登録を持たせた Bot を返します。
    let bot = bot.with_events(events::register()?);
    // Some は値があること、None は値がないことを表します。
    // if let は「この形に当てはまるときだけ」と書く方法です。
    let bot = if let Some(slash_commands) = slash_commands {
        // true の場合だけ、この Bot の指定サーバー向けコマンドをコードの一覧で置き換えます。
        let slash_commands = if replace_guild_commands {
            slash_commands.replace_commands()
        } else {
            slash_commands
        };
        bot.with_slash_commands(slash_commands)
    } else {
        bot
    };

    // .await は非同期処理が終わるまで待ちます。最後の行のセミコロンを省くと、
    // その行の値（ここでは Bot の実行結果）が main の戻り値になります。
    bot.run_commands(text_commands).await
}
