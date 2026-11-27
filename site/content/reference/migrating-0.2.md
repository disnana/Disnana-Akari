# 0.1 から 0.2 への移行

0.2 は、勝手な動作を減らして、エラーと文言を呼び出し側が決められるようにした版です。互換性のない変更がいくつかあります。上から順に確認してください。全体の一覧は[変更履歴](changelog.md)にあります。

## 1. Intent を自分で指定する

0.1 は、テキストコマンドやメッセージのハンドラーを登録すると、特権 Intent の `MESSAGE_CONTENT` も黙って要求していました。0.2 では、自動で足しません。

```rust
use disnana_akari::model::gateway::Intents;
use disnana_akari::{Bot, Commands, Result};

async fn run(commands: Commands) -> Result<()> {
    Bot::from_env()?
        .add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)
        .run_commands(commands)
        .await
}
```

足りないと、Discord に接続する前に、足りない Intent を示すエラーを返します。開発者ポータルで `MESSAGE_CONTENT` を有効にするのも忘れないでください（[Intent の設定](../guide/intents.md)）。

## 2. 標準出力とエラーの扱い

- 接続時の「Discord に接続しました」は、出さなくなりました。知りたいときは `Events::on_ready` に書きます。
- エラーは、既定では標準エラー出力に `[akari] ...` の形で書きます。自分で受け取るには `Bot::on_error` を使います（[エラーと自動で送る文言](../guide/errors.md)）。
- **テキストコマンドの失敗時の公開返信を、既定で送らなくなりました。** 送りたいときは `Messages::text_command_failed` を設定します。スラッシュコマンド、ボタンなどの本人向けの返信は、これまでどおり残っています。

## 3. ハンドラーが返すエラーの型

ハンドラーは、`io::Error` や `ParseIntError` など、任意のエラーを返せるようになりました（`?` がそのまま使えます）。

クロージャが `Ok(())` で終わる場合は、戻り値の型を書いてください。

```rust
use disnana_akari::{CommandContext, Commands, Result};

fn register(commands: &mut Commands) -> Result<()> {
    // 0.1: async |ctx: CommandContext<'_>| { ...; Ok(()) }
    commands.register("ping", async |ctx: CommandContext<'_>| -> Result<()> {
        ctx.reply("Pong!").await
    })
}
```

## 4. `Error` の判別

`Result<T>` が返す `Error` は、文字列ではなく、種類ごとのバリアントで判別できるようになりました。メッセージを `match` していた場合は置き換えます。

| 0.1 | 0.2 |
| --- | --- |
| `Error::InvalidRegistration(文字列)` | `Error::Registration(RegistrationError)` |
| `Error::InvalidSelection(文字列)` | `Error::Selection(SelectionError)` |
| `Error::InvalidResponseState(文字列)` | `Error::ResponseState(ResponseStateError)` |
| `Error::CommandSync(文字列)` | `Error::CommandSync(CommandSyncError)` |
| `Error::QueueFull(文字列)` | `Error::QueueFull(QueueKind)` |
| `Error::Handler(Box<dyn Error>)` | `Error::Handler(HandlerError)`（`downcast_ref`、`into_inner` で元のエラーを取り出せます） |

`Error`、`CommandSource`、`Mentionable` などは `#[non_exhaustive]` です。外部クレートの `match` には `_` の分岐を付けてください。

## 5. スラッシュコマンドの一括同期

`SlashCommands::replace_guild_commands()` は、`replace_commands()` に名前を変えました（グローバルコマンドにも使えるためです）。`InfoEvent::SlashCommandsSynced` の `guild_id` は `Option` です。

## 6. TLS の暗号プロバイダー

Akari は、`rustls` の暗号プロバイダーを登録しなくなりました。`ring` だけが有効なら、`rustls` が自動で選びます。アプリで別のプロバイダーも有効にする場合は、起動前にアプリ側で登録してください。

## 新しい機能

移行のあとに、次の機能が使えます。

- [権限の確認](../guide/permissions.md)（`ctx.require_permissions(..)?`）
- [サブコマンド、オートコンプリート、グローバルコマンド、型付きの引数](../guide/slash-commands.md)
- [モジュールで整理する](../guide/modules.md)
- `Bot::limits`（処理枠とキューの大きさ）、`Bot::include_bots`、`ctx.reply_with`（通知する相手の指定）
