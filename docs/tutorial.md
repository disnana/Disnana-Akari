# コマンドを関数ごとに登録する

[クイックスタート](quickstart.md) で用意した Playground では、`src/main.rs` が Bot を起動し、`src/commands.rs` がコマンドを登録します。

```rust
use disnana_akari::{CommandContext, Commands, Result};

pub fn register() -> Result<Commands> {
    let mut commands = Commands::new("!")?;
    commands.register("ping", ping)?;
    commands.register("echo", echo)?;
    Ok(commands)
}

async fn ping(ctx: CommandContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}

async fn echo(ctx: CommandContext<'_>) -> Result<()> {
    let text = ctx.args();
    if text.is_empty() {
        return ctx.reply("使い方 `!echo <文章>`").await;
    }

    ctx.reply(text).await
}
```

`main.rs` で `mod commands;` と宣言し、`bot.run_commands(commands::register()?).await` で登録したコマンドを有効にします。関数を増やすときは `src/commands.rs` に `async fn` を追加し、`register` に名前と関数を渡します。名前は `ping` のようにプレフィックスを省いて指定します。

`!echo こんにちは` を送ると `ctx.args()` は `こんにちは` を返します。引数の前後の空白は取り除かれ、途中の空白は保持されます。引数は元のメッセージ本文の一部を借りた `&str` なので、そのまま `reply` に渡せます。ハンドラーの終了後にも本文を保持したい場合は `to_owned()` で `String` を作ります。

コマンドが増えたら `commands` の下に用途別のサブモジュールを作り、各モジュールから同じ `Commands` に登録できます。未登録のコマンドや通常の投稿を扱う処理は、`Bot::run` のメッセージハンドラーに書けます。[API リファレンス](api.md)
