# API リファレンス

## `Bot`

| メソッド | 動作 |
| --- | --- |
| `Bot::new(token: &str) -> Result<Bot>` | 渡されたトークンの前後の空白を除去して Bot を作ります。値が空の場合はエラーを返します。 |
| `Bot::from_env() -> Result<Bot>` | `DISCORD_TOKEN` を読み込み、前後の空白を除去して Bot を作ります。値が空の場合はエラーを返します。 |
| `bot.run(on_message).await -> Result<()>` | Tokio 上で Discord に接続し、サーバーの新着メッセージをハンドラーへ渡します。 |
| `bot.run_commands(commands).await -> Result<()>` | 登録したテキストコマンドを受け付けます。接続やエラー処理は `run` と共通です。 |

ハンドラーは `async fn on_message(ctx: Context<'_>) -> Result<()>` の形で定義できます。`run` は `GUILD_MESSAGES` と `MESSAGE_CONTENT` を要求し、Bot と Webhook の投稿を除外します。ハンドラーは順番に実行されます。返信の待機も含め、ハンドラーの実行中は Gateway の受信処理が待機します。

Gateway で復旧可能な切断が起きた場合は Twilight が再接続を処理します。イベントストリームが終了すると `run` がエラーを返します。

## `Context<'_>`

| メソッド | 動作 |
| --- | --- |
| `ctx.message() -> &Message` | イベントが持つ Twilight のメッセージを借り、本文や投稿者、チャンネル ID を参照します。 |
| `ctx.reply(content).await -> Result<()>` | Discord の返信機能を使い、受信したメッセージに返信します。返信先の投稿者や本文中のメンションへの通知は抑止します。 |

`reply` はリクエストが成功すると `Ok(())` を返します。入力が Discord の制約に合わない場合や、権限不足・通信エラーの場合は失敗します。

## `Commands`

| メソッド | 動作 |
| --- | --- |
| `Commands::new(prefix) -> Result<Commands>` | `!` などのプレフィックスを指定します。空文字列と空白を含む値はエラーです。 |
| `commands.register(name, handler) -> Result<()>` | プレフィックスを省いた名前で非同期関数を登録します。空の名前、空白がある名前、重複した名前はエラーです。 |
| `commands.dispatch(ctx).await -> Result<()>` | 該当するコマンドを実行します。`Bot::run` で独自のメッセージ処理と組み合わせるときにも使えます。 |

名前は大文字と小文字を区別します。最初の空白までがコマンド名です。`! ping` のようにプレフィックスの直後が空白の投稿、未登録の名前、通常の投稿は無視されます。

## `CommandContext<'_>`

| メソッド | 動作 |
| --- | --- |
| `ctx.args() -> &str` | コマンド名に続く本文を借ります。前後の空白を除去し、引数なしの場合は空文字列を返します。 |
| `ctx.context() -> &Context<'_>` | 元メッセージのコンテキストを借ります。 |
| `ctx.reply(content).await -> Result<()>` | コマンドが書かれたメッセージに返信します。 |

## `Result<T>`

`std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>` の別名です。ハンドラー内の `?` でエラーを返すと、Akari が標準エラー出力に記録します。

## 使用例

```rust,no_run
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
