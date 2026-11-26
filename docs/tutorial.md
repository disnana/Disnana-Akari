# Playground のコードを読む

[クイックスタート](quickstart.md) で Bot を動かしたら、RustRover で `playground/src/main.rs` を開きます。コピー元のサンプルにも日本語のコメントがあります。[サンプルの入口](../examples/ping/main.rs)

| ファイル | 担当 |
| --- | --- |
| `main.rs` | 設定を読み、コマンドとイベントを登録して Bot を起動します。 |
| `config.rs` | `.env` から Bot トークンとテスト用サーバー ID を読みます。 |
| `commands.rs` | `!ping`、`!echo`、`/ping`、`/echo` を個別の関数に結び付け、`!latency` と `/latency` は同じ関数につなぎます。 |
| `events.rs` | Gateway が準備できたときや、メッセージを受け取ったときの処理を定義します。 |

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

`!echo こんにちは` を送ると、`ctx.args()` がコマンド名より後ろの `こんにちは` を返します。値は `&str` で、元の本文の一部を借りています。ハンドラーの終了後まで文字列が必要なら `to_owned()` で所有する `String` に変換できます。

## `/echo` が引数を受け取る仕組み

`/echo` の `text` 欄は、登録時に `SlashCommands::string_option("text", "返す文章", true)` で作ります。テキストコマンドでは本文の分割を Akari が行い、スラッシュコマンドでは入力欄の値を Discord が名前付きで渡します。

`ctx.string("text")` は `Option<&str>` を返します。値があれば `Some(text)`、なければ `None` です。`&mut SlashContext<'_>` はコンテキストを変更できる形で借りています。Akari はこの借用を通じて初回応答が済んだことを記録します。[コマンドの実装](../examples/ping/commands.rs)

## `!latency` と `/latency` で同じ関数を使う

`register_hybrid` にテキストとスラッシュの登録先を渡すと、一つの `async fn` を両方で呼び出せます。`Some(&mut slash)` は「スラッシュの登録先がある」、`None` は「テキストだけを登録する」という意味です。Playground では `DISCORD_GUILD_ID` の有無に応じて渡す値が変わります。

共通の関数は `&mut HybridContext<'_>` を受け取ります。`ctx.timing()` で測定値を読み取り、表示用の文字列に整形してから `ctx.reply(&response).await` で呼び出し元に返信します。`&response` は、表示用の文字列を返信関数へ一時的に貸しています。

Playground の `latency` では、`ctx.source()` を `match` で分けて、`!latency` には 3 秒、`/latency` には 1 秒のクールダウンを適用します。`ctx.author_id()` が返すユーザー ID を `cooldown.check(user)` に渡すと、まだ実行できない場合に `Err(remaining)` が返ります。直後にもう片方の形式を試しても、待ち時間は共有されません。

`Cooldown::new(...)` は登録時に一度だけ実行します。`async move |ctx| ...` は、登録した関数の中に作成済みのクールダウンを持たせる書き方です。`latency` 関数は `&Cooldown` でそれを借りるため、前回の記録を次回も使えます。[コマンドの実装](../examples/ping/commands.rs) · [API リファレンス](api.md#cooldown)

## イベントに処理を追加する

`events.on_ready(ready)` に登録した関数は、Discord とのセッションが準備できると呼ばれます。投稿を観察する関数は `events.on_message(handler)` に登録できます。テキストコマンドの返信処理と同じファイルに置く必要はありません。[イベントの実装](../examples/ping/events.rs) · [API リファレンス](api.md)
