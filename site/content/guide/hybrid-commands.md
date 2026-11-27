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

## 型付きの引数を共有する

`register_typed_hybrid` に渡したオプションの定義を使い、テキストとスラッシュの両方を検証します。例えば `!repeat 3 「Rust の本」` と `/repeat count:3 text:Rust の本` は同じ値を受け取ります。

```rust
use disnana_akari::{
    Commands, HybridArguments, HybridContext, Result, SlashCommands, register_typed_hybrid,
};

fn register(commands: &mut Commands, slash: &mut SlashCommands) -> Result<()> {
    register_typed_hybrid(
        commands, Some(slash), "repeat", "回数と文章を受け取る",
        vec![
            SlashCommands::integer_option("count", "回数", true),
            SlashCommands::string_option("text", "文章", true),
        ],
        repeat,
    )
}

async fn repeat(ctx: &mut HybridContext<'_>, args: HybridArguments<'_>) -> Result<()> {
    let count = args.integer("count").unwrap_or_default();
    let text = args.string("text").unwrap_or_default();
    ctx.reply(&format!("{count} 回: {text}")).await
}
```

テキストでは定義の順に値を入力します。空白を含む値は引用符で囲み、省略可能な引数は末尾から省略します。文字列、整数、小数、真偽値、ユーザー・ロール・チャンネル ID に対応します。ID は数字または種類の合うメンションで指定できます。引数を読み取れない場合は、ハンドラーを実行する前に `Error::HybridArgument` を既存のエラー処理へ渡します。

### 添付とメンションを受け取る

添付は `attachment_option` で登録します。たとえばテキストでは `!inspect` の投稿にファイルを付け、スラッシュでは `/inspect file:` にファイルを指定します。

```rust
use disnana_akari::{
    Commands, HybridArguments, HybridContext, Result, SlashCommands, register_typed_hybrid,
};

fn register(text: &mut Commands, slash: &mut SlashCommands) -> Result<()> {
    register_typed_hybrid(
        text, Some(slash), "inspect", "添付のファイル名を表示します",
        vec![SlashCommands::attachment_option("file", "調べるファイル", true)],
        inspect,
    )
}

async fn inspect(ctx: &mut HybridContext<'_>, args: HybridArguments<'_>) -> Result<()> {
    if let Some(file) = args.attachment("file") {
        ctx.reply(&format!("添付ファイルは {} です。", file.filename)).await?;
    }
    Ok(())
}
```

テキスト側に複数の添付引数がある場合、添付は投稿順に対応します。添付は本文の引数を消費しません。`mentionable_option` は、テキストではユーザーやロールのメンションを受け入れます。`args.mentionable(name)` で `Mentionable::User` または `Mentionable::Role` を受け取ります。メンションの種類を特定できない数字だけの ID はエラーになります。

## グループを両方に登録する

```rust
use disnana_akari::{
    Commands, HybridArguments, HybridContext, Result, SlashCommands, register_hybrid_group,
};

fn register(commands: &mut Commands, slash: &mut SlashCommands) -> Result<()> {
    register_hybrid_group(commands, Some(slash), "config", "設定", |group| {
        group.fallback("show", "設定一覧を表示する", show)?;
        group.group("advanced", "詳細設定")?.subcommand(
            "set", "値を設定する",
            vec![SlashCommands::integer_option("value", "設定値", true)],
            set,
        )
    })
}

async fn show(ctx: &mut HybridContext<'_>, _: HybridArguments<'_>) -> Result<()> {
    ctx.reply("設定一覧").await
}

async fn set(ctx: &mut HybridContext<'_>, args: HybridArguments<'_>) -> Result<()> {
    let value = args.integer("value").unwrap_or_default();
    ctx.reply(&format!("{value} を設定しました。")).await
}
```

`!config advanced set 3` と `/config advanced set value:3` は同じ関数を呼びます。`fallback` は `!config` または `/config show` で呼べます。未知のサブコマンドには一致しません。`register_hybrid_group_with_aliases`、`group_with_aliases`、`subcommand_with_aliases` の別名はテキストだけで使えます。

グループの深さと件数には Discord の制約が適用されます。クロージャや定義の検証がエラーになった場合は、両方の一覧への登録を中止します。実行者の権限が必要なら、共通のハンドラーで `ctx.require_permissions(...).await?` を呼びます。[discord.py の HybridGroup](https://discordpy.readthedocs.io/en/v2.7.1/ext/commands/api.html#discord.ext.commands.HybridGroup)

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
