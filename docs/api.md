# API リファレンス

## `Bot`

| メソッド | 動作 |
| --- | --- |
| `Bot::new(token) -> Result<Bot>` | トークンの前後の空白を除去して Bot を作ります。値が空の場合はエラーを返します。 |
| `Bot::from_env() -> Result<Bot>` | 環境変数 `DISCORD_TOKEN` から Bot を作ります。 |
| `bot.with_events(events) -> Bot` | Gateway イベントに応じて呼び出す関数の一覧を設定します。 |
| `bot.with_slash_commands(commands) -> Bot` | スラッシュコマンドを指定サーバーで有効にします。 |
| `bot.with_intents(intents) -> Bot` | 自動で選ぶ Gateway Intent を、渡された `Intents` の集合で置き換えます。 |
| `bot.add_intents(intents) -> Bot` | 自動で選ぶ集合または `with_intents` で指定した集合に、渡された `Intents` を追加します。 |
| `bot.start().await -> Result<()>` | 設定したイベントとスラッシュコマンドで Bot を起動します。 |
| `bot.run_commands(commands).await -> Result<()>` | 上記に加えて、テキストコマンドを実行します。 |
| `bot.run(on_message).await -> Result<()>` | テキストコマンドの代わりに、受信メッセージを自前の `async fn on_message(ctx: Context<'_>) -> Result<()>` で処理します。 |

起動メソッドは Tokio ランタイム内で呼び出します。テキストコマンドやメッセージハンドラーを使う場合は、自動で `GUILD_MESSAGES` と `MESSAGE_CONTENT` を要求します。スラッシュコマンドと `on_ready` のみなら、この二つの Intent は不要です。

`with_intents` を使う場合、テキストコマンドには `GUILD_MESSAGES` と `MESSAGE_CONTENT` を明示的に含めます。自動で選ぶ Intent に別のビットを足すときは `add_intents` を使います。両方を指定した場合は、呼ぶ順序に関係なく `with_intents` で最後に指定した集合に、`add_intents` の集合が加わります。`Intents` は `disnana_akari::model::gateway::Intents` から参照できます。

Bot と Webhook が送ったメッセージは除外します。復旧可能な Gateway の切断は Twilight が処理し、イベントストリームの終了やスラッシュコマンドの登録失敗は起動メソッドからエラーとして返ります。

### ハンドラーの実行順

Gateway の受信、テキストコマンドとイベントの処理、スラッシュコマンドの処理は、非同期で並行して進みます。テキストコマンドとイベントは一つのキューで到着順に処理し、待ち行列は最大 64 件です。満杯時は新しく届いたイベントを破棄し、標準エラー出力に記録します。

スラッシュコマンドの同時実行枠は 8 件で、受信から処理への引き渡しに最大 64 件のキューを使います。同時実行枠やキューが埋まっている場合、別の処理枠から本人にだけ見える混雑通知を試みます。混雑通知も同時 8 件、待ち行列 64 件までです。通知用の待ち行列が満杯の場合や、Discord への通知が失敗した場合は、標準エラー出力に記録します。Gateway のイベントストリームが終わると処理中と保留中のイベントも中止します。

ハンドラーが I/O の完了を `.await` している間は Gateway の受信を続けられます。同期 I/O や長い計算は、同じタスク内の他の処理も止めるため、Tokio の `spawn_blocking` などで切り分けます。

## `Commands` と `CommandContext<'_>`

| メソッド | 動作 |
| --- | --- |
| `Commands::new(prefix) -> Result<Commands>` | `!` などのプレフィックスを指定します。空文字列と空白を含む値はエラーです。 |
| `commands.register(name, handler) -> Result<()>` | プレフィックスを省いた名前で非同期関数を登録します。空の名前、空白がある名前、重複した名前はエラーです。 |
| `commands.dispatch(ctx).await -> Result<()>` | 該当するコマンドを実行します。`Bot::run` の独自処理にも組み込めます。 |
| `ctx.args() -> &str` | コマンド名に続く本文を借ります。前後の空白を除去し、引数なしの場合は空文字列を返します。 |
| `ctx.context() -> &Context<'_>` | 元のメッセージを扱うコンテキストを借ります。 |
| `ctx.timing() -> Timing` | このテキストコマンド呼び出しの測定値を取得します。 |
| `ctx.reply(content).await -> Result<()>` | コマンドのメッセージに返信し、返信先の投稿者や本文中のメンションへの通知を抑止します。 |

名前は大文字と小文字を区別します。最初の空白までがコマンド名です。`! ping` のようにプレフィックスの直後が空白の投稿、未登録の名前、通常の投稿は無視されます。

`Bot::run_commands` で登録済みのコマンドの実行に失敗した場合は、元の投稿に「コマンドを処理できませんでした。」と返信を試みます。エラーの詳細は標準エラー出力に記録されます。`commands.dispatch` を独自の処理に組み込む場合は、その呼び出し元でエラーを扱えます。

## `SlashCommands` と `SlashContext<'_>`

| メソッド | 動作 |
| --- | --- |
| `SlashCommands::for_guild(guild_id) -> SlashCommands` | Discord のサーバー ID を指定して空のコマンド一覧を作ります。ID の型は `Id<GuildMarker>` です。 |
| `commands.replace_guild_commands() -> SlashCommands` | 起動時に、このアプリのサーバー内の全コマンドを登録した一覧に一括で置き換えます。 |
| `commands.register(name, description, options, handler) -> Result<()>` | 表示名、説明、引数の定義、呼び出す非同期関数を登録します。Discord の名前や説明の制約、重複名を検証します。 |
| `SlashCommands::string_option(name, description, required) -> CommandOption` | 文字列引数を定義します。`required` が `true` なら Discord の入力欄で必須になります。 |
| `ctx.option(name) -> Option<&CommandOptionValue>` | 指定した名前の引数を Twilight の値として借ります。 |
| `ctx.string(name) -> Option<&str>` | 指定した名前の文字列を借ります。引数がない場合や他の型の場合は `None` を返します。 |
| `ctx.author_id() -> Option<Id<UserMarker>>` | 呼び出したユーザーの ID を取得します。 |
| `ctx.timing() -> Timing` | このスラッシュコマンド呼び出しの測定値を取得します。 |
| `ctx.defer().await -> Result<()>` | チャンネルに表示される処理中の初回応答を送ります。 |
| `ctx.defer_ephemeral().await -> Result<()>` | 呼び出したユーザーだけに表示される処理中の初回応答を送ります。 |
| `ctx.reply(content).await -> Result<()>` | チャンネルに初回応答を送ります。`defer` または `defer_ephemeral` の後は、公開範囲を維持して処理中の表示を本文に置き換えます。 |
| `ctx.reply_ephemeral(content).await -> Result<()>` | 呼び出したユーザーだけに初回応答を送ります。`defer_ephemeral` の後は、その処理中の表示を本文に置き換えます。 |
| `ctx.followup(content).await -> Result<()>` | 最初の本文を送った後に、チャンネルに追加のメッセージを送ります。 |
| `ctx.followup_ephemeral(content).await -> Result<()>` | 最初の本文を送った後に、呼び出したユーザーだけに追加のメッセージを送ります。 |

### サーバーに登録するコマンドを同期する

既定の起動では、同名のスラッシュコマンドを追加または更新し、ほかの既存コマンドを保持します。Akari に登録していないサーバー限定のスラッシュコマンドを受信した場合は、本人にだけ「このコマンドは現在利用できません。」と返します。

`replace_guild_commands()` を指定すると、登録一覧に含まれないコマンドが対象サーバーから削除されます。削除の範囲は同じアプリ・同じサーバーの全アプリケーションコマンドで、ユーザーやメッセージを右クリックして実行するコマンドも対象です。空の一覧で一括同期すると対象の全コマンドが削除されます。[Discord のサーバー単位の一括上書き](https://docs.discord.com/developers/interactions/application-commands#bulk-overwrite-guild-application-commands)

### 時間がかかる処理に応答する

関数の形は `async fn handler(ctx: &mut SlashContext<'_>) -> Result<()>` です。Discord は Interaction の受信後 3 秒以内に初回応答を求めます。外部サービスへの問い合わせなどがある場合は、先に `ctx.defer().await?` または `ctx.defer_ephemeral().await?` を呼び、処理後に `ctx.reply(...).await?` で処理中の表示を本文に置き換えます。公開範囲は `defer` または `defer_ephemeral` の時点で決まります。公開の `defer` の後に `reply_ephemeral` を呼ぶとエラーになります。`followup` と `followup_ephemeral` は最初の本文を送った後に呼びます。Discord の Interaction トークンの有効期間は 15 分です。[Discord の Interaction 応答仕様](https://docs.discord.com/developers/interactions/receiving-and-responding)

ハンドラーが最初の本文を送れずに終了した場合は、Akari がエラー応答を試みます。初回応答が済んでいない場合は本人にだけ表示し、処理中の応答がある場合はその表示をエラー文に置き換えます。返信本文中のメンション通知は抑止します。

## `register_hybrid` と `HybridContext<'_>`

`register_hybrid(&mut commands, slash, name, description, options, handler)` は、テキストコマンドとスラッシュコマンドに同じ関数を登録します。二つ目の引数に `Some(&mut slash_commands)` を渡すと両方に登録し、`None` を渡すとテキストコマンドだけに登録します。`options` はスラッシュ側の入力欄の定義です。登録先に同名コマンドがある場合や、定義が登録先の形式に合わない場合は、追加前にエラーを返します。

ハンドラーは `async fn handler(ctx: &mut HybridContext<'_>) -> Result<()>` の形で定義します。

| メソッド | 動作 |
| --- | --- |
| `ctx.source() -> CommandSource` | テキストから呼ばれた場合は `CommandSource::Text`、スラッシュからなら `CommandSource::Slash` を返します。 |
| `ctx.author_id() -> Option<Id<UserMarker>>` | 呼び出したユーザーの ID を返します。取得できない場合は `None` を返します。 |
| `ctx.args() -> Option<&str>` | テキスト側ではコマンド名に続く本文を `Some` で返します。スラッシュ側では `None` を返します。 |
| `ctx.option(name) -> Option<&CommandOptionValue>` | スラッシュ側の名前付き引数を借ります。テキスト側や該当する引数がない場合は `None` を返します。 |
| `ctx.string(name) -> Option<&str>` | スラッシュ側の文字列引数を借ります。テキスト側、引数なし、他の型の場合は `None` を返します。 |
| `ctx.text_arg(name) -> Option<&str>` | スラッシュでは同名の文字列引数、テキストでは最初に定義したトップレベルの文字列引数に対応する残りの本文を借ります。値が空か空白だけの場合は `None` です。 |
| `ctx.timing() -> Timing` | 呼び出し元に応じた測定値を取得します。 |
| `ctx.reply(content).await -> Result<()>` | テキストなら元の投稿に返信し、スラッシュなら公開の初回応答を送ります。 |

テキスト側の `text_arg` は、コマンド名の後ろ全体を一つの値として返します。二つ目以降の文字列引数や他の型の引数を読む場合は、必要に応じて `args()` や `option()` を使います。ユーザーごとの呼び出し頻度を制限するときは `ctx.author_id()` と `Cooldown` を組み合わせられます。

## `Cooldown`

`Cooldown::new(interval)` に `std::time::Duration` を渡すと、ユーザーごとの実行間隔を管理できます。`check(user_id)` が `Ok(())` を返した時点で実行が記録され、同じユーザーが間隔を空けずに呼ぶと `Err(残り時間)` が返ります。拒否された呼び出しは待ち時間を延ばしません。

状態は Bot プロセス内に保存されます。別のコマンドや呼び出し方に個別の制限を設けるには、別の `Cooldown` を作ります。テキストとスラッシュで同じ `Cooldown` を使うと、待ち時間も共有できます。[Playground の適用例](tutorial.md)

## `Timing`

| メソッド | 測定範囲 |
| --- | --- |
| `gateway_rtt() -> Option<Duration>` | イベントを受信した時点で確認できた、Bot と Discord Gateway 間の直近のハートビートを送ってから Discord の応答を受け取るまでの往復時間です。応答が得られるまでは `None` です。 |
| `queue_wait() -> Duration` | Akari がイベントを受信してから、処理する側が内部のキューから取り出すまでの時間です。 |
| `handler_elapsed() -> Duration` | このコマンドの処理を始めてから、経過時間を取得するまでの時間です。 |

`Duration` は Rust 標準ライブラリの時間の長さを表す型です。サンプルは各値をミリ秒で表示し、処理時間は返信を送る直前に取得します。

## `Events`

| メソッド | 動作 |
| --- | --- |
| `Events::new() -> Events` | 空のイベントハンドラー一覧を作ります。 |
| `events.on_ready(handler)` | Gateway が新規セッションを確立したときに受け取る `&Ready` を渡します。 |
| `events.on_message(handler)` | 人が送った新着メッセージの `&Message` を渡します。通常の投稿とテキストコマンドの両方を受け取ります。 |

同じイベントに複数の関数を登録できます。テキストコマンドを併用すると、メッセージをコマンドとして処理した後に `on_message` の関数を登録順に呼び出します。各関数が返したエラーは標準エラー出力に記録され、次のハンドラーへ進みます。

## `Context<'_>` と `Result<T>`

`Context` は `Bot::run` に渡すメッセージ処理用の型です。`ctx.message()` で元の `&Message` を借りられます。`ctx.reply(content).await` は元のメッセージへの返信を送ります。`ctx.timing()` で受信から処理までの測定値を取得できます。

`Result<T>` は `std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>` の別名です。関数内で `?` を使うと、失敗時にそのエラーを呼び出し元へ返します。

## 使用例

```rust,no_run
use disnana_akari::{
    Bot, CommandContext, Commands, Events, HybridContext, Ready, Result, SlashCommands,
    SlashContext, register_hybrid,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut commands = Commands::new("!")?;
    commands.register("ping", ping)?;

    let guild_id = std::env::var("DISCORD_GUILD_ID")?.parse()?;
    let mut slash = SlashCommands::for_guild(guild_id);
    slash.register("ping", "Bot の応答を確認します", vec![], slash_ping)?;
    register_hybrid(
        &mut commands,
        Some(&mut slash),
        "echo",
        "入力した文章を返します",
        vec![SlashCommands::string_option("text", "返す文章", true)],
        echo,
    )?;
    register_hybrid(
        &mut commands,
        Some(&mut slash),
        "latency",
        "Bot のキュー待ちを確認します",
        vec![],
        latency,
    )?;

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

async fn echo(ctx: &mut HybridContext<'_>) -> Result<()> {
    let Some(text) = ctx.text_arg("text") else {
        return ctx.reply("返したい文章を入力してください。").await;
    };
    ctx.reply(text).await
}

async fn latency(ctx: &mut HybridContext<'_>) -> Result<()> {
    let wait = ctx.timing().queue_wait();
    ctx.reply(&format!("キュー待ち {:.3} ms", wait.as_secs_f64() * 1_000.0))
        .await
}

async fn ready(_event: &Ready) -> Result<()> {
    println!("Bot の準備ができました");
    Ok(())
}
```
