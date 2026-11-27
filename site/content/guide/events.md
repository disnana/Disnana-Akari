# イベントを受け取る

接続が完了したときや、メッセージが届いたときに、関数を呼ぶ仕組みです。discord.py の `on_ready` や `on_message` にあたります。

## 基本

```rust
use disnana_akari::model::channel::Message;
use disnana_akari::model::gateway::Intents;
use disnana_akari::{Bot, Events, Ready, Result};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut events = Events::new();
    events.on_ready(on_ready);
    events.on_message(on_message);

    Bot::from_env()?
        .add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT) // on_message に必要
        .with_events(events)
        .start()
        .await
}

async fn on_ready(ready: &Ready) -> Result<()> {
    println!("{} としてログインしました", ready.user.name);
    Ok(())
}

async fn on_message(message: &Message) -> Result<()> {
    println!("{}: {}", message.author.name, message.content);
    Ok(())
}
```

| メソッド | 呼ばれるとき |
| --- | --- |
| `events.on_ready(関数)` | Discord との新しいセッションの準備ができたとき |
| `events.on_message(関数)` | 人がサーバーに送った新しいメッセージ（Bot と Webhook は除く） |
| `events.on_button(id, 関数)` | [ボタン](buttons.md)が押されたとき |
| `events.on_select(id, 関数)` ほか | [選択メニュー](select-menus.md)が操作されたとき |
| `events.on_modal(id, 関数)` | [入力フォーム](modals.md)が送信されたとき |

`on_ready` と `on_message` は、複数の関数を登録できます。登録した順に呼ばれ、1 つがエラーを返しても次へ進みます（エラーは [`Bot::on_error`](errors.md) に報告されます）。

> [!NOTE]
> `on_message` には、`GUILD_MESSAGES` と `MESSAGE_CONTENT` の Intent が必要です。Akari は自動では追加しないので、`add_intents` で指定してください（足りないときは、接続前にエラーになります）。Developer Portal の **Message Content Intent** も有効にします。[Intent の設定](intents.md)

## メッセージに返信したいとき

`on_message` の関数は `&Message` だけを受け取るので、返信はできません。返信したいときは、`Bot::run` で `Context` を受け取ります。

```rust
use disnana_akari::model::gateway::Intents;
use disnana_akari::{Bot, Context, Result};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    Bot::from_env()?
        .add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)
        .run(async |ctx: Context<'_>| -> Result<()> {
            if ctx.message().content == "こんにちは" {
                ctx.reply("こんにちは！").await?;
            }
            Ok(())
        })
        .await
}
```

テキストコマンドと併用するときは、コマンドは `run_commands` に任せ、ほかの反応は `on_message` に書きます。

## 実行の順序

`on_ready` と `on_message` は、**受信した順に 1 件ずつ** 処理されます。処理が遅いと、あとのイベントが待たされます。重い処理は、コマンドやボタンのハンドラー（並行して実行されます）に置くか、`tokio::spawn` で切り分けてください。詳しくは [Akari の動き方](how-it-works.md) を参照してください。
