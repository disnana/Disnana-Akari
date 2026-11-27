use crate::{Commands, Events, Result, SlashCommands};

/// 関連するコマンドやイベントを、ひとまとまりで登録する単位です。
///
/// discord.py の Cog にあたりますが、継承やマクロは使いません。構造体に必要な状態を持たせ、
/// [`register`](Self::register) で、普通の登録メソッドを呼ぶだけです。ハンドラーは `'static` なので、
/// 状態は `Arc` などで共有します。
///
/// ```
/// use std::sync::{
///     Arc,
///     atomic::{AtomicU64, Ordering},
/// };
///
/// use disnana_akari::{CommandContext, Commands, Events, Module, Registry, Result};
///
/// /// 呼ばれた回数を数えるモジュール。
/// #[derive(Default)]
/// struct Counter {
///     count: Arc<AtomicU64>,
/// }
///
/// impl Module for Counter {
///     fn register(&self, registry: &mut Registry<'_>) -> Result<()> {
///         let count = Arc::clone(&self.count);
///         registry.commands.register("count", async move |ctx: CommandContext<'_>| {
///             let n = count.fetch_add(1, Ordering::Relaxed) + 1;
///             ctx.reply(&format!("{n} 回目です")).await
///         })
///     }
/// }
///
/// # fn main() -> Result<()> {
/// let mut commands = Commands::new("!")?;
/// let mut events = Events::new();
/// let mut registry = Registry::new(&mut commands, None, &mut events);
/// registry.add(&Counter::default())?;
/// # Ok(())
/// # }
/// ```
pub trait Module {
    /// このモジュールのコマンドやイベントを、登録先に追加します。
    ///
    /// # エラー
    ///
    /// 名前の重複など、登録に失敗した場合はエラーを返します。
    fn register(&self, registry: &mut Registry<'_>) -> Result<()>;
}

/// [`Module`] が登録する先の、テキストコマンド、スラッシュコマンド、イベントの一覧です。
///
/// フィールドは公開しているので、`register_hybrid(registry.commands, registry.slash.as_deref_mut(), ..)`
/// のように、そのまま渡せます。
#[derive(Debug)]
#[non_exhaustive]
pub struct Registry<'a> {
    /// テキストコマンドの登録先です。
    pub commands: &'a mut Commands,
    /// スラッシュコマンドの登録先です。使わない Bot では `None` です。
    pub slash: Option<&'a mut SlashCommands>,
    /// イベントの登録先です。
    pub events: &'a mut Events,
}

impl<'a> Registry<'a> {
    /// 登録先をまとめます。
    pub fn new(
        commands: &'a mut Commands,
        slash: Option<&'a mut SlashCommands>,
        events: &'a mut Events,
    ) -> Self {
        Self {
            commands,
            slash,
            events,
        }
    }

    /// モジュールを登録します。
    ///
    /// # エラー
    ///
    /// モジュールの登録が失敗した場合、そのエラーを返します。
    /// 失敗する前に登録したものは残るので、起動を中止してください。
    pub fn add<M: Module + ?Sized>(&mut self, module: &M) -> Result<()> {
        module.register(self)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use twilight_model::id::Id;

    use super::*;
    use crate::{CommandContext, SlashContext, register_hybrid};

    /// コマンドとスラッシュコマンドと状態を持つ、テスト用のモジュール。
    struct Greeter {
        calls: Arc<AtomicUsize>,
    }

    impl Module for Greeter {
        fn register(&self, registry: &mut Registry<'_>) -> Result<()> {
            let calls = Arc::clone(&self.calls);
            registry.commands.register(
                "hello",
                async move |_: CommandContext<'_>| -> Result<()> {
                    calls.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                },
            )?;
            if let Some(slash) = registry.slash.as_deref_mut() {
                slash.register(
                    "hello",
                    "あいさつ",
                    vec![],
                    async |_: &mut SlashContext<'_>| -> Result<()> { Ok(()) },
                )?;
            }
            registry.events.on_ready(async |_| -> Result<()> { Ok(()) });
            Ok(())
        }
    }

    #[test]
    fn modules_register_into_every_target_and_duplicates_fail() -> Result<()> {
        let module = Greeter {
            calls: Arc::new(AtomicUsize::new(0)),
        };
        let mut commands = Commands::new("!")?;
        let mut slash = SlashCommands::for_guild(Id::new(1));
        let mut events = Events::new();
        let mut registry = Registry::new(&mut commands, Some(&mut slash), &mut events);

        registry.add(&module)?;
        assert!(registry.add(&module).is_err());
        assert!(commands.matches_message("!hello"));
        Ok(())
    }

    #[test]
    fn slash_is_optional() -> Result<()> {
        let mut commands = Commands::new("!")?;
        let mut events = Events::new();
        let module = Greeter {
            calls: Arc::new(AtomicUsize::new(0)),
        };
        Registry::new(&mut commands, None, &mut events).add(&module)?;
        assert!(commands.matches_message("!hello"));
        Ok(())
    }

    #[test]
    fn registry_fields_work_with_register_hybrid() -> Result<()> {
        struct Hybrid;
        impl Module for Hybrid {
            fn register(&self, registry: &mut Registry<'_>) -> Result<()> {
                register_hybrid(
                    registry.commands,
                    registry.slash.as_deref_mut(),
                    "ping",
                    "応答",
                    vec![],
                    async |_: &mut crate::HybridContext<'_>| -> Result<()> { Ok(()) },
                )
            }
        }
        let mut commands = Commands::new("!")?;
        let mut slash = SlashCommands::for_guild(Id::new(1));
        let mut events = Events::new();
        Registry::new(&mut commands, Some(&mut slash), &mut events).add(&Hybrid)?;
        assert!(commands.matches_message("!ping"));
        Ok(())
    }
}
