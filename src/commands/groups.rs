use std::collections::{HashMap, HashSet};

use super::{CommandContext, CommandHandler, Invocation, duplicate_command};
use crate::{RegistrationError, Result, error::adapt_handler};

/// テキストコマンドをグループにまとめます。
///
/// たとえば `config` の `set` というサブコマンドは `!config set` で実行します。
/// 別名は、それぞれのグループ内で使えます。名前と別名は大文字・小文字を区別します。
pub struct TextCommandGroup<'a> {
    tree: &'a mut CommandTree,
}

impl std::fmt::Debug for TextCommandGroup<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextCommandGroup")
            .field("path", &self.tree.path)
            .field("registered", &self.tree.names().collect::<Vec<_>>())
            .field("has_fallback", &self.tree.fallback.is_some())
            .finish()
    }
}

impl TextCommandGroup<'_> {
    /// このグループにコマンドを登録します。
    ///
    /// # エラー
    ///
    /// 無効な名前や、このグループ内で既に使用中の名前を拒否します。
    pub fn subcommand<E>(
        &mut self,
        name: &str,
        handler: impl for<'a> AsyncFn(CommandContext<'a>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.subcommand_with_aliases(name, &[], handler)
    }

    /// このグループに、別名でも呼び出せるコマンドを登録します。
    ///
    /// # エラー
    ///
    /// 無効な名前やこのグループ内の重複を拒否します。失敗するとコマンドも別名も登録しません。
    pub fn subcommand_with_aliases<E>(
        &mut self,
        name: &str,
        aliases: &[&str],
        handler: impl for<'a> AsyncFn(CommandContext<'a>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let handler = adapt_handler!(handler, ctx: CommandContext<'_>);
        self.tree.register(name, aliases, Box::new(handler))
    }

    /// このグループ内に、さらにグループを作ります。
    ///
    /// # エラー
    ///
    /// 無効な名前や、このグループ内で既に使用中の名前を拒否します。
    pub fn group(&mut self, name: &str) -> Result<TextCommandGroup<'_>> {
        self.group_with_aliases(name, &[])
    }

    /// このグループ内に、別名でも呼び出せるグループを作ります。
    ///
    /// # エラー
    ///
    /// 無効な名前や同じ階層の重複を拒否します。失敗するとグループも別名も登録しません。
    pub fn group_with_aliases(
        &mut self,
        name: &str,
        aliases: &[&str],
    ) -> Result<TextCommandGroup<'_>> {
        self.tree.group(name, aliases)
    }

    /// グループ名まで入力されたときだけ呼ぶ関数を登録します。
    ///
    /// サブコマンド名や追加の文字が続いたときは、それが未登録の名前でも、この関数は呼びません。
    ///
    /// # エラー
    ///
    /// このグループに既に関数を登録済みの場合は失敗します。
    pub fn fallback<E>(
        &mut self,
        handler: impl for<'a> AsyncFn(CommandContext<'a>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.ensure_fallback_available()?;
        let handler = adapt_handler!(handler, ctx: CommandContext<'_>);
        self.tree.fallback = Some(Box::new(handler));
        Ok(())
    }

    pub(crate) fn ensure_names_available(&self, name: &str, aliases: &[&str]) -> Result<()> {
        self.tree.validate_names(name, aliases)
    }

    pub(crate) fn ensure_fallback_available(&self) -> Result<()> {
        if self.tree.fallback.is_some() {
            return Err(duplicate_command(&self.tree.path));
        }
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct CommandTree {
    entries: HashMap<String, CommandEntry>,
    path: Box<str>,
    fallback: Option<Box<dyn CommandHandler>>,
}

enum CommandEntry {
    Command {
        handler: Box<dyn CommandHandler>,
        path: Option<Box<str>>,
    },
    Group(Box<CommandTree>),
    Alias(Box<str>),
}

impl CommandTree {
    pub(super) fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    fn qualify(&self, name: &str) -> Box<str> {
        if self.path.is_empty() {
            name.into()
        } else {
            format!("{} {name}", self.path).into()
        }
    }

    pub(super) fn ensure_available(&self, name: &str) -> Result<()> {
        if name.is_empty() || name.chars().any(char::is_whitespace) {
            return Err(RegistrationError::InvalidCommandName.into());
        }
        if self.entries.contains_key(name) {
            return Err(duplicate_command(&self.qualify(name)));
        }
        Ok(())
    }

    pub(super) fn validate_names(&self, name: &str, aliases: &[&str]) -> Result<()> {
        self.ensure_available(name)?;
        let mut seen = HashSet::with_capacity(aliases.len());
        for &alias in aliases {
            self.ensure_available(alias)?;
            if alias == name || !seen.insert(alias) {
                return Err(duplicate_command(&self.qualify(alias)));
            }
        }
        Ok(())
    }

    pub(super) fn append(&mut self, staged: Self) {
        self.entries.extend(staged.entries);
    }

    fn insert_aliases(&mut self, name: &str, aliases: &[&str]) {
        for &alias in aliases {
            self.entries
                .insert(alias.into(), CommandEntry::Alias(name.into()));
        }
    }

    pub(super) fn register(
        &mut self,
        name: &str,
        aliases: &[&str],
        handler: Box<dyn CommandHandler>,
    ) -> Result<()> {
        self.validate_names(name, aliases)?;
        let path = (!self.path.is_empty()).then(|| self.qualify(name));
        self.entries
            .insert(name.into(), CommandEntry::Command { handler, path });
        self.insert_aliases(name, aliases);
        Ok(())
    }

    pub(super) fn group(&mut self, name: &str, aliases: &[&str]) -> Result<TextCommandGroup<'_>> {
        self.validate_names(name, aliases)?;
        let path = self.qualify(name);
        self.insert_aliases(name, aliases);
        let entry = self.entries.entry(name.into()).or_insert_with(|| {
            CommandEntry::Group(Box::new(Self {
                path,
                ..Self::default()
            }))
        });
        let CommandEntry::Group(tree) = entry else {
            return Err(duplicate_command(name));
        };
        Ok(TextCommandGroup { tree })
    }

    fn find(&self, name: &str) -> Option<(&str, &CommandEntry)> {
        let (canonical, entry) = self.entries.get_key_value(name)?;
        if let CommandEntry::Alias(target) = entry {
            self.entries
                .get(target.as_ref())
                .map(|entry| (target.as_ref(), entry))
        } else {
            Some((canonical.as_str(), entry))
        }
    }

    pub(super) fn resolve<'a>(
        &'a self,
        prefix: &'a str,
        mut input: &'a str,
    ) -> Option<Invocation<'a>> {
        let mut tree = self;
        loop {
            let (invoked_with, args) = input.split_once(char::is_whitespace).unwrap_or((input, ""));
            let (command_name, entry) = tree.find(invoked_with)?;
            let (handler, qualified_name, args) = match entry {
                CommandEntry::Command { handler, path } => (
                    handler.as_ref(),
                    path.as_deref().unwrap_or(command_name),
                    args.trim(),
                ),
                CommandEntry::Group(group) => {
                    let remaining = args.trim_start();
                    if !remaining.is_empty() {
                        tree = group;
                        input = remaining;
                        continue;
                    }
                    (group.fallback.as_deref()?, group.path.as_ref(), "")
                }
                CommandEntry::Alias(_) => return None,
            };
            return Some(Invocation {
                handler,
                command_name,
                qualified_name,
                invoked_with,
                prefix,
                args,
            });
        }
    }
}
