# クイックスタート

## Discord に Bot を用意する

1. Rust 1.89 以上を用意します。
2. Discord Developer Portal でアプリを作成します。**Bot** タブで **Message Content Intent** を有効にし、Bot トークンを取得します。
3. アプリを `bot` スコープでテスト用サーバーにインストールします。このスコープには `applications.commands` が含まれます。試すチャンネルで **View Channel**、**Send Messages**、**Read Message History** を許可します。返信には元メッセージを読む権限が必要です。

[Discord Developer Portal](https://discord.com/developers/applications) · [Discord の Message Content Intent](https://docs.discord.com/developers/events/gateway#message-content-intent) · [Discord のメッセージ作成 API](https://docs.discord.com/developers/resources/message#create-message) · [アプリのスコープとコマンド登録](https://docs.discord.com/developers/interactions/application-commands)

## Playground を起動する

`playground/` がまだない場合は、後述の手順で作成します。`playground/.env` を開き、`DISCORD_TOKEN=` の右辺に Bot トークンを貼り付けます。`/ping` と `/echo` も試す場合は、Discord の設定で開発者モードを有効にし、テスト用サーバーを右クリックして **サーバーIDをコピー** を選びます。コピーした数字を `DISCORD_GUILD_ID=` の右辺に貼り付けます。UTF-8 で保存し、値は引用符を付けずに書きます。`playground/` は Git の除外対象です。

```dotenv
DISCORD_TOKEN=
DISCORD_GUILD_ID=
```

RustRover でリポジトリのルートを開き、上部の実行構成で **Akari Playground** を選んで ▶ を押します。キーボードでは `Shift+F10` で起動できます。[RustRover の実行構成](https://www.jetbrains.com/help/rust/run-debug-configuration.html)

補完を有効にするには、RustRover の Project ツリーで `playground/Cargo.toml` を右クリックし、**Attach Cargo Project** を選びます。Cargo ツールウィンドウに `akari-playground` が表示されれば、`playground/src/main.rs` がバイナリクレートのルートとして認識されます。[Cargo プロジェクトの追加](https://www.jetbrains.com/help/rust/rust-attach-cargo-projects.html)

ターミナルを使う場合は、リポジトリのルートから起動します。

```sh
cargo run --locked --manifest-path playground/Cargo.toml --target-dir target
```

`Discord に接続しました` と表示されたら、テスト用サーバーのチャンネルで確認します。

| Discord での操作 | 返信 |
| --- | --- |
| メッセージとして `!ping` を送る | `Pong!` |
| メッセージとして `!echo こんにちは` を送る | `こんにちは` |
| 自分でインストールした Bot の `/ping` を選んで実行する | `Pong!` |
| 同じ Bot の `/echo` を選び、`text` 欄に `こんにちは` と入力して実行する | `こんにちは` |

スラッシュコマンドは `DISCORD_GUILD_ID` で指定したサーバーで使えます。ID を省略するとテキストコマンドとイベント処理で起動します。Bot を止めるには、RustRover の停止ボタンか `Ctrl+C` を使います。

起動時に Bot の Cargo プロジェクト直下の `.env` を優先して読みます。ファイルがない場合は、シェルや実行構成の環境変数を使います。

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

作成済みなら、`playground/src/commands.rs` でコマンド、`playground/src/events.rs` でイベントを編集して **Akari Playground** を実行します。自作のコードがある場合は、`examples/ping/` と `playground/src/` の差分を見ながら新しいサンプルを取り込めます。

新しくクローンした環境では、ルートで次のコマンドを実行してフォルダを作ります。既存の `playground/` がある場合はこの手順を飛ばしてください。

```powershell
cargo new --bin --vcs none --name akari-playground playground
cargo add --manifest-path playground/Cargo.toml --path .
cargo add --manifest-path playground/Cargo.toml tokio --features macros,rt
Copy-Item examples/ping/*.rs playground/src/
New-Item -ItemType File -Path playground/.env
cargo generate-lockfile --manifest-path playground/Cargo.toml
```

`playground/.env` にトークンを入れ、必要に応じてテスト用サーバー ID も設定します。[Playground のコードを読む](tutorial.md)

実験用 Bot を止めてトークンを使い終えたら、PowerShell では `Remove-Item Env:DISCORD_TOKEN`、Bash では `unset DISCORD_TOKEN` で現在のシェルから消去できます。
