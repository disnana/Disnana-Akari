# クールダウン

同じ人が、短い間隔でコマンドを連打できないようにします。

## ハイブリッドコマンドで使う

`HybridContext::check_cooldown` が、確認と通知までまとめて行います。使えない間は、残り時間を返信して `false` を返します。

```rust
use std::time::Duration;

use disnana_akari::{Commands, Cooldown, HybridContext, Result, SlashCommands, register_hybrid};

fn register(commands: &mut Commands, slash: &mut SlashCommands) -> Result<()> {
    // 5 秒に 1 回。登録時に 1 度だけ作る
    let cooldown = Cooldown::new(Duration::from_secs(5));

    register_hybrid(
        commands,
        Some(slash),
        "roll",
        "サイコロを振ります",
        vec![],
        async move |ctx| roll(ctx, &cooldown).await,
    )
}

async fn roll(ctx: &mut HybridContext<'_>, cooldown: &Cooldown) -> Result<()> {
    if !ctx.check_cooldown(cooldown).await? {
        return Ok(()); // 待ち時間を返信済み。ここで終わる
    }
    ctx.reply("🎲 4 が出ました！").await
}
```

- スラッシュコマンドでは、待ち時間の通知は本人にだけ表示されます。
- `Cooldown` は **登録時に 1 度だけ** 作ります。実行のたびに作ると、前回の記録を忘れてしまいます。`async move` のクロージャに入れて持たせます。

## テキストコマンドだけで使う

`Cooldown::check(user_id)` を直接呼びます。`Ok(())` なら実行を記録し、`Err(残り時間)` なら待ち時間が残っています。

```rust
use std::time::Duration;

use disnana_akari::{CommandContext, Commands, Cooldown, Result};

fn register(commands: &mut Commands) -> Result<()> {
    let cooldown = Cooldown::new(Duration::from_secs(10));

    commands.register("daily", async move |ctx: CommandContext<'_>| {
        let user = ctx.context().message().author.id;
        if let Err(wait) = cooldown.check(user) {
            return ctx.reply(&format!("あと {} 秒待ってください。", wait.as_secs() + 1)).await;
        }
        ctx.reply("今日の分を受け取りました！").await
    })
}
```

## 知っておくこと

- 拒否された呼び出しは、待ち時間を **延ばしません**。
- 制限はユーザーごとです。ほかの人には影響しません。
- 状態は Bot のプロセスの中にだけあります。再起動すると消えます。
- コマンドごと、呼び出し方（テキスト／スラッシュ）ごとに別の制限にしたいときは、別の `Cooldown` を作ります。同じ `Cooldown` を使うと、待ち時間も共有されます。
- `Duration::ZERO` を渡すと、常に通します。
