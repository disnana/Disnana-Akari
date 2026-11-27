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

## コマンドに別名を付ける

```rust
use disnana_akari::{CommandContext, Commands, Result};

fn register(commands: &mut Commands) -> Result<()> {
    commands.register_with_aliases("echo", &["e", "repeat"], echo)
}

async fn echo(ctx: CommandContext<'_>) -> Result<()> {
    ctx.reply(&format!("{}: {}", ctx.invoked_with(), ctx.args())).await
}
```

`!e こんにちは` の場合、`ctx.command_name()` は `"echo"`、`ctx.invoked_with()` は `"e"` です。別名は大文字と小文字を区別します。正式名やほかの別名と重複した場合、そのコマンドの登録全体が失敗します。

`Commands::with_prefixes(&["!", "akari."])?` を使うと、`!echo` と `akari.echo` のどちらでも同じコマンドを実行できます。プレフィックスが重なる場合は、`!` より長い `!!` のように、最長の一致を選びます。使われたプレフィックスは `ctx.prefix()` で確認できます。

## 投稿ごとにプレフィックスを変える

サーバーごとの設定を読む場合は、`Commands::with_prefix_resolver` に非同期関数を渡します。例では DM で `?`、サーバーで `!` と `akari.` を使います。

```rust
use disnana_akari::{CommandPrefixes, Commands, Result, model::channel::Message};

fn commands() -> Commands {
    Commands::with_prefix_resolver(async |message: &Message| -> Result<CommandPrefixes> {
        if message.guild_id.is_none() {
            CommandPrefixes::new(&["?"])
        } else {
            CommandPrefixes::new(&["!", "akari."])
        }
    })
}
```

`CommandPrefixes::default()` を返すと、その投稿でのコマンド判定を省きます。候補が複数一致すれば最長のものを使います。同じ候補を繰り返し使う場合は、あらかじめ `CommandPrefixes` を用意し、`clone()` で共有できます。判定のワーカーはコマンド実行用と独立しており、受け付けた投稿ごとに一度だけ関数を呼びます。判定のキューが満杯の場合は投稿への混雑返信を省きます。判定関数のエラーは `Bot::on_error` で `ErrorSource::PrefixResolver` として受け取れます。

## サブコマンドにまとめる

```rust
use disnana_akari::{CommandContext, Commands, Result};

fn register(commands: &mut Commands) -> Result<()> {
    let mut settings = commands.group_with_aliases("settings", &["config"])?;
    settings.subcommand_with_aliases("show", &["get"], show)?;
    settings.fallback(async |ctx: CommandContext<'_>| -> Result<()> {
        ctx.reply("show を指定すると設定を確認できます。").await
    })
}

async fn show(ctx: CommandContext<'_>) -> Result<()> {
    ctx.reply(&format!("実行したコマンド: {}", ctx.qualified_name())).await
}
```

`!config get` を実行しても、`qualified_name()` は正式な経路の `"settings show"` を返します。`command_name()` は末尾の正式名 `"show"`、`invoked_with()` は入力された末尾の名前 `"get"` です。`!settings` のようにグループ名だけ送ると、登録した `fallback` が実行されます。未知のサブコマンドは一致しません。子グループは `settings.group(name)` で作れます。

## ヘルプを表示する

`register_help("help")` でヘルプコマンドを追加できます。次の例では、`!help` に一覧、`!help echo` に使い方を表示します。

```rust
use disnana_akari::{CommandContext, CommandMetadata, Commands, Result};

async fn echo(ctx: CommandContext<'_>) -> Result<()> {
    ctx.reply(ctx.args()).await
}

fn with_help() -> Result<Commands> {
    let mut commands = Commands::new("!")?;
    commands.register("echo", echo)?;
    commands.set_metadata(
        "echo",
        CommandMetadata::new()
            .description("入力された文章を返します。")
            .usage("<文章>"),
    )?;
    commands.register_help("help")?;
    Ok(commands)
}
```

グループ内のコマンドは `!help config set` のように指定します。エイリアスからも同じヘルプを開けます。`CommandMetadata::hidden(true)` はヘルプの表示を制御します。実行者を制限するときは、コマンドのチェックを設定します。

独自のヘルプを作る場合は `command_infos()` で一覧を、`command_info(path)` で指定したコマンドのメタデータを参照できます。標準のヘルプは、一覧に出すコマンドの権限チェックを代行しません。

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

### 空白を含む値を読む

例えば `!search 「Rust の本」 3` から検索語と件数を取り出せます。`arguments()` は必要な分だけ読み進め、元の本文は引き続き `args()` から参照できます。

```rust
use disnana_akari::{CommandContext, Error, Result};

async fn search(ctx: CommandContext<'_>) -> Result<()> {
    let mut args = ctx.arguments();
    let Some(query) = args.next().transpose().map_err(Error::handler)? else {
        return ctx.reply("検索語を指定してください。").await;
    };
    let count = match args.next().transpose().map_err(Error::handler)? {
        Some(value) => value.parse::<u32>().map_err(Error::handler)?,
        None => 1,
    };
    ctx.reply(&format!("{query} を {count} 件検索します。")).await
}
```

半角の一重・二重引用符、スマートクォート、`「」`、`『』` で囲めます。クォートの閉じ忘れはエラーになります。

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
