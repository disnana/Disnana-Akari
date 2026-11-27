use std::collections::{HashMap, HashSet};

use super::{
    CommandContext, CommandHandler, CommandInfo, CommandMetadata, Invocation, duplicate_command,
};
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
    /// このグループ直下の登録済みコマンドやグループのヘルプ設定を置き換えます。
    ///
    /// # エラー
    /// 正式名の子が登録されていない場合に失敗します。
    pub fn set_child_metadata(&mut self, name: &str, metadata: CommandMetadata) -> Result<()> {
        self.tree.set_local_metadata(name, metadata).ok_or_else(|| {
            RegistrationError::UnknownCommand {
                name: self.tree.qualify(name).into(),
            }
            .into()
        })
    }

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
    metadata: HashMap<String, CommandMetadata>,
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
        self.metadata.extend(staged.metadata);
    }

    pub(super) fn set_metadata(&mut self, path: &str, metadata: CommandMetadata) -> Option<()> {
        let mut names = path.split_whitespace().peekable();
        let mut tree = self;
        while let Some(name) = names.next() {
            if names.peek().is_none() {
                return tree.set_local_metadata(name, metadata);
            }
            let CommandEntry::Group(group) = tree.entries.get_mut(name)? else {
                return None;
            };
            tree = group;
        }
        None
    }

    fn set_local_metadata(&mut self, name: &str, metadata: CommandMetadata) -> Option<()> {
        if matches!(self.entries.get(name)?, CommandEntry::Alias(_)) {
            return None;
        }
        self.metadata.insert(name.into(), metadata);
        Some(())
    }

    pub(super) fn info(&self, query: &str) -> Option<CommandInfo<'_>> {
        let mut tree = self;
        let mut names = query.split_whitespace().peekable();
        let mut hidden = false;
        while let Some(name) = names.next() {
            let (canonical, entry) = tree.find(name)?;
            if names.peek().is_none() {
                let aliases = tree
                    .entries
                    .iter()
                    .filter_map(|(alias, candidate)| match candidate {
                        CommandEntry::Alias(target) if target.as_ref() == canonical => {
                            Some(alias.as_str())
                        }
                        _ => None,
                    })
                    .collect();
                return tree.entry_info(canonical, entry, aliases, hidden);
            }
            let CommandEntry::Group(group) = entry else {
                return None;
            };
            hidden |= tree
                .metadata
                .get(canonical)
                .is_some_and(CommandMetadata::is_hidden);
            tree = group;
        }
        None
    }

    pub(super) fn infos(&self, hidden: bool) -> Vec<CommandInfo<'_>> {
        let mut aliases: HashMap<&str, Vec<&str>> = HashMap::new();
        for (name, entry) in &self.entries {
            if let CommandEntry::Alias(target) = entry {
                aliases.entry(target).or_default().push(name);
            }
        }
        let mut infos = self
            .entries
            .iter()
            .filter_map(|(name, entry)| {
                self.entry_info(
                    name,
                    entry,
                    aliases.remove(name.as_str()).unwrap_or_default(),
                    hidden,
                )
            })
            .collect::<Vec<_>>();
        infos.sort_unstable_by_key(CommandInfo::name);
        infos
    }

    fn entry_info<'a>(
        &'a self,
        name: &'a str,
        entry: &'a CommandEntry,
        aliases: Vec<&'a str>,
        parent_hidden: bool,
    ) -> Option<CommandInfo<'a>> {
        let (path, children, invokable) = match entry {
            CommandEntry::Command { path, .. } => (path.as_deref().unwrap_or(name), None, true),
            CommandEntry::Group(tree) => (
                tree.path.as_ref(),
                Some(tree.as_ref()),
                tree.fallback.is_some(),
            ),
            CommandEntry::Alias(_) => return None,
        };
        let metadata = self.metadata.get(name);
        let hidden = parent_hidden || metadata.is_some_and(CommandMetadata::is_hidden);
        Some(CommandInfo::new(
            name, path, aliases, metadata, children, invokable, hidden,
        ))
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
