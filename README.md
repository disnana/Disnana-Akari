# Disnana Akari

Twilight を土台にした、Rust 製の Discord Bot フレームワークです。discord.py を参考に、Rust らしく、多機能で自由度が高く、シンプルで分かりやすい形を目指しています。

## できること

- **コマンド**: テキストコマンド、スラッシュコマンド（サブコマンド、オートコンプリート、型付きの引数、グローバル登録）、両方に同じ関数を登録できるハイブリッドコマンド。
- **イベント**: Gateway イベントごとの関数の登録。関連する登録は `Module` にまとめられます。
- **返信**: Embed、ボタン、選択メニュー、入力フォーム、Components V2。
- **権限と管理操作**: `ctx.require_permissions(..)?` で実行できる人を絞り、`Needs` で必要な権限を登録時に宣言できます。Bot の権限とロールの階層を確認し、チャンネル、ロール、メンバー、権限の上書きを、数値やビットを書かずに管理できます。
- **クールダウンと同時実行の制限**: キーごとに、回数や同時に実行できる件数を決められます。
- **エラーとログ**: エラーは型で判別でき、`Bot::on_error` と `Bot::on_info` で受け取れます。ログは Rust の標準の `tracing` で出すので、出力先とレベルはアプリが決められます。

## 方針

Akari は、頼まれていないことを、勝手にしません。Intent を自動で追加せず、標準出力にも標準エラー出力にも書かず、権限の確認も、呼んだときだけ行います。依存は、安定していて保守が続いているものだけに絞っています。

0.x では公開 API に互換性のない変更が入る場合があります。更新時は[変更履歴](CHANGELOG.md)を確認してください。0.1 から更新する場合は、変更履歴の「互換性のない変更」と、ドキュメントサイトの「0.1 から 0.2 への移行」を見てください。

## はじめかた

```sh
cargo add disnana-akari
cargo add tokio --features macros,rt
```

Bot トークンを環境変数 `DISCORD_TOKEN` に設定して、次のコードを実行します（Discord 側の準備は[クイックスタート](docs/quickstart.md)を見てください）。

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

スラッシュコマンドだけを使う場合は、`Bot::from_env()?.with_slash_commands(slash).start().await` で起動できます。`slash` は、テスト用サーバーの ID とコマンドを指定した `SlashCommands` です（Intent の指定は要りません）。

## ドキュメント

- [ドキュメントサイト](https://disnana.github.io/Disnana-Akari/) — はじめかた、ガイド、discord.py との対応表
- [API リファレンス（docs.rs）](https://docs.rs/disnana-akari) · [日本語の API 一覧](docs/api.md)
- [クイックスタートとローカルの実験用 Bot](docs/quickstart.md) · [Playground のコードを読む](docs/tutorial.md)
- [トラブルシューティング](docs/troubleshooting.md)
- [変更履歴](CHANGELOG.md) · [設計の規則](DESIGN.md) · [公開前の確認](docs/releasing.md) · [セキュリティ報告](SECURITY.md)

## ライセンス

Disnana Akari は [MIT ライセンス](LICENSE)で提供します。
