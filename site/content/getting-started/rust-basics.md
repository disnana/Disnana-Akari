# Rust が初めての人へ

Akari のコードを読み書きするために知っておくと楽になる Rust の要点を、Python（discord.py）との違いに触れながら説明します。Rust 自体を学ぶには、公式の [The Rust Programming Language](https://doc.rust-lang.org/book/)（日本語版あり）がおすすめです。

## `async fn` と `.await`

Discord への返信のように「待つ」処理は `async fn` で書き、待つところに `.await` を付けます。Python の `async def` と `await` と同じ考え方です。

```rust
use disnana_akari::{CommandContext, Result};

async fn ping(ctx: CommandContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}
```

`.await` を付け忘れると、返信は送られません（コンパイラーが警告してくれます）。

## `Result` と `?`

Rust には例外がありません。失敗するかもしれない処理は `Result` を返し、`?` を付けると「失敗ならこの関数もそこでエラーを返して終わる」という意味になります。

```rust
use disnana_akari::{Commands, Result};

fn build() -> Result<Commands> {
    let mut commands = Commands::new("!")?; // 失敗したら、ここで終了
    Ok(commands)                            // 成功したら Ok で包んで返す
}
```

Akari の `Result<()>` の `()` は「成功しても返す値がない」という意味です。ハンドラーのエラーは `Bot::on_error` で受け取れます。ログとして表示する場合は `tracing` の購読先を設定します。[ログを出す](../guide/logging.md)

## `&str` と `String`

- `&str` … 文字列を「借りて」読むだけの型です。`ctx.args()` などが返します。
- `String` … 自分で持つ文字列です。`format!("...")` や `.to_owned()` で作ります。

関数の外へ持ち出したいときは `.to_owned()` で `String` にします。返信に使うだけなら、そのまま渡せます。

```rust
use disnana_akari::{CommandContext, Result};

async fn echo(ctx: CommandContext<'_>) -> Result<()> {
    let text = ctx.args();      // &str: 入力を借りている
    if text.is_empty() {
        return ctx.reply("使い方: `!echo 文章`").await;
    }
    ctx.reply(text).await
}
```

## `Option` と `let ... else`

「値があるかもしれないし、ないかもしれない」は `Option` で表します。`Some(値)` か `None` です。Python の `None` と違い、`None` の可能性を無視するとコンパイルが通りません。

`let ... else` は、値があるときだけ先へ進み、なければ `else` の中で抜ける書き方です。

```rust
use disnana_akari::{HybridContext, Result};

async fn echo(ctx: &mut HybridContext<'_>) -> Result<()> {
    let Some(text) = ctx.text_arg("text") else {
        return ctx.reply("文章を入力してください。").await;
    };
    ctx.reply(text).await
}
```

## `&mut` と `'_`

- `&mut` は「変更できる形で借りる」という意味です。`SlashContext` など、返信済みかどうかを記録するコンテキストは `&mut` で受け取ります。
- `CommandContext<'_>` の `'_` は、借りているデータがどのくらい生きるかをコンパイラーに推論させる印です。**そのまま書けば大丈夫** です。

## 関数を渡す

関数名に `()` を付けずに書くと、関数そのものを渡せます。

```rust
use disnana_akari::{CommandContext, Commands, Result};

async fn ping(ctx: CommandContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}

fn register(commands: &mut Commands) -> Result<()> {
    commands.register("ping", ping) // ping() ではなく ping
}
```

状態を持たせたいときは、`async move |ctx| ...` のように書いた名前のない関数（クロージャ）も渡せます。

## つまずいたときの読み方

Rust のエラーメッセージは長いですが、先頭の `error[E0xxx]: ...` の 1 行と、`^^^` で示された箇所を読めば、多くの場合は原因が分かります。`cargo check` を使うと、実行せずに素早く確認できます。

次は [discord.py との対応表](from-discordpy.md) か、さっそく [テキストコマンド](../guide/text-commands.md) へどうぞ。
