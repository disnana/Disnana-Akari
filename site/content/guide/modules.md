# モジュールで整理する

コマンドが増えてきたら、関連するものを **モジュール** にまとめると見通しがよくなります。discord.py の Cog と同じように、機能ごとのコマンドやイベントをまとめられます。`Module` は同期的な登録をそろえる trait です。共有状態の作成、定期処理の起動、終了後の保存は、アプリの起動・終了処理に書きます。[discord.py の Cog](https://discordpy.readthedocs.io/en/stable/ext/commands/cogs.html)

## 書き方

構造体に必要な状態を持たせ、`register` の中で、いつもの登録メソッドを呼びます。

```rust
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use disnana_akari::{CommandContext, Commands, Events, Module, Registry, Result};

/// 呼ばれた回数を数えるモジュール。
#[derive(Default)]
struct Counter {
    count: Arc<AtomicU64>,
}

impl Module for Counter {
    fn register(&self, registry: &mut Registry<'_>) -> Result<()> {
        // ハンドラーは 'static なので、状態は Arc で共有します
        let count = Arc::clone(&self.count);
        registry.commands.register("count", async move |ctx: CommandContext<'_>| {
            let n = count.fetch_add(1, Ordering::Relaxed) + 1;
            ctx.reply(&format!("{n} 回目です")).await
        })
    }
}

fn setup(counter: &Counter) -> Result<(Commands, Events)> {
    let mut commands = Commands::new("!")?;
    let mut events = Events::new();
    let mut registry = Registry::new(&mut commands, None, &mut events);
    registry.add(counter)?;
    Ok((commands, events))
}
```

`Registry` には、テキストコマンド（`commands`）、スラッシュコマンド（`slash`。使わなければ `None`）、イベント（`events`）の登録先が入っています。フィールドは公開しているので、そのまま渡せます。

```rust
use disnana_akari::{HybridContext, Module, Registry, Result, register_hybrid};

struct Ping;

impl Module for Ping {
    fn register(&self, registry: &mut Registry<'_>) -> Result<()> {
        register_hybrid(
            registry.commands,
            registry.slash.as_deref_mut(),
            "ping",
            "Bot の応答を確認します",
            vec![],
            async |ctx: &mut HybridContext<'_>| -> Result<()> { ctx.reply("Pong!").await },
        )
    }
}
```

## 注意

- 名前が重複すると、`registry.add(..)` がエラーを返します。起動を中止してください（途中まで登録したものは残ります）。
- `Arc::clone` はカウントの中身をコピーせず、同じ状態を使うハンドルを増やします。モジュールとハンドラーで同じ値を読み書きできます。
- 複数のモジュールで状態を共有するときは、アプリが `Arc<状態の型>` を作り、それぞれのモジュールのコンストラクターへ渡します。設定だけなら `Arc<設定の型>`、単純なカウントには `Arc<AtomicU64>`、同時に更新する複数の値には `Arc<Mutex<状態の型>>` が使えます。ロックは必要な値を読み書きしたら解放し、その後で Discord への返信などを `.await` します。

## アプリで起動と終了を管理する

接続の前にリソースを準備し、モジュールを登録します。上の例なら保存済みの値で `Counter` を作り、`setup(&counter)` で登録した一覧と `Arc::clone(&counter.count)` を実行側に渡せます。

終了通知を受けたら `ShutdownHandle` で Bot の停止を要求し、`run_commands` が戻ってから共有状態を保存します。Bot がエラーで終了した場合も保存処理を実行します。この例の `save` には保存先への書き込みを行うアプリの関数を渡します。

```rust
use std::{
    future::Future,
    sync::{Arc, atomic::{AtomicU64, Ordering}},
};

use disnana_akari::{Bot, Commands, Events, Result};
use disnana_akari::model::gateway::Intents;

async fn run_until(
    bot: Bot,
    commands: Commands,
    events: Events,
    counter: Arc<AtomicU64>,
    stop: impl Future<Output = ()>,
    save: impl AsyncFnOnce(u64) -> Result<()>,
) -> Result<()> {
    let (bot, shutdown) = bot
        .add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)
        .with_events(events)
        .with_shutdown();
    let running = bot.run_commands(commands);
    tokio::pin!(running);
    let result = tokio::select! {
        result = &mut running => result,
        () = stop => {
            shutdown.request();
            running.await
        }
    };

    let saved = save(counter.load(Ordering::Relaxed)).await;
    result.and(saved)
}
```

`result.and(saved)` は Bot が失敗した場合にそのエラーを返し、Bot が正常終了した場合に保存の結果を返します。Akari は停止後、受付済みの Bot ハンドラーを制限時間付きで待ちます。別に起動した定期処理が共有状態を更新する場合は、その処理にも停止を通知し、完了を待ってから保存します。[定期処理の停止と再試行](tasks.md)
