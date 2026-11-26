# Playground のコードを読む

[クイックスタート](quickstart.md) で Bot を動かしたら、RustRover で `playground/src/main.rs` を開きます。コピー元のサンプルにも日本語のコメントがあります。[サンプルの入口](../examples/ping/main.rs)

| ファイル | 担当 |
| --- | --- |
| `main.rs` | 設定を読み、コマンドとイベントを登録して Bot を起動します。 |
| `config.rs` | `.env` から Bot トークン、テスト用サーバー ID、登録一覧を一括置換するかどうかを読みます。 |
| `commands.rs` | `!ping` と `/ping` を個別の関数に結び付けます。`echo` と `latency` は、それぞれテキストとスラッシュで関数を共有します。 |
| `events.rs` | Gateway が準備できたときや、メッセージを受け取ったときの処理を定義します。 |
| `ui.rs` | `/ui` の埋め込み、ボタン、クリック時の処理を定義します。 |

## `!ping` の実行を追う

```rust
use disnana_akari::{CommandContext, Commands, Result};

fn register() -> Result<Commands> {
    let mut commands = Commands::new("!")?;
    commands.register("ping", ping)?;
    Ok(commands)
}

async fn ping(ctx: CommandContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}
```

- `use` はライブラリの型を短い名前で使えるようにします。別ファイルの処理は `mod commands;` のように宣言します。
- `let mut` は変更できる変数を作ります。`register` が一覧にコマンドを追加するため、この変数には `mut` が必要です。
- `Commands::new("!")` は `Commands` 型の関数を呼び出します。末尾の `?` は、失敗した場合にこの関数もそこで終了してエラーを返すという意味です。
- `commands.register("ping", ping)` の二つ目の `ping` は関数そのものです。Discord で `!ping` を受信すると、Akari がこの関数を呼びます。
- `async fn` は非同期関数です。`.await` は、この関数の実行を一時停止して Discord への返信処理の完了を待ちます。
- `CommandContext<'_>` は、Akari が持つメッセージの情報を一時的に借ります。`'_` は借用の有効な範囲をコンパイラーに推論させる記号です。
- `Result<()>` の `()` は「成功時に返す値がない」という意味です。関数の最後の式をセミコロンなしで書くと、その式の値が返ります。`ping` は返信処理の成功かエラーをそのまま返します。

## `echo` の引数をテキストとスラッシュで共通化する

`SlashCommands::string_option("text", "返す文章", true)` で `text` という必須の文字列引数を定義します。ハンドラーでは、どちらの形式でも `ctx.text_arg("text")` で読み出せます。

```rust
use disnana_akari::{HybridContext, Result};

async fn echo(ctx: &mut HybridContext<'_>) -> Result<()> {
    let Some(text) = ctx.text_arg("text") else {
        return ctx.reply("返したい文章を入力してください。").await;
    };
    ctx.reply(text).await
}
```

`!echo こんにちは 世界` では、コマンド名に続く `こんにちは 世界` が最初の文字列引数になります。`/echo` では、Discord の `text` 欄に入れた値を受け取ります。`let Some(text) = ... else` は、入力があるときに `text` という名前で取り出す書き方です。入力が空なら `else` に進みます。`&str` は元の入力を借りた文字列です。ハンドラーの終了後まで必要なら `to_owned()` で所有する `String` に変換できます。[コマンドの実装](../examples/ping/commands.rs)

## `!latency` と `/latency` で同じ関数を使う

`register_hybrid` にテキストとスラッシュの登録先を渡すと、一つの `async fn` を両方で呼び出せます。`Some(&mut slash)` は「スラッシュの登録先がある」、`None` は「テキストだけを登録する」という意味です。Playground では `DISCORD_GUILD_ID` の有無に応じて渡す値が変わります。

共通の関数は `&mut HybridContext<'_>` を受け取ります。`ctx.timing()` で測定値を読み取り、表示用の文字列に整形してから `ctx.reply(&response).await` で呼び出し元に返信します。`&response` は、表示用の文字列を返信関数へ一時的に貸しています。

Playground の `latency` では、`ctx.source()` を `match` で分けて、`!latency` には 3 秒、`/latency` には 1 秒のクールダウンを適用します。`ctx.check_cooldown(cooldown).await?` は、まだ実行できない場合に残り時間を知らせて `false` を返します。`if !... { return Ok(()); }` で、その場合に返信を終えて関数から戻れます。スラッシュでは残り時間が本人にだけ表示されます。直後にもう片方の形式を試しても、待ち時間は共有されません。

`Cooldown::new(...)` は登録時に一度だけ実行します。`async move |ctx| ...` は、登録した関数の中に作成済みのクールダウンを持たせる書き方です。`latency` 関数は `&Cooldown` でそれを借りるため、前回の記録を次回も使えます。[コマンドの実装](../examples/ping/commands.rs) · [API リファレンス](api.md#cooldown)

## 時間のかかるスラッシュコマンドに先に応答する

スラッシュ専用のハンドラーには `&mut SlashContext<'_>` が渡ります。Discord の 3 秒の応答期限に間に合わない可能性がある処理では、ハンドラーの最初で `defer_ephemeral` を呼びます。呼び出したユーザーだけに処理中の表示が出ます。調べ物が終わったら `reply` で表示を置き換え、別のメッセージも送りたい場合は `followup_ephemeral` を呼びます。

```rust
use disnana_akari::{Result, SlashContext};

async fn report(ctx: &mut SlashContext<'_>) -> Result<()> {
    ctx.defer_ephemeral().await?;
    // 実装時は、ここでデータ取得や集計を待ちます。
    ctx.reply("集計結果が出ました。").await?;
    ctx.followup_ephemeral("条件を変えると再集計できます。").await
}
```

先に `?` で応答の成否を確認すると、初回応答が失敗したときに集計を続けずに終了できます。`reply` を呼ぶまで Discord 側には処理中の表示が残ります。[API リファレンス](api.md)

## イベントに処理を追加する

`events.on_ready(ready)` に登録した関数は、Discord とのセッションが準備できると呼ばれます。投稿を観察する関数は `events.on_message(handler)` に登録できます。テキストコマンドの返信処理と同じファイルに置く必要はありません。[イベントの実装](../examples/ping/events.rs) · [API リファレンス](api.md)

## 埋め込みとボタンを付けて返信する

`/ui` は `UiMessage::new()` で返信を組み立て、`embed(...)` で説明を載せる枠、`buttons(...)` でボタンの行を追加します。`field` の最後の `true` は、Discord の表示幅が十分なときにほかの項目と横並びにできるという指定です。

```rust
use disnana_akari::{ButtonContext, Events, Result, SlashContext};
use disnana_akari::ui::{Button, Embed, UiMessage};

const CONFIRM: &str = "akari:example:confirm";

async fn show(ctx: &mut SlashContext<'_>) -> Result<()> {
    let message = UiMessage::new()
        .embed(
            Embed::new("確認")
                .description("下のボタンを押してください。")
                .color(0x5865F2)
                .field("操作", "結果は本人にだけ表示されます。", true),
        )
        .buttons(vec![Button::primary(CONFIRM, "確認する")]);
    ctx.reply_ui(&message).await
}

fn register(events: &mut Events) -> Result<()> {
    events.on_button(CONFIRM, confirm)
}

async fn confirm(ctx: &mut ButtonContext<'_>) -> Result<()> {
    ctx.reply_ephemeral("確認しました。").await
}
```

`Button::primary` の最初の値は、クリック時に Discord から送られてくる識別子 `custom_id` です。`on_button` に同じ値を渡すと、ボタンを押したときに登録した関数が呼ばれます。`const` で一箇所に定義しておくと、表示側と登録側の値を揃えられます。`reply_ephemeral` を使うと、クリックへの返信は本人にだけ見えます。[Playground の UI 実装](../examples/ping/ui.rs) · [API リファレンス](api.md)
