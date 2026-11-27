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

## チャンネルごとに複数回を許可する

`Cooldown::per_key` を使うと、最初に通過してから決めた時間が過ぎるまでに、同じキーで何回実行できるかを指定できます。例えばメッセージの投稿をチャンネルごとに 10 秒で 3 回までに絞れます。

```rust
use std::{num::NonZeroU32, time::Duration};
use disnana_akari::{CommandContext, Commands, Cooldown, Result};
use disnana_akari::model::id::{Id, marker::ChannelMarker};

fn register(commands: &mut Commands) -> Result<()> {
    let rate = NonZeroU32::new(3).unwrap();
    let cooldown = Cooldown::<Id<ChannelMarker>>::per_key(rate, Duration::from_secs(10));
    commands.register("post", async move |ctx: CommandContext<'_>| -> Result<()> {
        let channel = ctx.context().message().channel_id;
        if let Err(wait) = cooldown.check(channel) {
            return ctx.reply(&format!("{:.1} 秒待ってください。", wait.as_secs_f64())).await;
        }
        ctx.reply("受け付けました。").await
    })
}
```

複数の ID を組にすると「チャンネルごと、かつユーザーごと」に制限できます。全体で共有したい場合はキーに `()` を使います。

## 同時に実行できる件数を制限する

時間のかかるコマンドを同じ人が重ねて呼ぶのを抑えるには、`ConcurrencyLimit` を使います。処理が終わるまで枠を保持し、終わった直後から次の呼び出しを受け付けます。

```rust
use std::num::NonZeroU16;
use disnana_akari::{Commands, CommandContext, ConcurrencyLimit, Result};

fn register(commands: &mut Commands) -> Result<()> {
    let limit = ConcurrencyLimit::new(NonZeroU16::MIN);
    commands.register("report", async move |ctx: CommandContext<'_>| -> Result<()> {
        let user = ctx.context().message().author.id;
        let Some(_permit) = limit.try_acquire(user) else {
            return ctx.reply("前の処理が終わってから送ってください。").await;
        };
        ctx.reply("集計が完了しました。").await
    })
}
```

枠が空くまで待たせる場合は `acquire(key).await` を使います。スラッシュコマンドでは、待ち始める前に `defer` で初回応答を送ってください。同じ制限をハイブリッドハンドラーに渡せば、両方の呼び出しで枠を共有できます。

## 知っておくこと

- 拒否された呼び出しは、待ち時間を **延ばしません**。
- 制限はユーザーごとです。ほかの人には影響しません。
- 状態は Bot のプロセスの中にだけあります。再起動すると消えます。
- コマンドごと、呼び出し方（テキスト／スラッシュ）ごとに別の制限にしたいときは、別の `Cooldown` を作ります。同じ `Cooldown` を使うと、待ち時間も共有されます。
- `Duration::ZERO` を渡すと、常に通します。
