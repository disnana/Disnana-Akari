# スラッシュコマンド

`/ping` のように、Discord の入力欄で `/` を打つと出てくるメニューから呼び出すコマンドです。説明や入力欄が Discord に表示されるので、利用者にとって分かりやすい方法です。

> [!NOTE]
> 現在の Akari は、スラッシュコマンドを **サーバーを 1 つ指定して** 登録します。指定したサーバーでは、Bot の起動時に登録され、すぐに使えます。

## 基本

```rust
use disnana_akari::{Bot, Result, SlashCommands, SlashContext};
use disnana_akari::model::id::Id;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    // テスト用サーバーの ID（開発者モードで「サーバー ID をコピー」）
    let guild_id = Id::new(123456789012345678);

    let mut slash = SlashCommands::for_guild(guild_id);
    slash.register("ping", "Bot の応答を確認します", vec![], ping)?;

    Bot::from_env()?.with_slash_commands(slash).start().await
}

async fn ping(ctx: &mut SlashContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}
```

`register` の引数は、順に「名前」「説明」「引数の定義」「呼び出す関数」です。名前や説明が Discord の制約に合わないとき、または名前が重複しているときは、起動前にエラーになります。

起動は `start()` です。テキストコマンドも使う場合は、`run_commands(commands)` に切り替えます。

<!-- no-check -->
```rust
Bot::from_env()?
    .with_slash_commands(slash)
    .run_commands(commands)
    .await
```

> [!IMPORTANT]
> スラッシュコマンドは **必ず応答** してください。ハンドラーが何も返信せずに終わると、Akari が「コマンドを処理できませんでした。」と（本人にだけ）返します。この文言は `Messages::slash_command_failed` で変えられます（[エラーと自動で送る文言](errors.md)）。

## すべてのサーバーで使う（グローバルコマンド）

`SlashCommands::for_guild` の代わりに `SlashCommands::global()` を使うと、Bot が入っているすべてのサーバーと DM でコマンドを使えます。Discord 側の反映に時間がかかることがあるため、開発中は `for_guild`、公開するときに `global` にするのがおすすめです。

```rust
use disnana_akari::{Bot, Result, SlashCommands, SlashContext};

async fn ping(ctx: &mut SlashContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut slash = SlashCommands::global();
    slash.register("ping", "Bot の応答を確認します", vec![], ping)?;

    Bot::from_env()?.with_slash_commands(slash).start().await
}
```

同じ Bot に、グローバルとサーバー限定のコマンドを同時には登録できません。

## 引数を受け取る

引数の定義は、`register` の 3 番目の引数に渡します。文字列の引数は `SlashCommands::string_option` で作れます。

```rust
use disnana_akari::{Result, SlashCommands, SlashContext};

fn register(slash: &mut SlashCommands) -> Result<()> {
    // 引数名 / 説明 / 必須かどうか
    let text = SlashCommands::string_option("text", "返す文章", true);
    slash.register("echo", "入力した文章を返します", vec![text], echo)
}

async fn echo(ctx: &mut SlashContext<'_>) -> Result<()> {
    let Some(text) = ctx.string("text") else {
        return ctx.reply_ephemeral("文章を入力してください。").await;
    };
    ctx.reply(text).await
}
```

`ctx.string("text")` は、引数がない場合や、文字列でない場合は `None` を返します。

### 整数など、文字列以外の引数

`integer_option`、`number_option`、`boolean_option` で、種類ごとの引数を作れます。値は `ctx.integer`、`ctx.number`、`ctx.boolean` で受け取ります。返される値は Twilight の `CommandOption` なので、範囲などは書き換えられます。

```rust
use disnana_akari::{Result, SlashCommands, SlashContext};

fn register(slash: &mut SlashCommands) -> Result<()> {
    let mut count = SlashCommands::integer_option("count", "回数（1〜10）", true);
    count.min_value = Some(1.into());
    count.max_value = Some(10.into());
    slash.register("repeat", "回数ぶん挨拶します", vec![count], repeat)
}

async fn repeat(ctx: &mut SlashContext<'_>) -> Result<()> {
    let Some(count) = ctx.integer("count") else {
        return ctx.reply_ephemeral("回数を指定してください。").await;
    };
    ctx.reply(&"こんにちは！ ".repeat(count as usize)).await
}
```

### 文字列を好きな型に変える

`ctx.parse::<T>(name)` は、文字列の引数を `FromStr` で変換します。引数がなければ `None`、入力があって変換に失敗したら `Some(Err(..))` です。エラーはそのまま `?` で返せます。

```rust
use disnana_akari::{Result, SlashContext};

async fn double(ctx: &mut SlashContext<'_>) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(value) = ctx.parse::<i64>("value") else {
        ctx.reply_ephemeral("数を入力してください。").await?;
        return Ok(());
    };
    ctx.reply(&(value? * 2).to_string()).await?;
    Ok(())
}
```

テキストとスラッシュを兼ねる[ハイブリッドコマンド](hybrid-commands.md)でも、`ctx.parse::<T>(name)` を同じように使えます（文字列の引数が対象です）。

## 呼び出した人やチャンネルを調べる

| メソッド | 返す値 |
| --- | --- |
| `ctx.author_id()` | 呼び出したユーザーの ID |
| `ctx.channel_id()` | コマンドが実行されたチャンネルの ID |
| `ctx.option(name)` | 引数（Twilight の値） |
| `ctx.string(name)` | 文字列の引数 |
| `ctx.integer(name)` / `ctx.number(name)` / `ctx.boolean(name)` | 整数、数値、真偽値の引数 |
| `ctx.parse::<T>(name)` | 文字列の引数を `FromStr` で変換した結果 |

## 返信のしかた

スラッシュコマンドの応答には決まりがあります。たとえば、**最初の応答は 3 秒以内** に送る必要があります。詳しくは [応答のしかた](responding.md) を読んでください。よく使うのは次の 3 つです。

```rust
use disnana_akari::{Result, SlashContext};

async fn examples(ctx: &mut SlashContext<'_>) -> Result<()> {
    // 全員に見える返信
    ctx.reply("みんなに見えます").await?;
    // 追加のメッセージ（最初の返信のあとに送れる）
    ctx.followup_ephemeral("あなただけに見えます").await
}
```

| メソッド | 動作 |
| --- | --- |
| `ctx.reply(text)` | チャンネルに返信します。 |
| `ctx.reply_ephemeral(text)` | 呼び出した本人にだけ見える返信です。 |
| `ctx.defer()` / `ctx.defer_ephemeral()` | 「考え中…」を表示して、あとで返信します。 |
| `ctx.followup(text)` / `ctx.followup_ephemeral(text)` | 最初の返信のあとに、追加のメッセージを送ります。 |
| `ctx.reply_ui(&message)` | [Embed やボタン](buttons.md)付きで返信します。 |
| `ctx.reply_v2(&message)` | [Components V2](components-v2.md) で返信します。 |
| `ctx.show_modal(&form)` | [入力フォーム](modals.md)を開きます。 |

## 時間のかかる処理

データベースや外部 API を待つ処理は、3 秒を超えるかもしれません。先に `defer` で受け付けておきます。

```rust
use disnana_akari::{Result, SlashContext};

async fn report(ctx: &mut SlashContext<'_>) -> Result<()> {
    ctx.defer_ephemeral().await?;   // 3 秒以内に「処理中」を出す
    // ここで時間のかかる処理を待つ
    ctx.reply("集計が終わりました。").await?; // 「処理中」を結果に置き換える
    ctx.followup_ephemeral("条件を変えると再集計できます。").await
}
```

## 登録済みのコマンドを整理する

既定では、コードにある名前のコマンドを追加または更新するだけで、ほかのコマンドは残ります。コードから消したコマンドが Discord に残ったときは、次のように **一括同期** を指定します。

```rust
use disnana_akari::SlashCommands;
use disnana_akari::model::id::Id;

fn build() -> SlashCommands {
    SlashCommands::for_guild(Id::new(123456789012345678)).replace_commands()
}
```

> [!WARNING]
> 一括同期は、このアプリがそのサーバーに登録している **すべての** アプリケーションコマンド（右クリックメニューのコマンドを含む）を、今の一覧で置き換えます。一覧が空なら、すべて削除されます。テスト用のサーバーで使ってください。

Akari に登録していないコマンドが実行されたときは、`Bot::on_error` に報告し、本人にだけ「このコマンドは現在利用できません。」と返します（`Messages::slash_command_unavailable`）。
