use std::fmt::Write as _;

use super::{CommandContext, Commands, groups::CommandTree};
use crate::{
    CommandCheckContext, Result, command_check::run_checks, validation::MESSAGE_CONTENT_MAX,
};

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

pub(super) async fn reply_help(ctx: CommandContext<'_>, verify_checks: bool) -> Result<()> {
    let mut view = HelpView::for_query(ctx.commands, ctx.args());
    if verify_checks && !ctx.commands.checks.is_empty() {
        view = view.checked(&ctx).await;
    }
    let content = view.render(ctx.prefix());
    for page in pages(&content).filter(|page| !page.trim().is_empty()) {
        ctx.reply(page).await?;
    }
    Ok(())
}

enum HelpView<'a> {
    Index(Vec<CommandInfo<'a>>),
    Detail(CommandInfo<'a>, Vec<CommandInfo<'a>>),
    NotFound,
}

impl<'a> HelpView<'a> {
    fn for_query(commands: &'a Commands, query: &str) -> Self {
        if query.trim().is_empty() {
            let mut commands = commands.command_infos();
            commands.retain(|command| !command.is_hidden());
            return Self::Index(commands);
        }
        let Some(command) = commands
            .command_info(query)
            .filter(|info| !info.is_hidden())
        else {
            return Self::NotFound;
        };
        let mut children = command.children();
        children.retain(|child| !child.is_hidden());
        Self::Detail(command, children)
    }

    async fn checked(self, ctx: &CommandContext<'_>) -> Self {
        match self {
            Self::Index(commands) => Self::Index(check_entries(commands, ctx).await),
            Self::Detail(command, children) => {
                if can_show(&command, ctx).await {
                    Self::Detail(command, check_entries(children, ctx).await)
                } else {
                    Self::NotFound
                }
            }
            Self::NotFound => Self::NotFound,
        }
    }

    fn render(self, prefix: &str) -> String {
        let mut message = String::new();
        let entries = match self {
            Self::Index(entries) => {
                message.push_str("使えるコマンド\n");
                entries
            }
            Self::Detail(command, children) => {
                write_detail(&mut message, prefix, &command);
                if !children.is_empty() {
                    message.push_str("\n\nサブコマンド\n");
                }
                children
            }
            Self::NotFound => return "指定されたコマンドが見つかりません。".into(),
        };
        for info in entries {
            write_usage(&mut message, prefix, &info);
            if let Some(summary) = info
                .description()
                .and_then(|description| description.lines().next())
            {
                let _ = write!(message, " — {summary}");
            }
            message.push('\n');
        }
        message
    }
}

async fn check_entries<'a>(
    entries: Vec<CommandInfo<'a>>,
    ctx: &CommandContext<'_>,
) -> Vec<CommandInfo<'a>> {
    let mut allowed = Vec::with_capacity(entries.len());
    for entry in entries {
        if can_show(&entry, ctx).await {
            allowed.push(entry);
        }
    }
    allowed
}

async fn can_show(entry: &CommandInfo<'_>, ctx: &CommandContext<'_>) -> bool {
    let name = entry.qualified_name();
    // ヘルプ自身は、ハンドラーの呼び出し前に確認済み。
    name == ctx.qualified_name()
        || run_checks(
            &ctx.commands.checks,
            CommandCheckContext::text(ctx.context(), name),
        )
        .await
        .is_ok()
}

fn write_detail(message: &mut String, prefix: &str, info: &CommandInfo<'_>) {
    write_usage(message, prefix, info);
    if let Some(description) = info.description() {
        let _ = write!(message, "\n{description}");
    }
    if !info.aliases().is_empty() {
        let _ = write!(message, "\n別名 {}", info.aliases().join("、"));
    }
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

fn pages(mut remaining: &str) -> impl Iterator<Item = &str> {
    std::iter::from_fn(move || {
        if remaining.is_empty() {
            return None;
        }
        let (page, rest) = remaining.split_at(page_end(remaining));
        remaining = rest;
        Some(page)
    })
}

fn page_end(content: &str) -> usize {
    let mut length = 0;
    let mut line_end = None;
    for (index, ch) in content.char_indices() {
        length += ch.len_utf16();
        if length > MESSAGE_CONTENT_MAX {
            return line_end.unwrap_or(index);
        }
        if ch == '\n' {
            line_end = Some(index + 1);
        }
    }
    content.len()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::{
        CheckFailure, CommandCheck, Commands, Context, Error, Needs, RegistrationError,
        access::tests::Mock, commands::tests::command_message, permissions::PermissionResolver,
        timing::Pending,
    };
    use twilight_model::guild::Permissions;

    async fn command(_: CommandContext<'_>) -> Result<()> {
        Ok(())
    }

    fn format_help(commands: &Commands, prefix: &str, query: &str) -> String {
        HelpView::for_query(commands, query).render(prefix)
    }

    async fn dispatch_help(commands: &Commands, input: &str) -> Result<String> {
        let server = Mock::start(vec![("/channels/2/messages".into(), "{}".into())]).await;
        let http = server.client();
        let permissions = PermissionResolver::new(None);
        let message = command_message(input)?;
        let (_, timing) = Pending::new((), None).begin();
        commands
            .dispatch(Context {
                http: &http,
                message: &message,
                timing,
                permissions: &permissions,
            })
            .await?;
        assert_eq!(server.count("POST /channels/2/messages"), 1);
        Ok(
            server.body_of("POST /channels/2/messages").unwrap()["content"]
                .as_str()
                .unwrap()
                .to_owned(),
        )
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

    #[tokio::test]
    async fn checked_help_filters_the_index_and_details_without_executing_candidates() -> Result<()>
    {
        let mut commands = Commands::new("!")?;
        commands.register_help("help")?;
        commands.register_help_with_checks("visible")?;
        commands.register("staff", command)?;
        let mut group = commands.group_with_aliases("config", &["cfg"])?;
        group.subcommand_with_aliases("lock", &["l"], command)?;
        group.set_child_metadata("lock", CommandMetadata::new().description("管理用"))?;
        group.subcommand("shadow", command)?;
        group.set_child_metadata("shadow", CommandMetadata::new().hidden(true))?;

        let trace = Arc::new(Mutex::new(Vec::new()));
        let parsed = Arc::clone(&trace);
        let invoked = Arc::clone(&trace);
        group.subcommand_parsed(
            "show",
            async move |_: &CommandContext<'_>| -> Result<()> {
                parsed.lock().unwrap().push("parser".into());
                Ok(())
            },
            async move |_: CommandContext<'_>, ()| -> Result<()> {
                invoked.lock().unwrap().push("handler".into());
                Ok(())
            },
        )?;
        let checks = Arc::clone(&trace);
        commands.add_check(CommandCheck::new(
            async move |ctx: CommandCheckContext<'_>| {
                let name = ctx.command_path().collect::<Vec<_>>().join(" ");
                assert_eq!(ctx.author_id().map(|id| id.get()), Some(3));
                assert_eq!(ctx.channel_id().map(|id| id.get()), Some(2));
                let allowed = name != "staff";
                checks.lock().unwrap().push(name);
                if allowed {
                    Ok(())
                } else {
                    Err(CheckFailure::Rejected.into())
                }
            },
        ));
        commands.require("cfg l", Needs::new().author(Permissions::MANAGE_ROLES))?;
        let before = Arc::clone(&trace);
        commands.set_before_invoke(async move |ctx: &CommandContext<'_>| {
            before
                .lock()
                .unwrap()
                .push(format!("before {}", ctx.qualified_name()));
            Ok(())
        });
        let after = Arc::clone(&trace);
        commands.set_after_invoke(async move |ctx: &CommandContext<'_>, _: Option<&Error>| {
            after
                .lock()
                .unwrap()
                .push(format!("after {}", ctx.qualified_name()));
            Ok(())
        });
        let take_trace = || std::mem::take(&mut *trace.lock().unwrap());

        let index = dispatch_help(&commands, "!visible").await?;
        assert!(index.contains("!config") && !index.contains("!staff"));
        assert_eq!(
            take_trace(),
            [
                "visible",
                "before visible",
                "config",
                "help",
                "staff",
                "after visible"
            ]
        );

        let detail = dispatch_help(&commands, "!visible cfg").await?;
        assert!(detail.contains("!config show"));
        assert!(!detail.contains("!config lock") && !detail.contains("!config shadow"));
        assert_eq!(
            take_trace(),
            [
                "visible",
                "before visible",
                "config",
                "config lock",
                "config show",
                "after visible"
            ]
        );

        let denied = dispatch_help(&commands, "!visible cfg l").await?;
        assert_eq!(
            take_trace(),
            ["visible", "before visible", "config lock", "after visible"]
        );
        let missing = dispatch_help(&commands, "!visible missing").await?;
        assert_eq!(denied, missing);
        assert_eq!(take_trace(), ["visible", "before visible", "after visible"]);
        let normal = dispatch_help(&commands, "!help cfg l").await?;
        assert!(normal.contains("!config lock\n管理用"));
        assert_eq!(take_trace(), ["help", "before help", "after help"]);
        Ok(())
    }

    #[test]
    fn help_pages_preserve_an_oversized_unicode_line() -> Result<()> {
        let mut commands = Commands::new("!")?;
        commands.register("about", command)?;
        let description = format!("{}🦀\n次の行", "a".repeat(MESSAGE_CONTENT_MAX - 1));
        commands.set_metadata("about", CommandMetadata::new().description(&description))?;
        let message = format_help(&commands, "!", "about");
        let pages: Vec<_> = pages(&message).collect();
        assert_eq!(message, format!("!about\n{description}"));
        assert_eq!(pages.concat(), message);
        assert_eq!(pages.first(), Some(&"!about\n"));
        assert_eq!(pages.last(), Some(&"🦀\n次の行"));
        assert!(
            pages
                .iter()
                .all(|page| page.encode_utf16().count() <= MESSAGE_CONTENT_MAX)
        );
        Ok(())
    }

    #[tokio::test]
    async fn help_dispatch_sends_all_pages_without_losing_visible_commands() -> Result<()> {
        let mut commands = Commands::new("!")?;
        commands.register_help("help")?;
        let summary = "🦀".repeat(20);
        for index in 0..80 {
            let name = format!("cmd{index:02}");
            commands.register(&name, command)?;
            commands.set_metadata(&name, CommandMetadata::new().description(&summary))?;
        }
        commands.register("secret", command)?;
        commands.set_metadata("secret", CommandMetadata::new().hidden(true))?;

        let content = format_help(&commands, "!", "");
        let pages: Vec<_> = pages(&content).collect();
        assert!(pages.len() > 1);
        assert_eq!(
            pages
                .iter()
                .flat_map(|page| page.lines())
                .filter_map(|line| line.split_once(" — "))
                .filter(|(command, _)| command.starts_with("!cmd"))
                .map(|(command, _)| command)
                .collect::<Vec<_>>(),
            (0..80)
                .map(|index| format!("!cmd{index:02}"))
                .collect::<Vec<_>>()
        );
        assert!(pages.iter().all(|page| {
            page.encode_utf16().count() <= MESSAGE_CONTENT_MAX && !page.contains("!secret")
        }));

        let server = Mock::start(vec![("/channels/2/messages".into(), "{}".into())]).await;
        let http = server.client();
        let permissions = PermissionResolver::new(None);
        let message = command_message("!help")?;
        let (_, timing) = Pending::new((), None).begin();
        commands
            .dispatch(Context {
                http: &http,
                message: &message,
                timing,
                permissions: &permissions,
            })
            .await?;
        assert_eq!(server.count("POST /channels/2/messages"), pages.len());
        let first = server.body_of("POST /channels/2/messages").unwrap();
        assert_eq!(first["content"].as_str(), pages.first().copied());
        assert_eq!(first["allowed_mentions"]["parse"], serde_json::json!([]));
        Ok(())
    }
}
