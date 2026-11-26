# はじめての Bot

`src/main.rs` を次の内容に置き換えます。

```rust
use disnana_akari::model::gateway::Intents;
use disnana_akari::{Bot, CommandContext, Commands, Events, Ready, Result};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    // プレフィックスが「!」のコマンド一覧を作る
    let mut commands = Commands::new("!")?;

    // 「!ping」が来たら ping 関数を呼ぶ
    commands.register("ping", ping)?;

    // 接続できたときに呼ぶ関数を登録する
    let mut events = Events::new();
    events.on_ready(on_ready);

    // 環境変数 DISCORD_TOKEN のトークンで Bot を作り、起動する
    Bot::from_env()?
        // テキストコマンドは、メッセージの本文を読むので、この Intent が必要
        .add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)
        .with_events(events)
        .run_commands(commands)
        .await
}

async fn ping(ctx: CommandContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}

async fn on_ready(ready: &Ready) -> Result<()> {
    println!("{} としてログインしました", ready.user.name);
    Ok(())
}
```

[トークンを環境変数に設定](discord-setup.md#トークンを渡す)したターミナルで実行します。

```sh
cargo run
```

次のように表示されたら、Bot が Discord につながっています。`（Bot 名）` の部分には、あなたの Bot の名前が入ります。

```text
（Bot 名） としてログインしました
```

> [!NOTE]
> この表示は、上のコードの `on_ready` が出しています。Akari 自身は、接続しても何も表示しません。接続を知りたいときは、このように `on_ready` に書きます。

Bot を招待したサーバーのチャンネルで `!ping` と送ってみましょう。Bot が元のメッセージに `Pong!` と返信します。止めるときは `Ctrl+C` を押します。

## コードを読む

<!-- no-check -->
```rust
let mut commands = Commands::new("!")?;
```

`Commands` は、テキストコマンドの登録先です。引数はプレフィックスで、空文字列や空白を含む文字列はエラーになります。

<!-- no-check -->
```rust
commands.register("ping", ping)?;
```

プレフィックスを除いた名前と、呼び出す関数を結び付けます。`ping` に `()` を付けていないのは、関数を **呼ぶ** のではなく、関数そのものを **渡して** いるからです。同じ名前を 2 回登録するとエラーになります。

<!-- no-check -->
```rust
Bot::from_env()?
    .add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)
    .with_events(events)
    .run_commands(commands)
    .await
```

`Bot::from_env` は `DISCORD_TOKEN` を読んで Bot を作ります。`add_intents` は、Bot が Discord から受け取りたいイベントを指定します。テキストコマンドには、この 2 つの Intent が必要です（[Intent の設定](../guide/intents.md)）。`run_commands` が Discord に接続し、メッセージを待ち続けます。接続が切れて復旧できなくなったときだけ、エラーを返して終了します。

<!-- no-check -->
```rust
events.on_ready(on_ready);
```

接続できたときに呼ばれる関数を登録します。discord.py の `on_ready` と同じです。

<!-- no-check -->
```rust
async fn ping(ctx: CommandContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}
```

コマンドの本体です。`CommandContext` に、呼び出されたメッセージの情報が入っています。`ctx.reply` は、そのメッセージへの返信を送ります。通知（メンション）は飛ばないようになっています。

> [!TIP]
> `Result<()>` や `?`、`'_` が分からなくても大丈夫です。[Rust が初めての人へ](rust-basics.md) で、Akari を使うのに必要な分だけ説明しています。

## うまくいかないときは

| 症状 | 確認すること |
| --- | --- |
| `DISCORD_TOKEN に Discord Bot トークンを設定してください` | `cargo run` と同じターミナルでトークンを設定したか |
| 起動時に「Intent … が必要です」と出る | `add_intents` で `GUILD_MESSAGES` と `MESSAGE_CONTENT` を追加したか |
| `!ping` に反応しない | Message Content Intent を Developer Portal で有効にしたか、Bot にチャンネルが見えているか |
| 反応するが返信が出ない | Bot に **Send Messages** と **Read Message History** の権限があるか |

詳しくは [よくある質問](../reference/faq.md) を見てください。

## 次のステップ

- コマンドに引数を渡したい → [テキストコマンド](../guide/text-commands.md)
- `/` から呼べるようにしたい → [スラッシュコマンド](../guide/slash-commands.md)
