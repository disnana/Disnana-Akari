# テキストコマンド

`!ping` のように、メッセージの先頭にプレフィックスを付けて呼び出すコマンドです。

> [!NOTE]
> テキストコマンドは、メッセージの本文を読む必要があります。Developer Portal で **Message Content Intent** を有効にし、コードでは `GUILD_MESSAGES` と `MESSAGE_CONTENT` を `add_intents` で指定してください（[手順](../getting-started/discord-setup.md#_3-message-content-intent-を有効にする)、[Intent の設定](intents.md)）。

## 基本

```rust
use disnana_akari::model::gateway::Intents;
use disnana_akari::{Bot, CommandContext, Commands, Result};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut commands = Commands::new("!")?;
    commands.register("ping", ping)?;
    commands.register("hello", hello)?;
    Bot::from_env()?
        .add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)
        .run_commands(commands)
        .await
}

async fn ping(ctx: CommandContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}

async fn hello(ctx: CommandContext<'_>) -> Result<()> {
    let user = &ctx.context().message().author;
    ctx.reply(&format!("こんにちは、{} さん！", user.name)).await
}
```

- `Commands::new("!")` でプレフィックスを決めます。空文字列と、空白を含む文字列はエラーです。
- `register("名前", 関数)` でコマンドを登録します。名前にプレフィックスは含めません。
- コマンド関数は `async fn(CommandContext<'_>) -> Result<()>` の形です。

## 引数を受け取る

コマンド名のあとの文章は、`ctx.args()` で丸ごと受け取れます。前後の空白は取り除かれ、引数がなければ空文字列です。

```rust
use disnana_akari::{CommandContext, Result};

async fn echo(ctx: CommandContext<'_>) -> Result<()> {
    let text = ctx.args();
    if text.is_empty() {
        return ctx.reply("使い方: `!echo 文章`").await;
    }
    ctx.reply(text).await
}
```

`!echo こんにちは 世界` なら、`ctx.args()` は `こんにちは 世界` です。複数の値に分けたいときは、自分で分割します。

```rust
use disnana_akari::{CommandContext, Result};

async fn add(ctx: CommandContext<'_>) -> Result<()> {
    let mut numbers = ctx.args().split_whitespace().map(str::parse::<i64>);
    let (Some(Ok(a)), Some(Ok(b))) = (numbers.next(), numbers.next()) else {
        return ctx.reply("使い方: `!add 1 2`").await;
    };
    ctx.reply(&format!("{a} + {b} = {}", a + b)).await
}
```

## 誰が呼んだか調べる

`ctx.context().message()` で、元のメッセージ（Twilight の `Message`）を借りられます。投稿者、チャンネル、サーバーなどを調べられます。

```rust
use disnana_akari::{CommandContext, Result};

async fn whoami(ctx: CommandContext<'_>) -> Result<()> {
    let message = ctx.context().message();
    let text = format!("あなたの ID は `{}` です。", message.author.id);
    ctx.reply(&text).await
}
```

## コマンドの間で状態を共有する

`register` には、名前のない関数（クロージャ）も渡せます。`async move` で値を取り込めば、呼び出しのたびに使えます。複数の呼び出しで共有する値は `Arc` で包みます。

```rust
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};

use disnana_akari::{CommandContext, Commands, Result};

fn register(commands: &mut Commands) -> Result<()> {
    let counter = Arc::new(AtomicU64::new(0));
    commands.register("count", async move |ctx: CommandContext<'_>| {
        let n = counter.fetch_add(1, Ordering::Relaxed) + 1;
        ctx.reply(&format!("{n} 回目の呼び出しです。")).await
    })
}
```

## 返信の種類

| メソッド | 動作 |
| --- | --- |
| `ctx.reply(text)` | テキストで返信します。返信先への通知やメンションは飛びません。 |
| `ctx.reply_with(text, &mentions)` | 通知する相手を指定して返信します。`mentions` は `disnana_akari::model::channel::message::AllowedMentions` です。 |
| `ctx.reply_ui(&message)` | [Embed やボタン](buttons.md)付きで返信します。 |
| `ctx.reply_v2(&message)` | [Components V2](components-v2.md) で返信します。 |

## 呼び出しのルール

- コマンド名は **大文字と小文字を区別** します。`!PING` は `!ping` とは別物です。
- 最初の空白までがコマンド名です。`! ping`（プレフィックスの直後が空白）は無視されます。
- 未登録のコマンドや、普通の投稿には反応しません。
- Bot と Webhook の投稿は、既定では無視されます。`Bot::include_bots(true)` で渡せます（自分自身の投稿は、常に渡しません）。

## エラーが起きたとき

コマンド関数が `Err` を返すと、Akari はエラーを `Bot::on_error` に報告し、`tracing` のログにも出します（[ログを出す](logging.md)）。元のメッセージへの返信は、既定では **しません**。返信したいときは、`Messages::text_command_failed` に文言を設定します（[エラーと自動で送る文言](errors.md)）。利用者に見せたい説明は、`Err` を返さず、自分で返信するのが確実です。

## 次のステップ

- `/` から呼べるようにする → [スラッシュコマンド](slash-commands.md)
- 両方に対応する → [ハイブリッドコマンド](hybrid-commands.md)
- 連打を防ぐ → [クールダウン](cooldowns.md)
