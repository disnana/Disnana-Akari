# クイックスタート

## Discord に Bot を用意する

1. Rust 1.89 以上を用意します。
2. Discord Developer Portal でアプリを作成します。**Bot** タブで **Message Content Intent** を有効にし、Bot トークンを取得します。
3. アプリを `bot` スコープでテスト用サーバーにインストールします。試すチャンネルで **View Channel**、**Send Messages**、**Read Message History** を許可します。返信には元メッセージを読む権限が必要です。

[Discord Developer Portal](https://discord.com/developers/applications) · [Discord の Message Content Intent](https://docs.discord.com/developers/events/gateway#message-content-intent) · [Discord のメッセージ作成 API](https://docs.discord.com/developers/resources/message#create-message)

## Playground を起動する

`playground/.env` を開き、`DISCORD_TOKEN=` の右辺に Bot トークンを貼り付けます。UTF-8 で保存し、値は引用符で囲まずに書きます。このファイルは `.gitignore` の除外対象です。

```dotenv
DISCORD_TOKEN=
```

RustRover でリポジトリのルートを開き、上部の実行構成で **Akari Playground** を選んで ▶ を押します。キーボードでは `Shift+F10` で起動できます。[RustRover の実行構成](https://www.jetbrains.com/help/rust/run-debug-configuration.html)

補完を有効にするには、RustRover の Project ツリーで `playground/Cargo.toml` を右クリックし、**Attach Cargo Project** を選びます。Cargo ツールウィンドウに `akari-playground` が表示されれば、`playground/src/main.rs` がバイナリクレートのルートとして認識されます。[Cargo プロジェクトの追加](https://www.jetbrains.com/help/rust/rust-attach-cargo-projects.html)

ターミナルを使う場合は、リポジトリのルートから起動します。

```sh
cargo run --locked --manifest-path playground/Cargo.toml --target-dir target
```

`Discord に接続しました` と表示されたら、テスト用サーバーで `!ping` と `!echo こんにちは` を送ります。それぞれ `Pong!` と `こんにちは` が返ります。RustRover の停止ボタンか `Ctrl+C` で終了します。

起動時に Bot の Cargo プロジェクト直下の `.env` を優先して読みます。ファイルがない場合は、シェルや実行構成の `DISCORD_TOKEN` を読みます。

## リポジトリ同梱のサンプルを実行する

リポジトリのルートで、PowerShell の場合は次のように実行します。トークンを環境変数に設定し、サンプルを起動します。

```powershell
$env:DISCORD_TOKEN = Read-Host 'Discord Bot token' -MaskInput
cargo run --locked --example ping
```

`-MaskInput` は PowerShell 7.2 以上で使えます。Bash の場合は次のコマンドを使います。

```bash
read -rs -p 'Discord Bot token: ' DISCORD_TOKEN; printf '\n'
export DISCORD_TOKEN
cargo run --locked --example ping
```

`Discord に接続しました` と表示されたら、サーバーのチャンネルで `!ping` を送ります。Bot が元のメッセージに `Pong!` と返信します。終了は `Ctrl+C` です。

## Git 管理外で自分のコードを書く

`playground/` は `.gitignore` に登録済みです。Akari を `path = ".."` で参照する独立した Cargo プロジェクトなので、Akari 本体の変更もそのまま試せます。

作成済みなら、`playground/src/commands.rs` でコマンドを編集して **Akari Playground** を実行します。

新しくクローンした環境では、ルートで次のコマンドを実行してフォルダを作ります。既存の `playground/` がある場合はこの手順を飛ばしてください。

```powershell
cargo new --bin --vcs none --name akari-playground playground
cargo add --manifest-path playground/Cargo.toml --path .
cargo add --manifest-path playground/Cargo.toml tokio --features macros,rt
Copy-Item examples/ping/*.rs playground/src/
New-Item -ItemType File -Path playground/.env
```

`playground/.env` に `DISCORD_TOKEN=` の行を追加してトークンを設定します。

実験用 Bot を止めてトークンを使い終えたら、PowerShell では `Remove-Item Env:DISCORD_TOKEN`、Bash では `unset DISCORD_TOKEN` で現在のシェルから消去できます。
