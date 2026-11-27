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

## 利用場所と翻訳を設定する

登録済みのコマンドに `set_command_settings` で設定を追加します。グローバルコマンドでは、インストール先と利用場所を選べます。翻訳はギルドコマンドにも設定できます。

```rust
use disnana_akari::{CommandSettings, Result, SlashCommands, SlashContext};
use disnana_akari::model::{
    application::{command::CommandType, interaction::InteractionContextType},
    oauth::ApplicationIntegrationType,
};

fn register(slash: &mut SlashCommands) -> Result<()> {
    slash.register("ping", "Check the response", vec![], ping)?;
    slash.set_command_settings(
        CommandType::ChatInput,
        "ping",
        CommandSettings::new()
            .contexts([InteractionContextType::Guild, InteractionContextType::PrivateChannel])
            .install_types([ApplicationIntegrationType::UserInstall])
            .name_localization("ja", "ピング")
            .description_localization("ja", "応答を確認する"),
    )
}

async fn ping(ctx: &mut SlashContext<'_>) -> Result<()> {
    let text = if ctx.locale() == Some("ja") { "応答しました。" } else { "Pong!" };
    ctx.reply_ephemeral(text).await
}
```

この例には `SlashCommands::global()` を使い、Developer Portal でもユーザーインストールを有効にします。ユーザー・メッセージのメニューはそれぞれ `CommandType::User`、`CommandType::Message` で指定できます。設定に渡す名前は登録時の名前です。[Discord の利用場所とインストール先](https://docs.discord.com/developers/interactions/application-commands#contexts)

## コンテキストメニューを登録する

Discord でユーザーやメッセージのメニューを開き、アプリの一覧に出るコマンドを登録できます。スラッシュコマンドと同じ `SlashCommands` に登録し、同じ手順で同期します。

```rust
use disnana_akari::{Result, SlashCommands, SlashContext, UserMenuTarget};
use disnana_akari::model::channel::Message;

fn register(slash: &mut SlashCommands) -> Result<()> {
    slash.register_user_menu("ユーザーの ID", user_id)?;
    slash.register_message_menu("メッセージの ID", message_id)
}

async fn user_id(ctx: &mut SlashContext<'_>, target: UserMenuTarget<'_>) -> Result<()> {
    ctx.reply_ephemeral(&target.user.id.to_string()).await
}

async fn message_id(ctx: &mut SlashContext<'_>, message: &Message) -> Result<()> {
    ctx.reply_ephemeral(&message.id.to_string()).await
}
```

実行したユーザーは `ctx.author_id()`、選ばれたユーザーは `target.user` です。Discord が選択対象のサーバー内情報を送った場合は `target.member` で取得できます。Discord が送った選択対象を使うため、受け取り時に追加の REST リクエストはありません。

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

## 入力中の候補を出す（オートコンプリート）

引数を入力している途中に、候補を表示できます。コマンドを登録したあとに `SlashCommands::autocomplete` で、コマンド名（サブコマンドなら経路）と引数名を指定して関数を付けます。Akari が、その引数の `autocomplete` を自動で有効にします。

```rust
use disnana_akari::{AutocompleteContext, Result, SlashCommands, SlashContext};

fn register(slash: &mut SlashCommands) -> Result<()> {
    let color = SlashCommands::string_option("color", "好きな色", true);
    slash.register("paint", "色を選びます", vec![color], paint)?;
    slash.autocomplete(&["paint"], "color", suggest_color)
}

async fn suggest_color(ctx: &mut AutocompleteContext<'_>) -> Result<()> {
    let typed = ctx.focused().to_lowercase();
    let choices = ["red", "green", "blue"]
        .into_iter()
        .filter(|name| name.contains(&typed))
        .map(|name| SlashCommands::string_choice(name, name))
        .collect();
    ctx.respond(choices).await
}

async fn paint(ctx: &mut SlashContext<'_>) -> Result<()> {
    let color = ctx.string("color").unwrap_or("red");
    ctx.reply(color).await
}
```

- `ctx.focused()` は、いま入力している途中の文字列です。`ctx.string(name)` で、同じコマンドの入力済みの文字列の引数も読めます。
- 候補は最大 25 件で、`ctx.respond` は一度だけ呼べます。候補は `string_choice`、`integer_choice`、`number_choice` で作ります。
- 付けられるのは、文字列、整数、数値の引数で、選択肢（`choices`）を持たないものだけです。
- 関数が候補を返さずに終わったときは、入力欄が読み込み中のままにならないよう、空の候補を返します。エラーは `Bot::on_error` に報告します。

## サブコマンドにまとめる

`/config set`、`/config advanced reset` のように、コマンドをまとめるには `SlashCommands::group` を使います。返された `CommandGroup` に `subcommand` を登録します。グループの中にグループ（`group`）も作れますが、Discord の制約で最大 3 段（コマンド → グループ → サブコマンド）までです。

```rust
use disnana_akari::{Result, SlashCommands, SlashContext};

fn register(slash: &mut SlashCommands) -> Result<()> {
    let mut config = slash.group("config", "設定を変更します")?;
    let value = SlashCommands::string_option("value", "設定する値", true);
    config.subcommand("set", "値を設定します", vec![value], set)?;

    let mut advanced = config.group("advanced", "詳細な設定")?;
    advanced.subcommand("reset", "初期化します", vec![], reset)?;
    Ok(())
}

async fn set(ctx: &mut SlashContext<'_>) -> Result<()> {
    // サブコマンドの引数は、そのまま ctx.string などで受け取れます
    let value = ctx.string("value").unwrap_or_default();
    ctx.reply(&format!("{value} に設定しました")).await
}

async fn reset(ctx: &mut SlashContext<'_>) -> Result<()> {
    ctx.reply("初期化しました").await
}
```

- サブコマンドを持つコマンド自体（`/config` だけ）は、Discord の制約で実行できません。
- 同じ名前の普通のコマンドと、グループは両立しません。
- `ctx.command_path()` は、`["config", "advanced", "reset"]` のように呼ばれた経路を返します。`Bot::on_error` に渡る名前も `config advanced reset` の形です。
- `default_member_permissions("config", ..)` は、グループ全体にかかります。

### サブコマンドや引数を翻訳する

`CommandOptionLocalizations` を使うと、引数とサブコマンドにロケールごとの名前や説明を設定できます。固定の選択肢は `SlashCommands::localize_choice` で表示名を翻訳します。

```rust
use disnana_akari::{CommandOptionLocalizations, Result, SlashCommands, SlashContext};

async fn find(ctx: &mut SlashContext<'_>) -> Result<()> {
    let query = ctx.string("keyword").unwrap_or_default();
    ctx.reply(&format!("検索語句は {query} です。")).await
}

fn localized_group(slash: &mut SlashCommands) -> Result<()> {
    let keyword = CommandOptionLocalizations::new()
        .name_localization("ja", "キーワード")
        .description_localization("ja", "調べたい語句")
        .apply(SlashCommands::string_option("keyword", "Search term", true))?;

    let mut group = slash.group("tools", "Utilities")?;
    group.subcommand("find", "Find a record", vec![keyword], find)?;
    group.set_child_localizations(
        "find",
        CommandOptionLocalizations::new()
            .name_localization("ja", "検索")
            .description_localization("ja", "記録を検索"),
    )?;
    Ok(())
}
```

翻訳は Discord 上の表示に反映されます。登録、ハンドラー、引数の取得では `find` と `keyword` のような元の名前を使います。同じ階層で元の名前や同じロケールの表示名が重複する場合は、登録時にエラーになります。[Discord のコマンド翻訳仕様](https://docs.discord.com/developers/interactions/application-commands#localization)

## 呼び出した人やチャンネルを調べる

| メソッド | 返す値 |
| --- | --- |
| `ctx.author_id()` | 呼び出したユーザーの ID |
| `ctx.channel_id()` | コマンドが実行されたチャンネルの ID |
| `ctx.option(name)` | 引数（Twilight の値。サブコマンドの引数も同じ名前で取れます） |
| `ctx.command_path()` | コマンド名とサブコマンド名の経路（`["config", "set"]`） |
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

設定したサーバーに登録された古いコマンドが実行された場合は、`Bot::on_error` に報告し、本人にだけ「このコマンドは現在利用できません。」と返します（`Messages::slash_command_unavailable`）。グローバルの場合、Akari が同期した一覧に ID がないコマンドは処理対象から外れます。

## コマンド全体の文字数

1コマンドの名前、説明、サブコマンド、引数、選択肢を合わせて最大8,000文字です。翻訳がある項目は、既定値を含めた最長の文字列を数えます。1項目ずつ制限内でも、合計が超えると同期前にエラーになります。選択肢は1つの引数につき25件までで、オートコンプリートとは併用できません。[Discord のスラッシュコマンド仕様](https://docs.discord.com/developers/interactions/application-commands#slash-commands)

## 実行前後の処理を共通化する

`SlashCommands` の実行フックは、権限チェックと必要な引数の準備に通った後に呼ばれます。スラッシュコマンドとユーザー・メッセージのメニューに共通です。`set_before_invoke` のエラーは実行を中止します。`set_after_invoke` には実行結果が渡り、ハンドラーとフックが両方失敗した場合はハンドラーのエラーを優先します。

```rust
use disnana_akari::{Error, Result, SlashCommands, SlashContext};

fn install_hooks(commands: &mut SlashCommands) {
    commands.set_after_invoke(async |ctx: &SlashContext<'_>, error: Option<&Error>| -> Result<()> {
        eprintln!("実行完了 {:?} 成功={}", ctx.command_path(), error.is_none());
        Ok(())
    });
}
```

自動補完に実行フックは適用されません。ハイブリッドでは `!` 用の `Commands` とスラッシュ用の `SlashCommands` にそれぞれ設定します。
