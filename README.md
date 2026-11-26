# Disnana Akari

Twilight を土台にした Rust 製 Discord Bot フレームワークです。テキストコマンド、サーバー向けのスラッシュコマンド、Gateway イベントをそれぞれ `async fn` として登録できます。現時点では `publish = false` を設定しており、ローカルのパス依存として使えます。

```rust
use disnana_akari::{Bot, CommandContext, Commands, Result};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut commands = Commands::new("!")?;
    commands.register("ping", ping)?;
    Bot::from_env()?.run_commands(commands).await
}

async fn ping(ctx: CommandContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}
```

- [日本語ドキュメント](docs/README.md)
- [クイックスタートとローカルの実験用 Bot](docs/quickstart.md)
- [Playground のコードを読む](docs/tutorial.md)
- [API リファレンス](docs/api.md)
