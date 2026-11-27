use std::fmt::Write as _;

use super::{CommandContext, groups::CommandTree};
use crate::Result;

/// ヘルプに表示するテキストコマンドの説明、引数の書式、表示設定です。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommandMetadata {
    description: Option<String>,
    usage: Option<String>,
    hidden: bool,
}

impl CommandMetadata {
    /// 説明や引数の書式がなく、ヘルプに表示される設定を作ります。
    pub fn new() -> Self {
        Self::default()
    }

    /// ヘルプに出す説明を指定します。
    pub fn description(mut self, value: impl Into<String>) -> Self {
        self.description = Some(value.into());
        self
    }

    /// 正式名の後ろに表示する引数の書式を指定します。実際の引数の解析には影響しません。
    pub fn usage(mut self, value: impl Into<String>) -> Self {
        self.usage = Some(value.into());
        self
    }

    /// 標準のヘルプで、このコマンドと配下のコマンドを隠すか指定します。この設定は実行権限に影響しません。
    pub fn hidden(mut self, value: bool) -> Self {
        self.hidden = value;
        self
    }

    pub(super) const fn is_hidden(&self) -> bool {
        self.hidden
    }
}

/// 登録済みテキストコマンドの正式名、別名、説明を参照します。
#[derive(Clone)]
pub struct CommandInfo<'a> {
    name: &'a str,
    qualified_name: &'a str,
    aliases: Vec<&'a str>,
    metadata: Option<&'a CommandMetadata>,
    children: Option<&'a CommandTree>,
    invokable: bool,
    hidden: bool,
}

impl std::fmt::Debug for CommandInfo<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandInfo")
            .field("qualified_name", &self.qualified_name)
            .field("aliases", &self.aliases)
            .field("metadata", &self.metadata)
            .field("group", &self.is_group())
            .field("invokable", &self.invokable)
            .field("hidden", &self.hidden)
            .finish_non_exhaustive()
    }
}

impl<'a> CommandInfo<'a> {
    pub(super) fn new(
        name: &'a str,
        qualified_name: &'a str,
        mut aliases: Vec<&'a str>,
        metadata: Option<&'a CommandMetadata>,
        children: Option<&'a CommandTree>,
        invokable: bool,
        hidden: bool,
    ) -> Self {
        aliases.sort_unstable();
        Self {
            name,
            qualified_name,
            aliases,
            metadata,
            children,
            invokable,
            hidden,
        }
    }

    /// 親グループを含まない正式名です。
    pub fn name(&self) -> &'a str {
        self.name
    }

    /// 正式名を最上位から空白でつないだ名前です。
    pub fn qualified_name(&self) -> &'a str {
        self.qualified_name
    }

    /// この階層で使える別名を名前順に返します。
    pub fn aliases(&self) -> &[&'a str] {
        &self.aliases
    }

    /// ヘルプ用に設定された説明です。
    pub fn description(&self) -> Option<&'a str> {
        self.metadata
            .and_then(|metadata| metadata.description.as_deref())
    }

    /// ヘルプ用に設定された引数の書式です。
    pub fn usage(&self) -> Option<&'a str> {
        self.metadata.and_then(|metadata| metadata.usage.as_deref())
    }

    /// このコマンド、または親グループが非表示に設定されているかを返します。
    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    /// サブコマンドを追加できるグループかを返します。
    pub fn is_group(&self) -> bool {
        self.children.is_some()
    }

    /// 正式名まで入力したときに実行できるかを返します。グループでは関数がある場合だけ実行できます。
    pub fn is_invokable(&self) -> bool {
        self.invokable
    }

    /// 直下のコマンドを正式名の順で返します。設定済みの非表示コマンドも取得できます。
    pub fn children(&self) -> Vec<Self> {
        self.children
            .map_or_else(Vec::new, |tree| tree.infos(self.hidden))
    }
}

pub(super) async fn reply_help(ctx: CommandContext<'_>) -> Result<()> {
    let content = format_help(ctx.commands, ctx.prefix(), ctx.args());
    ctx.reply(&content).await
}

fn format_help(commands: &super::Commands, prefix: &str, query: &str) -> String {
    let mut message = String::new();
    let visible = if query.trim().is_empty() {
        message.push_str("使えるコマンド\n");
        commands.command_infos()
    } else if let Some(info) = commands
        .command_info(query)
        .filter(|info| !info.is_hidden())
    {
        write_usage(&mut message, prefix, &info);
        if let Some(description) = info.description() {
            let _ = write!(message, "\n{description}");
        }
        if !info.aliases().is_empty() {
            let _ = write!(message, "\n別名 {}", info.aliases().join("、"));
        }
        let children = info.children();
        if children.iter().any(|child| !child.is_hidden()) {
            message.push_str("\n\nサブコマンド\n");
        }
        children
    } else {
        return "指定されたコマンドが見つかりません。".into();
    };
    for info in visible.iter().filter(|info| !info.is_hidden()) {
        write_usage(&mut message, prefix, info);
        if let Some(summary) = info
            .description()
            .and_then(|description| description.lines().next())
        {
            let _ = write!(message, " — {summary}");
        }
        message.push('\n');
    }
    limit_message(message)
}

fn write_usage(message: &mut String, prefix: &str, info: &CommandInfo<'_>) {
    let _ = write!(message, "{prefix}{}", info.qualified_name());
    if let Some(usage) = info.usage() {
        let _ = write!(message, " {usage}");
    }
    if !info.is_invokable() {
        message.push_str("（グループ）");
    }
}

fn limit_message(mut message: String) -> String {
    const LIMIT: usize = 2000;
    const SUFFIX: &str = "\n…";
    if message.encode_utf16().take(LIMIT + 1).count() > LIMIT {
        let max_content = LIMIT - SUFFIX.encode_utf16().count();
        let mut length = 0;
        if let Some((index, _)) = message.char_indices().find(|(_, ch)| {
            length += ch.len_utf16();
            length > max_content
        }) {
            message.truncate(index);
            message.push_str(SUFFIX);
        }
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Commands, Error, RegistrationError};

    async fn command(_: CommandContext<'_>) -> Result<()> {
        Ok(())
    }

    #[test]
    fn metadata_discovers_canonical_paths_aliases_and_group_fallbacks() -> Result<()> {
        let mut commands = Commands::new("!")?;
        commands.register("zeta", command)?;
        let mut config = commands.group_with_aliases("config", &["cfg"])?;
        let mut advanced = config.group_with_aliases("advanced", &["a"])?;
        advanced.fallback(command)?;
        advanced.subcommand_with_aliases("set", &["s", "put"], command)?;
        advanced.set_child_metadata(
            "set",
            CommandMetadata::new()
                .description("設定を更新")
                .usage("<値>"),
        )?;

        let names: Vec<_> = commands
            .command_infos()
            .iter()
            .map(CommandInfo::name)
            .collect();
        assert_eq!(names, ["config", "zeta"]);
        assert!(!commands.command_info("cfg").unwrap().is_invokable());
        let advanced = commands.command_info("cfg a").unwrap();
        assert!(advanced.is_invokable());
        assert!(advanced.is_group());
        let leaf = commands.command_info(" cfg\ta  s ").unwrap();
        assert!(!leaf.is_group());
        assert_eq!(leaf.qualified_name(), "config advanced set");
        assert_eq!(leaf.aliases(), ["put", "s"]);
        assert_eq!(leaf.description(), Some("設定を更新"));
        assert_eq!(leaf.usage(), Some("<値>"));
        assert!(
            commands
                .command_info("config advanced set unexpected")
                .is_none()
        );
        assert!(matches!(
            commands.set_metadata("cfg a s", CommandMetadata::new()),
            Err(Error::Registration(
                RegistrationError::UnknownCommand { .. }
            ))
        ));
        Ok(())
    }

    #[test]
    fn help_is_opt_in_and_keeps_hidden_subtrees_out_of_alias_queries() -> Result<()> {
        let mut commands = Commands::with_prefixes(&["!", "akari."])?;
        assert!(!commands.matches_message("!help"));
        let mut cfg = commands.group_with_aliases("config", &["cfg"])?;
        cfg.subcommand("set", command)?;
        cfg.set_child_metadata("set", CommandMetadata::new().usage("<値>"))?;
        cfg.group_with_aliases("admin", &["a"])?
            .subcommand("erase", command)?;
        commands.set_metadata("config admin", CommandMetadata::new().hidden(true))?;
        assert!(commands.command_info("cfg a erase").unwrap().is_hidden());
        assert!(commands.command_info("cfg a").unwrap().children()[0].is_hidden());
        assert!(commands.matches_message("!cfg a erase"));

        commands.register_help("help")?;
        assert!(commands.matches_message("!help cfg"));
        assert!(commands.register_help("help").is_err());
        assert!(format_help(&commands, "akari.", "").contains("akari.help [コマンド名]"));
        let visible = format_help(&commands, "akari.", "cfg");
        assert!(visible.contains("akari.config set <値>"));
        assert!(visible.contains("別名 cfg"));
        assert!(!visible.contains("admin"));
        let missing = format_help(&commands, "akari.", "missing");
        assert_eq!(format_help(&commands, "akari.", "cfg a erase"), missing);
        Ok(())
    }

    #[test]
    fn default_format_fits_a_discord_message_without_splitting_unicode() -> Result<()> {
        let mut commands = Commands::new("!")?;
        commands.register("about", command)?;
        commands.set_metadata(
            "about",
            CommandMetadata::new().description("🦀".repeat(2000)),
        )?;
        let message = format_help(&commands, "!", "about");
        assert!(message.starts_with("!about\n🦀"));
        assert!(message.ends_with("\n…"));
        assert!(message.encode_utf16().count() <= 2000);
        Ok(())
    }
}
