# ハイブリッドコマンド

`!echo こんにちは` と `/echo text:こんにちは` を、**同じ関数** で処理したいときに使います。同じ処理を 2 回書かずに済みます。

## 基本

```rust
use disnana_akari::model::gateway::Intents;
use disnana_akari::model::id::Id;
use disnana_akari::{
    Bot, Commands, HybridContext, Result, SlashCommands, register_hybrid,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut commands = Commands::new("!")?;
    let mut slash = SlashCommands::for_guild(Id::new(123456789012345678));

    register_hybrid(
        &mut commands,
        Some(&mut slash),            // スラッシュの登録先。None ならテキストだけ
        "echo",                      // コマンド名
        "入力した文章を返します",     // スラッシュの説明
        vec![SlashCommands::string_option("text", "返す文章", true)],
        echo,
    )?;

    Bot::from_env()?
        // テキスト側（!echo）のために必要
        .add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)
        .with_slash_commands(slash)
        .run_commands(commands)
        .await
}

async fn echo(ctx: &mut HybridContext<'_>) -> Result<()> {
    let Some(text) = ctx.text_arg("text") else {
        return ctx.reply("文章を入力してください。").await;
    };
    ctx.reply(text).await
}
```

`register_hybrid` は、テキストとスラッシュのどちらにも同名のコマンドが登録できるか先に確認します。どちらかが重複している場合は、**どちらにも追加せずに** エラーを返します。

## 引数の読み方

| メソッド | テキスト（`!echo ...`） | スラッシュ（`/echo ...`） |
| --- | --- | --- |
| `ctx.text_arg("text")` | 最初の文字列引数に、コマンド名のあとの本文を丸ごと割り当てます。 | 同名の文字列引数。 |
| `ctx.args()` | 本文（`Some`） | `None` |
| `ctx.string("text")` | `None` | 同名の文字列引数 |
| `ctx.option("count")` | `None` | 同名の引数（整数など） |

`text_arg` は、空や空白だけの入力を `None` として扱います。「文章を入力してください」の分岐に使えます。

> [!NOTE]
> テキスト側は、コマンド名のあとの本文 **全体** が最初の文字列引数になります。`count`（整数）と `text`（文字列）を定義して `!cmd 3 hello` と送ると、`text_arg("text")` は `3 hello` です。整数などの引数をテキストでも使いたいときは、`ctx.args()` を自分で分割してください。

## 呼び出し方で分岐する

```rust
use disnana_akari::{CommandSource, HybridContext, Result};

async fn where_from(ctx: &mut HybridContext<'_>) -> Result<()> {
    let text = match ctx.source() {
        CommandSource::Text => "テキストコマンドから呼ばれました。",
        CommandSource::Slash => "スラッシュコマンドから呼ばれました。",
        _ => "この呼び出し方には対応していません。",
    };
    ctx.reply(text).await
}
```

## 使えるメソッド

| メソッド | 動作 |
| --- | --- |
| `ctx.source()` | `CommandSource::Text` か `CommandSource::Slash` |
| `ctx.author_id()` | 呼び出したユーザーの ID |
| `ctx.reply(text)` | テキストなら元の投稿に返信、スラッシュなら最初の応答 |
| `ctx.reply_ui(&message)` | [Embed やボタン](buttons.md)付きで返信 |
| `ctx.reply_v2(&message)` | [Components V2](components-v2.md) で返信 |
| `ctx.check_cooldown(&cooldown)` | [クールダウン](cooldowns.md)を確認して、待ち時間を伝える |
| `ctx.timing()` | 受信から処理までの時間の測定値 |
| `ctx.http()` | Twilight の HTTP クライアント |

> [!TIP]
> `defer` や `followup` が必要な、時間のかかる処理はスラッシュ専用の関数（`SlashContext`）で書くほうが素直です。両方で動かしたいときは、共通の処理を普通の関数に切り出して、それぞれから呼びます。
