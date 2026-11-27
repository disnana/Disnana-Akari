# Disnana Akari

Twilight を土台にした Rust 製 Discord Bot フレームワークです。テキストコマンド、サーバー向けのスラッシュコマンド、Gateway イベントをそれぞれ `async fn` として登録できます。テキストとスラッシュで処理を共通化したい場合は、同じ関数をハイブリッドコマンドとして登録できます。Discord の返信には埋め込みや Components V2 を使え、ボタンや選択メニューから入力フォームも開けます。スラッシュコマンドは、サブコマンド、オートコンプリート、型付きの引数、グローバル登録に対応し、権限の確認は `ctx.require_permissions(..)?` と明示的に呼びます。関連する登録は `Module` にまとめられます。

Akari は、頼まれていないことを、勝手にしません。Intent を自動で追加せず、標準出力にも書かず、エラーは `Bot::on_error` で受け取れます。ログは Rust の標準の `tracing` で出すので、出力先とレベルはアプリが決められます。

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
- 0.1 から更新する場合は、変更履歴の「互換性のない変更」と、ドキュメントサイトの「0.1 から 0.2 への移行」を確認してください。
- [公開前の確認](docs/releasing.md)
- [セキュリティ報告](SECURITY.md)

`Result<T>` が返す `Error` は、Intent の不足、Discord REST API の失敗、アプリ固有のエラーなどを型で判別できます。実行中のエラーは `Bot::on_error` でも受け取れます。[エラーの扱い方](docs/api.md#エラーと自動で送る文言)

Gateway の接続や Interaction の初回応答にかかった時間は、必要に応じて `Bot::on_info` で受け取れます。[実行時の情報](docs/api.md#実行時の情報)

## ライセンス

Disnana Akari は [MIT ライセンス](LICENSE)で提供します。
