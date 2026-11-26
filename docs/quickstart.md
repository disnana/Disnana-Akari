# クイックスタート

## Discord に Bot を用意する

1. Rust 1.89 以上を用意します。
2. Discord Developer Portal でアプリを作成します。**Bot** タブで **Message Content Intent** を有効にし、Bot トークンを取得します。
3. アプリを `bot` スコープでテスト用サーバーにインストールします。このスコープには `applications.commands` が含まれます。試すチャンネルで **View Channel**、**Send Messages**、**Read Message History** を許可します。返信には元メッセージを読む権限が必要です。

[Discord Developer Portal](https://discord.com/developers/applications) · [Discord の Message Content Intent](https://docs.discord.com/developers/events/gateway#message-content-intent) · [Discord のメッセージ作成 API](https://docs.discord.com/developers/resources/message#create-message) · [アプリのスコープとコマンド登録](https://docs.discord.com/developers/interactions/application-commands)

## Playground を起動する

`playground/` がまだない場合は、後述の手順で作成します。`playground/.env` を開き、`DISCORD_TOKEN=` の右辺に Bot トークンを貼り付けます。スラッシュコマンドも試す場合は、Discord の設定で開発者モードを有効にし、テスト用サーバーを右クリックして **サーバーIDをコピー** を選びます。コピーした数字を `DISCORD_GUILD_ID=` の右辺に貼り付けます。UTF-8 で保存し、値は引用符を付けずに書きます。`playground/` は Git の除外対象です。

```dotenv
DISCORD_TOKEN=
DISCORD_GUILD_ID=
DISCORD_REPLACE_GUILD_COMMANDS=false
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
| メッセージとして `!latency` を送る | Gateway RTT、キュー待ち、返信直前までの処理時間 |
| 自分でインストールした Bot の `/ping` を選んで実行する | `Pong!` |
| 同じ Bot の `/echo` を選び、`text` 欄に `こんにちは` と入力して実行する | `こんにちは` |
| 同じ Bot の `/latency` を選んで実行する | `!latency` と同じ 3 項目 |
| 同じ Bot の `/ui` を実行して、表示された **Ping** ボタンを押す | 埋め込みとボタンがチャンネルに表示され、`Pong!` は押した人にだけ表示されます。 |
| `/ui` の **更新** ボタンを押す | ボタンが付いているメッセージの表示が変わります。 |
| `/ui` の **遅延更新** ボタンを押す | 約 4 秒後に元のメッセージが更新され、完了の通知は押した人にだけ表示されます。 |
| 同じ Bot の `/components` を実行し、選択メニューで **概要** または **詳細** を選ぶ | Components V2 の文章、区切り線、選択メニューとボタンが表示されます。選択への返信は操作した人にだけ表示されます。 |
| `/components` の **フォームを開く** を押し、**メモ** を入力して送信する | フォームが開き、送信すると「入力を受け取りました。」が本人にだけ表示されます。 |
| 同じ Bot の `/entities` を実行し、ユーザー、ロール、テキストチャンネルのどれかを選ぶ | 選んだ項目の ID が本人にだけ表示されます。 |

遅延はミリ秒（ms）で表示されます。接続直後など、Gateway からハートビートの応答をまだ受け取っていない場合は、Gateway RTT が「未計測」と表示されます。[測定値の定義](api.md#timing)

スラッシュコマンドは `DISCORD_GUILD_ID` で指定したサーバーで使えます。ID を省略するとテキストコマンドとイベント処理で起動します。Bot を止めるには、RustRover の停止ボタンか `Ctrl+C` を使います。

既定の起動では、コードにある同名のスラッシュコマンドを追加または更新します。**テストサーバー内で、この Bot に登録済みの全アプリケーションコマンドを現在の定義に置き換える**場合は `DISCORD_REPLACE_GUILD_COMMANDS=true` を設定します。コードから削除したコマンドに加え、同じ Bot の別名コマンドや、ユーザー・メッセージの右クリックメニューにあるアプリコマンドも、同期する一覧に含まれなければ削除されます。[Discord のサーバー単位の一括上書き](https://docs.discord.com/developers/interactions/application-commands#bulk-overwrite-guild-application-commands)

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

作成済みなら、`playground/src/commands.rs` でコマンド、`playground/src/events.rs` でイベントを編集して **Akari Playground** を実行します。コマンドごとの UI は `playground/src/ui.rs`、`playground/src/components.rs`、`playground/src/entities.rs` にあります。自作のコードがある場合は、`examples/ping/` と `playground/src/` の差分を見ながら新しいサンプルを取り込めます。

新しくクローンした環境では、ルートで次のコマンドを実行してフォルダを作ります。既存の `playground/` がある場合はこの手順を飛ばしてください。

```powershell
cargo new --bin --vcs none --name akari-playground playground
cargo add --manifest-path playground/Cargo.toml --path .
cargo add --manifest-path playground/Cargo.toml tokio --features macros,rt,time
Copy-Item examples/ping/*.rs playground/src/
New-Item -ItemType File -Path playground/.env
cargo generate-lockfile --manifest-path playground/Cargo.toml
```

`playground/.env` にトークンを入れ、必要に応じてテスト用サーバー ID も設定します。[Playground のコードを読む](tutorial.md)

実験用 Bot を止めてトークンを使い終えたら、PowerShell では `Remove-Item Env:DISCORD_TOKEN`、Bash では `unset DISCORD_TOKEN` で現在のシェルから消去できます。
