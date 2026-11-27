# Disnana Akari

Twilight を土台にした Rust 製 Discord Bot フレームワークです。テキストコマンド、サーバー向けのスラッシュコマンド、Gateway イベントをそれぞれ `async fn` として登録できます。テキストとスラッシュで処理を共通化したい場合は、同じ関数をハイブリッドコマンドとして登録できます。Discord の返信には埋め込みや Components V2 を使え、ボタンや選択メニューから入力フォームも開けます。

Akari は、頼まれていないことを、勝手にしません。Intent を自動で追加せず、標準出力にも書かず、エラーは `Bot::on_error` で受け取れます。

0.x では公開 API に互換性のない変更が入る場合があります。更新時は変更履歴を確認してください。[変更履歴](CHANGELOG.md) · [最新リリース](https://github.com/disnana/Disnana-Akari/releases/latest)

```rust
use disnana_akari::model::gateway::Intents;
use disnana_akari::{Bot, CommandContext, Commands, Result};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut commands = Commands::new("!")?;
    commands.register("ping", ping)?;
    Bot::from_env()?
        // テキストコマンドは、メッセージの本文を読むので、この Intent が必要です。
        .add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)
        .run_commands(commands)
        .await
}

async fn ping(ctx: CommandContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}
```

スラッシュコマンドだけを使う場合は、`Bot::from_env()?.with_slash_commands(slash).start().await` で起動できます。`slash` はテスト用サーバー ID とコマンドを指定した `SlashCommands` です。[API リファレンス](docs/api.md)

- [日本語ドキュメント](docs/README.md)
- [クイックスタートとローカルの実験用 Bot](docs/quickstart.md)
- [Playground のコードを読む](docs/tutorial.md)
- [API リファレンス](docs/api.md)
- [変更履歴](CHANGELOG.md)
- [公開前の確認](docs/releasing.md)
- [セキュリティ報告](SECURITY.md)

`Result<T>` が返す `Error` は、Intent の不足、Discord REST API の失敗、アプリ固有のエラーなどを型で判別できます。実行中のエラーは `Bot::on_error` でも受け取れます。[エラーの扱い方](docs/api.md#エラーと自動で送る文言)

## ライセンス

Disnana Akari は [MIT ライセンス](LICENSE)で提供します。
