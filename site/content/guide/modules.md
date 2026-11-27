# モジュールで整理する

コマンドが増えてきたら、関連するものを **モジュール** にまとめると見通しがよくなります。discord.py の Cog にあたりますが、継承もマクロもありません。`Module` は **登録のしかたをそろえるだけ** の trait で、Akari が勝手に何かをすることはありません。

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

fn setup() -> Result<(Commands, Events)> {
    let mut commands = Commands::new("!")?;
    let mut events = Events::new();
    let mut registry = Registry::new(&mut commands, None, &mut events);
    registry.add(&Counter::default())?;
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
- モジュール同士で状態を共有したいときは、`Arc` を作って、それぞれのモジュールに渡します。Akari は、共有の入れ物（型を消した状態置き場）を用意しません。何を共有しているかが、型で見えるようにするためです。
