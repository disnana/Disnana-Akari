# API リファレンス

## `Bot`

| メソッド | 動作 |
| --- | --- |
| `Bot::new(token) -> Result<Bot>` | トークンの前後の空白を除去して Bot を作ります。値が空の場合はエラーを返します。 |
| `Bot::from_env() -> Result<Bot>` | 環境変数 `DISCORD_TOKEN` から Bot を作ります。 |
| `bot.with_events(events) -> Bot` | Gateway イベントに応じて呼び出す関数の一覧を設定します。 |
| `bot.with_slash_commands(commands) -> Bot` | スラッシュコマンドを指定サーバーで有効にします。 |
| `bot.with_intents(intents) -> Bot` | 自動で選ぶ Gateway Intent を、渡された `Intents` の集合で置き換えます。 |
| `bot.start().await -> Result<()>` | 設定したイベントとスラッシュコマンドで Bot を起動します。 |
| `bot.run_commands(commands).await -> Result<()>` | 上記に加えて、テキストコマンドを実行します。 |
| `bot.run(on_message).await -> Result<()>` | テキストコマンドの代わりに、受信メッセージを自前の `async fn on_message(ctx: Context<'_>) -> Result<()>` で処理します。 |

起動メソッドは Tokio ランタイム内で呼び出します。テキストコマンドやメッセージハンドラーを使う場合は、自動で `GUILD_MESSAGES` と `MESSAGE_CONTENT` を要求します。スラッシュコマンドと `on_ready` のみなら、この二つの Intent は不要です。Intent を上書きする場合は用途に必要なビットを指定します。`Intents` は `disnana_akari::model::gateway::Intents` から参照できます。

Bot と Webhook が送ったメッセージは除外します。復旧可能な Gateway の切断は Twilight が処理し、イベントストリームの終了やスラッシュコマンドの登録失敗は起動メソッドからエラーとして返ります。

### ハンドラーの実行順

Gateway の受信、テキストコマンドとイベントの処理、スラッシュコマンドの処理は、非同期で並行して進みます。テキストコマンドとイベントは同じキュー、スラッシュコマンドは別のキューに入り、それぞれ到着順に処理されます。キューごとに待ち行列の上限は 64 件です。満杯の場合は新しく受け取ったイベントを破棄し、標準エラー出力に記録します。Gateway のイベントストリームが終わると処理中と保留中のイベントも中止します。

ハンドラーが I/O の完了を `.await` している間は Gateway の受信を続けられます。同期 I/O や長い計算は、同じタスク内の他の処理も止めるため、Tokio の `spawn_blocking` などで切り分けます。

## `Commands` と `CommandContext<'_>`

| メソッド | 動作 |
| --- | --- |
| `Commands::new(prefix) -> Result<Commands>` | `!` などのプレフィックスを指定します。空文字列と空白を含む値はエラーです。 |
| `commands.register(name, handler) -> Result<()>` | プレフィックスを省いた名前で非同期関数を登録します。空の名前、空白がある名前、重複した名前はエラーです。 |
| `commands.dispatch(ctx).await -> Result<()>` | 該当するコマンドを実行します。`Bot::run` の独自処理にも組み込めます。 |
| `ctx.args() -> &str` | コマンド名に続く本文を借ります。前後の空白を除去し、引数なしの場合は空文字列を返します。 |
| `ctx.context() -> &Context<'_>` | 元のメッセージを扱うコンテキストを借ります。 |
| `ctx.reply(content).await -> Result<()>` | コマンドのメッセージに返信し、返信先の投稿者や本文中のメンションへの通知を抑止します。 |

名前は大文字と小文字を区別します。最初の空白までがコマンド名です。`! ping` のようにプレフィックスの直後が空白の投稿、未登録の名前、通常の投稿は無視されます。

## `SlashCommands` と `SlashContext<'_>`

| メソッド | 動作 |
| --- | --- |
| `SlashCommands::for_guild(guild_id) -> SlashCommands` | Discord のサーバー ID を指定して空のコマンド一覧を作ります。ID の型は `Id<GuildMarker>` です。 |
| `commands.register(name, description, options, handler) -> Result<()>` | 表示名、説明、引数の定義、呼び出す非同期関数を登録します。Discord の名前や説明の制約、重複名を検証します。 |
| `SlashCommands::string_option(name, description, required) -> CommandOption` | 文字列引数を定義します。`required` が `true` なら Discord の入力欄で必須になります。 |
| `ctx.option(name) -> Option<&CommandOptionValue>` | 指定した名前の引数を Twilight の値として借ります。 |
| `ctx.string(name) -> Option<&str>` | 指定した名前の文字列を借ります。引数がない場合や他の型の場合は `None` を返します。 |
| `ctx.reply(content).await -> Result<()>` | チャンネルに表示される初回応答を送ります。 |
| `ctx.reply_ephemeral(content).await -> Result<()>` | 呼び出したユーザーだけに表示される初回応答を送ります。 |

関数の形は `async fn handler(ctx: &mut SlashContext<'_>) -> Result<()>` です。起動時、同じアプリの同じサーバーにある同名のスラッシュコマンドは更新されます。関数から返るまでに初回応答が成功しなかった場合は、Akari が本人にだけ見えるエラー応答を試みます。

初回応答には Discord の 3 秒の期限があります。現在の API は一度の初回応答に対応しています。返信前の外部 I/O を短く保ってください。返信本文中のメンション通知は抑止します。[Discord の Interaction 応答仕様](https://docs.discord.com/developers/interactions/receiving-and-responding)

## `Events`

| メソッド | 動作 |
| --- | --- |
| `Events::new() -> Events` | 空のイベントハンドラー一覧を作ります。 |
| `events.on_ready(handler)` | Gateway が新規セッションを確立したときに受け取る `&Ready` を渡します。 |
| `events.on_message(handler)` | 人が送った新着メッセージの `&Message` を渡します。通常の投稿とテキストコマンドの両方を受け取ります。 |

同じイベントに複数の関数を登録できます。テキストコマンドを併用すると、メッセージをコマンドとして処理した後に `on_message` の関数を登録順に呼び出します。各関数が返したエラーは標準エラー出力に記録され、次のハンドラーへ進みます。

## `Context<'_>` と `Result<T>`

`Context` は `Bot::run` に渡すメッセージ処理用の型です。`ctx.message()` で元の `&Message` を借りられます。`ctx.reply(content).await` は元のメッセージへの返信を送ります。

`Result<T>` は `std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>` の別名です。関数内で `?` を使うと、失敗時にそのエラーを呼び出し元へ返します。

## 使用例

```rust,no_run
use disnana_akari::{Bot, CommandContext, Commands, Events, Ready, Result, SlashCommands, SlashContext};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut commands = Commands::new("!")?;
    commands.register("ping", ping)?;

    let guild_id = std::env::var("DISCORD_GUILD_ID")?.parse()?;
    let mut slash = SlashCommands::for_guild(guild_id);
    slash.register("ping", "Bot の応答を確認します", vec![], slash_ping)?;

    let mut events = Events::new();
    events.on_ready(ready);

    Bot::from_env()?
        .with_events(events)
        .with_slash_commands(slash)
        .run_commands(commands)
        .await
}

async fn ping(ctx: CommandContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}

async fn slash_ping(ctx: &mut SlashContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}

async fn ready(_event: &Ready) -> Result<()> {
    println!("Bot の準備ができました");
    Ok(())
}
```
