use std::{
    collections::{BTreeMap, HashMap, btree_map::Entry, hash_map::Entry as HashEntry},
    future::Future,
    pin::Pin,
    time::Instant,
};

use twilight_http::Client;
use twilight_model::{
    application::{
        command::{
            Command, CommandOption, CommandOptionChoice, CommandOptionChoiceValue,
            CommandOptionType, CommandType,
        },
        interaction::{
            Interaction, InteractionData, InteractionType,
            application_command::{CommandData, CommandDataOption, CommandOptionValue},
        },
    },
    channel::message::{AllowedMentions, MessageFlags},
    guild::Permissions,
    http::interaction::{InteractionResponse, InteractionResponseData, InteractionResponseType},
    id::{
        Id,
        marker::{ChannelMarker, CommandMarker, GuildMarker, UserMarker},
    },
};
use twilight_validate::command;

use crate::{
    CheckFailure, CommandSyncError, Error, ErrorSource, HandlerKind, InfoEvent, RegistrationError,
    ResponseStateError, Result, Timing,
    autocomplete::{AutocompleteContext, AutocompleteHandler, choice},
    error::adapt_handler,
    hooks::{Hooks, InfoReporter},
    permissions::check_permissions,
    ui::{ComponentsV2, Modal, UiMessage},
};

/// 一つのサーバー、またはアプリ全体に登録するスラッシュコマンド一覧です。
pub struct SlashCommands {
    scope: Scope,
    entries: BTreeMap<String, Definition>,
    replace: bool,
}

/// コマンドを登録する範囲です。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scope {
    Guild(Id<GuildMarker>),
    Global,
}

impl Scope {
    fn guild_id(self) -> Option<Id<GuildMarker>> {
        match self {
            Self::Guild(guild_id) => Some(guild_id),
            Self::Global => None,
        }
    }
}

impl std::fmt::Debug for SlashCommands {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlashCommands")
            .field("scope", &self.scope)
            .field("registered", &self.entries.keys().collect::<Vec<_>>())
            .field("replace", &self.replace)
            .finish()
    }
}

impl SlashCommands {
    /// 指定したサーバー向けに空の一覧を作ります。
    pub fn for_guild(guild_id: Id<GuildMarker>) -> Self {
        Self::new(Scope::Guild(guild_id))
    }

    /// アプリ全体（すべてのサーバーと DM）向けに空の一覧を作ります。
    ///
    /// グローバルコマンドは、Discord 側で反映されるまでに時間がかかることがあります。
    /// 開発中は [`for_guild`](Self::for_guild) のほうが、すぐに反映されて便利です。
    /// 同じ Bot に、グローバルとサーバー限定を同時には登録できません。
    pub fn global() -> Self {
        Self::new(Scope::Global)
    }

    fn new(scope: Scope) -> Self {
        Self {
            scope,
            entries: BTreeMap::new(),
            replace: false,
        }
    }

    /// 起動時に、このアプリの同じ範囲（サーバー、またはグローバル）のコマンド一覧を、
    /// 登録済みの定義で置き換えます。
    ///
    /// 一覧に含まれないスラッシュコマンドや、ユーザー・メッセージのコマンドも削除されます。
    /// 空の一覧を指定した場合は、その範囲のコマンドがすべて削除されます。
    pub fn replace_commands(mut self) -> Self {
        self.replace = true;
        self
    }

    /// Discord に表示するコマンド名、説明、引数と、呼び出す関数を設定します。
    ///
    /// # エラー
    ///
    /// Discord の制約に合わない定義や、重複した名前を拒否します。
    pub fn register<E>(
        &mut self,
        name: &str,
        description: &str,
        options: Vec<CommandOption>,
        handler: impl AsyncFn(&mut SlashContext<'_>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.insert(
            name,
            description,
            options,
            Some(Box::new(
                adapt_handler!(handler, ctx: &mut SlashContext<'_>),
            )),
            PermissionPolicy::Unrestricted,
        )?;
        Ok(())
    }

    fn insert(
        &mut self,
        name: &str,
        description: &str,
        options: Vec<CommandOption>,
        handler: Option<Box<dyn SlashHandler>>,
        permissions: PermissionPolicy,
    ) -> Result<&mut Definition> {
        command::chat_input_name(name)?;
        command::description(description)?;
        command::options(&options)?;
        match self.entries.entry(name.into()) {
            Entry::Vacant(entry) => Ok(entry.insert(Definition {
                description: description.into(),
                options,
                permissions,
                handlers: RouteHandlers {
                    command: handler,
                    autocompletes: HashMap::new(),
                },
                children: BTreeMap::new(),
            })),
            Entry::Occupied(_) => Err(RegistrationError::Duplicate {
                kind: HandlerKind::SlashCommand,
                name: name.to_owned(),
            }
            .into()),
        }
    }

    /// 引数に、入力中の候補を返す関数（オートコンプリート）を付けます。
    ///
    /// `path` は、コマンド名とサブコマンド名の経路です（`&["echo"]`、`&["config", "set"]`）。
    /// 先にコマンドを [`register`](Self::register) または [`CommandGroup::subcommand`] で登録してください。
    /// 文字列、整数、数値の引数で、選択肢（`choices`）を持たないものだけに付けられます。
    /// 引数の定義には、Akari が自動で `autocomplete` を有効にします。
    ///
    /// # エラー
    ///
    /// コマンドや引数が見つからない場合、付けられない種類の引数の場合、すでに付けている場合に失敗します。
    pub fn autocomplete<E>(
        &mut self,
        path: &[&str],
        option: &str,
        handler: impl AsyncFn(&mut AutocompleteContext<'_>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let unknown = || RegistrationError::UnknownCommand {
            name: path.join(" "),
        };
        let (root, rest) = path.split_first().ok_or_else(unknown)?;
        let definition = self.entries.get_mut(*root).ok_or_else(unknown)?;
        let (options, handlers) = match (rest, &mut definition.children) {
            ([], children) if children.is_empty() => {
                (&mut definition.options, &mut definition.handlers)
            }
            ([sub], children) => match children.get_mut(*sub) {
                Some(Child::Sub(sub)) => (&mut sub.options, &mut sub.handlers),
                _ => return Err(unknown().into()),
            },
            ([group, sub], children) => match children.get_mut(*group) {
                Some(Child::Group { subs, .. }) => match subs.get_mut(*sub) {
                    Some(sub) => (&mut sub.options, &mut sub.handlers),
                    None => return Err(unknown().into()),
                },
                _ => return Err(unknown().into()),
            },
            _ => return Err(unknown().into()),
        };
        let target = options
            .iter_mut()
            .find(|candidate| candidate.name == option)
            .ok_or_else(|| RegistrationError::UnknownOption {
                command: path.join(" "),
                option: option.to_owned(),
            })?;
        if !matches!(
            target.kind,
            CommandOptionType::String | CommandOptionType::Integer | CommandOptionType::Number
        ) || target.choices.is_some()
        {
            return Err(RegistrationError::AutocompleteUnsupported {
                option: option.to_owned(),
            }
            .into());
        }
        match handlers.autocompletes.entry(option.to_owned()) {
            HashEntry::Vacant(entry) => {
                target.autocomplete = Some(true);
                entry.insert(Box::new(
                    adapt_handler!(handler, ctx: &mut AutocompleteContext<'_>),
                ));
                Ok(())
            }
            HashEntry::Occupied(_) => Err(RegistrationError::Duplicate {
                kind: HandlerKind::Autocomplete,
                name: format!("{} {option}", path.join(" ")),
            }
            .into()),
        }
    }

    /// オートコンプリートで返す、文字列の候補を作ります。
    pub fn string_choice(name: &str, value: &str) -> CommandOptionChoice {
        choice(name, CommandOptionChoiceValue::String(value.into()))
    }

    /// オートコンプリートで返す、整数の候補を作ります。
    pub fn integer_choice(name: &str, value: i64) -> CommandOptionChoice {
        choice(name, CommandOptionChoiceValue::Integer(value))
    }

    /// オートコンプリートで返す、数値の候補を作ります。
    pub fn number_choice(name: &str, value: f64) -> CommandOptionChoice {
        choice(name, CommandOptionChoiceValue::Number(value))
    }

    /// サブコマンドをまとめるコマンドを作ります。`/config set` の `config` にあたります。
    ///
    /// 返された [`CommandGroup`] に [`subcommand`](CommandGroup::subcommand) を登録します。
    /// 同じ名前の普通のコマンドがあるとき、またはグループがすでにあるときは失敗します。
    /// Discord の制約で、サブコマンドを持つコマンド自体は、実行できません。
    ///
    /// # エラー
    ///
    /// 名前や説明が Discord の制約に合わない場合と、重複した名前を拒否します。
    pub fn group(&mut self, name: &str, description: &str) -> Result<CommandGroup<'_>> {
        let root = self.insert(
            name,
            description,
            Vec::new(),
            None,
            PermissionPolicy::Unrestricted,
        )?;
        Ok(CommandGroup { root, group: None })
    }

    /// Discord 上の既定の表示と、実行時の両方で権限を確認するギルドコマンドを登録します。
    /// `Permissions::ADMINISTRATOR` または空の権限集合を渡すと管理者専用になります。
    /// 実行時にはサーバー内であることも確認するため、DM からは使えません。
    ///
    /// # エラー
    ///
    /// グローバル登録、Discord の制約に合わない定義、重複した名前を拒否します。
    pub fn register_with_permissions<E>(
        &mut self,
        name: &str,
        description: &str,
        options: Vec<CommandOption>,
        required: Permissions,
        handler: impl AsyncFn(&mut SlashContext<'_>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let permissions = self.required_permissions(required)?;
        self.insert(
            name,
            description,
            options,
            Some(Box::new(
                adapt_handler!(handler, ctx: &mut SlashContext<'_>),
            )),
            permissions,
        )?;
        Ok(())
    }

    /// 権限の確認が必要なギルドコマンドのグループを登録します。
    /// 最上位の表示条件と、配下のすべてのサブコマンド・補完に適用します。
    /// 権限の指定とエラーは [`Self::register_with_permissions`] と同じです。
    pub fn group_with_permissions(
        &mut self,
        name: &str,
        description: &str,
        required: Permissions,
    ) -> Result<CommandGroup<'_>> {
        let permissions = self.required_permissions(required)?;
        let root = self.insert(name, description, Vec::new(), None, permissions)?;
        Ok(CommandGroup { root, group: None })
    }

    fn required_permissions(&self, required: Permissions) -> Result<PermissionPolicy> {
        if matches!(self.scope, Scope::Global) {
            return Err(RegistrationError::RestrictedCommandRequiresGuild.into());
        }
        Ok(PermissionPolicy::Required(if required.is_empty() {
            Permissions::ADMINISTRATOR
        } else {
            required
        }))
    }

    /// 登録済みのコマンドを、既定で使える人の権限を絞って Discord に登録します。
    ///
    /// 指定した権限を持たない人には、コマンドが Discord の画面に出ません。ただし、サーバーの管理者は
    /// この既定を変えられるため、安全のための確認は、ハンドラーで
    /// [`SlashContext::require_permissions`] も呼んでください。
    /// [`register`](Self::register) のあとに呼びます。
    ///
    /// # エラー
    ///
    /// 登録されていない名前、または実行時の権限確認も設定済みの名前を指定した場合に失敗します。
    pub fn default_member_permissions(
        &mut self,
        name: &str,
        permissions: Permissions,
    ) -> Result<()> {
        match self.entries.get_mut(name) {
            Some(definition) if matches!(definition.permissions, PermissionPolicy::Required(_)) => {
                Err(RegistrationError::CommandPermissionsAlreadyRequired {
                    name: name.to_owned(),
                }
                .into())
            }
            Some(definition) => {
                definition.permissions = PermissionPolicy::Visibility(permissions);
                Ok(())
            }
            None => Err(RegistrationError::UnknownCommand {
                name: name.to_owned(),
            }
            .into()),
        }
    }

    /// 文字列の引数を作ります。必要なら返された Twilight の定義で長さなどを設定できます。
    pub fn string_option(name: &str, description: &str, required: bool) -> CommandOption {
        Self::typed_option(CommandOptionType::String, name, description, required)
    }

    /// 整数の引数を作ります。範囲は、返された定義の `min_value` と `max_value` で設定できます。
    /// 値は [`SlashContext::integer`] で受け取ります。
    pub fn integer_option(name: &str, description: &str, required: bool) -> CommandOption {
        Self::typed_option(CommandOptionType::Integer, name, description, required)
    }

    /// 小数を含む数値の引数を作ります。値は [`SlashContext::number`] で受け取ります。
    pub fn number_option(name: &str, description: &str, required: bool) -> CommandOption {
        Self::typed_option(CommandOptionType::Number, name, description, required)
    }

    /// 真偽値の引数を作ります。値は [`SlashContext::boolean`] で受け取ります。
    pub fn boolean_option(name: &str, description: &str, required: bool) -> CommandOption {
        Self::typed_option(CommandOptionType::Boolean, name, description, required)
    }

    fn typed_option(
        kind: CommandOptionType,
        name: &str,
        description: &str,
        required: bool,
    ) -> CommandOption {
        CommandOption {
            autocomplete: None,
            channel_types: None,
            choices: None,
            description: description.into(),
            description_localizations: None,
            kind,
            max_length: None,
            max_value: None,
            min_length: None,
            min_value: None,
            name: name.into(),
            name_localizations: None,
            options: None,
            file_types: None,
            required: Some(required),
        }
    }

    pub(crate) async fn sync(
        self,
        http: &Client,
        info: &InfoReporter,
    ) -> Result<ActiveSlashCommands> {
        if self.entries.is_empty() && !self.replace {
            return self.bind(Vec::new());
        }

        let application = http.current_user_application().await?.model().await?;
        let client = http.interaction(application.id);
        let registered = if self.replace {
            let commands = self.replacement_commands()?;
            match self.scope {
                Scope::Guild(guild_id) => {
                    client
                        .set_guild_commands(guild_id, &commands)
                        .await?
                        .models()
                        .await?
                }
                Scope::Global => {
                    client
                        .set_global_commands(&commands)
                        .await?
                        .models()
                        .await?
                }
            }
        } else {
            let mut registered = Vec::with_capacity(self.entries.len());
            for (name, definition) in &self.entries {
                let registration = |source| Error::CommandRegistration {
                    name: name.clone(),
                    source,
                };
                let options = definition.option_tree();
                let command = match self.scope {
                    Scope::Guild(guild_id) => {
                        let mut request = client
                            .create_guild_command(guild_id)
                            .chat_input(name, &definition.description)
                            .command_options(&options);
                        if let Some(permissions) = definition.permissions.discord_default() {
                            request = request.default_member_permissions(permissions);
                        }
                        request.await
                    }
                    Scope::Global => {
                        let mut request = client
                            .create_global_command()
                            .chat_input(name, &definition.description)
                            .command_options(&options);
                        if let Some(permissions) = definition.permissions.discord_default() {
                            request = request.default_member_permissions(permissions);
                        }
                        request.await
                    }
                }
                .map_err(registration)?
                .model()
                .await?;
                registered.push(command);
            }
            registered
        };

        let count = registered.len();
        let active = self.bind(registered)?;
        info.report(InfoEvent::SlashCommandsSynced {
            guild_id: active.scope.guild_id(),
            count,
        });
        Ok(active)
    }

    #[allow(
        deprecated,
        reason = "Twilight の Command モデルに含まれる dm_permission は送信しない。"
    )]
    fn replacement_commands(&self) -> Result<Vec<Command>> {
        self.entries
            .iter()
            .map(|(name, definition)| {
                let command = Command {
                    application_id: None,
                    contexts: None,
                    default_member_permissions: definition.permissions.discord_default(),
                    description: definition.description.clone(),
                    description_localizations: None,
                    dm_permission: None,
                    guild_id: None,
                    id: None,
                    integration_types: None,
                    kind: CommandType::ChatInput,
                    name: name.clone(),
                    name_localizations: None,
                    nsfw: None,
                    options: definition.option_tree(),
                    version: Id::new(1),
                };
                command::command(&command)?;
                Ok(command)
            })
            .collect()
    }

    fn bind(mut self, registered: Vec<Command>) -> Result<ActiveSlashCommands> {
        if registered.len() != self.entries.len() {
            return Err(CommandSyncError::CountMismatch {
                expected: self.entries.len(),
                returned: registered.len(),
            }
            .into());
        }

        let mut handlers = HashMap::with_capacity(registered.len());
        for command in registered {
            let name = command.name;
            if command.kind != CommandType::ChatInput {
                return Err(CommandSyncError::NotChatInput { name }.into());
            }
            let id = command
                .id
                .ok_or_else(|| CommandSyncError::MissingId { name: name.clone() })?;
            let definition = self
                .entries
                .remove(&name)
                .ok_or_else(|| CommandSyncError::UnexpectedName { name: name.clone() })?;
            if let std::collections::hash_map::Entry::Vacant(entry) = handlers.entry(id) {
                entry.insert(definition.into_route());
            } else {
                return Err(CommandSyncError::DuplicateId { name }.into());
            }
        }

        Ok(ActiveSlashCommands {
            scope: self.scope,
            handlers,
        })
    }
}

/// サブコマンドを登録する先です。[`SlashCommands::group`] で作ります。
///
/// Discord では、`コマンド` → `グループ` → `サブコマンド` の最大 3 段まで作れます。
///
/// ```
/// use disnana_akari::{Result, SlashCommands, SlashContext};
///
/// async fn set(ctx: &mut SlashContext<'_>) -> Result<()> {
///     ctx.reply("設定しました").await
/// }
///
/// # fn main() -> Result<()> {
/// let mut slash = SlashCommands::global();
/// let mut config = slash.group("config", "設定を変更します")?;
/// config.subcommand("set", "値を設定します", vec![], set)?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct CommandGroup<'a> {
    root: &'a mut Definition,
    /// サブコマンドをまとめるグループの中にいるときの、そのグループ名。
    group: Option<String>,
}

impl std::fmt::Debug for Definition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Definition")
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}

impl CommandGroup<'_> {
    /// サブコマンドを登録します。
    ///
    /// # エラー
    ///
    /// Discord の制約に合わない定義や、同じ階層で重複した名前を拒否します。
    pub fn subcommand<E>(
        &mut self,
        name: &str,
        description: &str,
        options: Vec<CommandOption>,
        handler: impl AsyncFn(&mut SlashContext<'_>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        command::chat_input_name(name)?;
        command::description(description)?;
        command::options(&options)?;
        let definition = SubDefinition {
            description: description.into(),
            options,
            handlers: RouteHandlers {
                command: Some(Box::new(
                    adapt_handler!(handler, ctx: &mut SlashContext<'_>),
                )),
                autocompletes: HashMap::new(),
            },
        };
        let siblings = match &self.group {
            None => &mut self.root.children,
            Some(group) => match self.root.children.get_mut(group) {
                Some(Child::Group { subs, .. }) => return insert_sub(subs, name, definition),
                // グループは作成時に登録済みなので、通常は来ない。来たときは、黙って捨てずに失敗させる。
                _ => {
                    return Err(RegistrationError::UnknownCommand {
                        name: group.clone(),
                    }
                    .into());
                }
            },
        };
        match siblings.entry(name.into()) {
            Entry::Vacant(entry) => {
                entry.insert(Child::Sub(definition));
                Ok(())
            }
            Entry::Occupied(_) => Err(duplicate_sub(name)),
        }
    }

    /// サブコマンドをまとめるグループを作ります。`/config advanced set` の `advanced` にあたります。
    ///
    /// グループの中に、さらにグループは作れません。
    ///
    /// # エラー
    ///
    /// 名前や説明が Discord の制約に合わない場合、重複した名前、グループの中でさらにグループを
    /// 作ろうとした場合に失敗します。
    pub fn group(&mut self, name: &str, description: &str) -> Result<CommandGroup<'_>> {
        if self.group.is_some() {
            return Err(RegistrationError::NestedGroup {
                name: name.to_owned(),
            }
            .into());
        }
        command::chat_input_name(name)?;
        command::description(description)?;
        match self.root.children.entry(name.into()) {
            Entry::Vacant(entry) => {
                entry.insert(Child::Group {
                    description: description.into(),
                    subs: BTreeMap::new(),
                });
            }
            Entry::Occupied(_) => return Err(duplicate_sub(name)),
        }
        Ok(CommandGroup {
            root: &mut *self.root,
            group: Some(name.to_owned()),
        })
    }
}

fn insert_sub(
    subs: &mut BTreeMap<String, SubDefinition>,
    name: &str,
    definition: SubDefinition,
) -> Result<()> {
    match subs.entry(name.into()) {
        Entry::Vacant(entry) => {
            entry.insert(definition);
            Ok(())
        }
        Entry::Occupied(_) => Err(duplicate_sub(name)),
    }
}

fn duplicate_sub(name: &str) -> Error {
    RegistrationError::Duplicate {
        kind: HandlerKind::SlashCommand,
        name: name.to_owned(),
    }
    .into()
}

/// 経路から、先頭のコマンド名を除いた、サブコマンドの部分。
fn sub_path<'a, 'b>(path: &'a [&'b str]) -> &'a [&'b str] {
    path.get(1..).unwrap_or_default()
}

/// コマンド名に、呼ばれたサブコマンドとグループの名前を続けた経路。
pub(crate) fn command_path(data: &CommandData) -> Vec<&str> {
    let mut path = vec![data.name.as_str()];
    let mut options = data.options.as_slice();
    while let [option] = options {
        match &option.value {
            CommandOptionValue::SubCommand(inner) | CommandOptionValue::SubCommandGroup(inner) => {
                path.push(option.name.as_str());
                options = inner;
            }
            _ => break,
        }
    }
    path
}

/// サブコマンドの入れ子をたどった先の、実際の引数。
pub(crate) fn leaf_options(mut options: &[CommandDataOption]) -> &[CommandDataOption] {
    while let [option] = options {
        match &option.value {
            CommandOptionValue::SubCommand(inner) | CommandOptionValue::SubCommandGroup(inner) => {
                options = inner;
            }
            _ => break,
        }
    }
    options
}

#[derive(Clone, Copy)]
enum PermissionPolicy {
    Unrestricted,
    Visibility(Permissions),
    Required(Permissions),
}

impl PermissionPolicy {
    fn discord_default(self) -> Option<Permissions> {
        match self {
            Self::Unrestricted => None,
            Self::Visibility(permissions) => Some(permissions),
            Self::Required(permissions) if permissions.contains(Permissions::ADMINISTRATOR) => {
                Some(Permissions::empty())
            }
            Self::Required(permissions) => Some(permissions),
        }
    }

    fn check(self, interaction: &Interaction) -> Result<()> {
        if let Self::Required(required) = self {
            check_permissions(
                interaction.guild_id,
                interaction_permissions(interaction),
                required,
            )?;
        }
        Ok(())
    }
}

fn interaction_permissions(interaction: &Interaction) -> Option<Permissions> {
    interaction
        .member
        .as_ref()
        .and_then(|member| member.permissions)
}

struct Definition {
    description: String,
    options: Vec<CommandOption>,
    permissions: PermissionPolicy,
    /// サブコマンドを持たない、普通のコマンドの関数と補完。
    handlers: RouteHandlers,
    /// サブコマンドと、そのグループ。
    children: BTreeMap<String, Child>,
}

enum Child {
    Sub(SubDefinition),
    Group {
        description: String,
        subs: BTreeMap<String, SubDefinition>,
    },
}

struct SubDefinition {
    description: String,
    options: Vec<CommandOption>,
    handlers: RouteHandlers,
}

impl Definition {
    /// Discord に送る引数の一覧。サブコマンドを持つ場合は、その入れ子を作る。
    fn option_tree(&self) -> Vec<CommandOption> {
        if self.children.is_empty() {
            return self.options.clone();
        }
        let nested = |kind, name: &String, description: &str, options: Vec<CommandOption>| {
            let mut option = SlashCommands::typed_option(kind, name, description, false);
            option.required = None;
            option.options = Some(options);
            option
        };
        self.children
            .iter()
            .map(|(name, child)| match child {
                Child::Sub(sub) => nested(
                    CommandOptionType::SubCommand,
                    name,
                    &sub.description,
                    sub.options.clone(),
                ),
                Child::Group { description, subs } => nested(
                    CommandOptionType::SubCommandGroup,
                    name,
                    description,
                    subs.iter()
                        .map(|(name, sub)| {
                            nested(
                                CommandOptionType::SubCommand,
                                name,
                                &sub.description,
                                sub.options.clone(),
                            )
                        })
                        .collect(),
                ),
            })
            .collect()
    }

    /// 受信側で、呼ぶ関数を引くための表に変える。
    fn into_route(self) -> Route {
        Route {
            root: self.handlers,
            children: self
                .children
                .into_iter()
                .map(|(name, child)| {
                    let child = match child {
                        Child::Sub(sub) => ChildRoute::Sub(sub.handlers),
                        Child::Group { subs, .. } => ChildRoute::Group(
                            subs.into_iter()
                                .map(|(name, sub)| (name, sub.handlers))
                                .collect(),
                        ),
                    };
                    (name, child)
                })
                .collect(),
            permissions: self.permissions,
        }
    }
}

/// 一つのコマンドについて、サブコマンドの経路ごとの関数。
struct Route {
    root: RouteHandlers,
    children: HashMap<String, ChildRoute>,
    permissions: PermissionPolicy,
}

enum ChildRoute {
    Sub(RouteHandlers),
    Group(HashMap<String, RouteHandlers>),
}

struct RouteHandlers {
    command: Option<Box<dyn SlashHandler>>,
    autocompletes: HashMap<String, Box<dyn AutocompleteHandler>>,
}

impl Route {
    fn find(&self, path: &[&str]) -> Option<&dyn SlashHandler> {
        self.handlers(path)?.command.as_deref()
    }

    fn find_autocomplete(&self, path: &[&str], option: &str) -> Option<&dyn AutocompleteHandler> {
        self.handlers(path)?
            .autocompletes
            .get(option)
            .map(AsRef::as_ref)
    }

    fn handlers(&self, path: &[&str]) -> Option<&RouteHandlers> {
        match path {
            [] => Some(&self.root),
            [sub] => match self.children.get(*sub)? {
                ChildRoute::Sub(handlers) => Some(handlers),
                ChildRoute::Group(_) => None,
            },
            [group, sub] => match self.children.get(*group)? {
                ChildRoute::Group(subs) => subs.get(*sub),
                ChildRoute::Sub(_) => None,
            },
            _ => None,
        }
    }
}

pub(crate) struct ActiveSlashCommands {
    scope: Scope,
    handlers: HashMap<Id<CommandMarker>, Route>,
}

impl ActiveSlashCommands {
    pub(crate) fn accepts(&self, interaction: &Interaction) -> bool {
        self.command_data(interaction).is_some()
    }

    fn command_data<'a>(&self, interaction: &'a Interaction) -> Option<&'a CommandData> {
        if !matches!(
            interaction.kind,
            InteractionType::ApplicationCommand | InteractionType::ApplicationCommandAutocomplete
        ) {
            return None;
        }
        let Some(InteractionData::ApplicationCommand(data)) = &interaction.data else {
            return None;
        };
        let in_scope = match self.scope {
            Scope::Guild(guild_id) => {
                interaction.guild_id == Some(guild_id)
                    && match data.guild_id {
                        Some(command_guild) => command_guild == guild_id,
                        None => self.handlers.contains_key(&data.id),
                    }
            }
            // グローバルコマンドは、どのサーバーや DM からも呼ばれる。
            // サーバー限定のコマンド（`data.guild_id` が `Some`）は対象にしない。
            Scope::Global => data.guild_id.is_none() && self.handlers.contains_key(&data.id),
        };
        (data.kind == CommandType::ChatInput && in_scope).then_some(data)
    }

    async fn dispatch_autocomplete(
        &self,
        initial: InitialResponder<'_>,
        data: &CommandData,
        path: &[&str],
        hooks: &Hooks,
    ) {
        let mut ctx = AutocompleteContext {
            initial,
            data,
            responded: false,
        };
        let name = path.join(" ");
        let Some(handler) = self
            .handlers
            .get(&data.id)
            .filter(|route| route.permissions.check(initial.interaction).is_ok())
            .and_then(|route| route.find_autocomplete(sub_path(path), ctx.focused_name()))
        else {
            // 権限がない場合や関数がない場合は空の候補を返す。
            if let Err(error) = ctx.respond_empty_if_needed().await {
                hooks
                    .reporter
                    .report(ErrorSource::Response, Some(&name), &error);
            }
            return;
        };
        if let Err(error) = handler.call(&mut ctx).await {
            hooks
                .reporter
                .report(ErrorSource::SlashCommand, Some(&name), &error);
        }
        if let Err(error) = ctx.respond_empty_if_needed().await {
            hooks
                .reporter
                .report(ErrorSource::Response, Some(&name), &error);
        }
    }

    pub(crate) async fn dispatch(
        &self,
        http: &Client,
        interaction: &Interaction,
        timing: Timing,
        hooks: &Hooks,
    ) {
        let Some(data) = self.command_data(interaction) else {
            return;
        };
        let path = command_path(data);
        let initial = InitialResponder {
            http,
            interaction,
            timing,
            info: &hooks.info,
        };
        if interaction.kind == InteractionType::ApplicationCommandAutocomplete {
            self.dispatch_autocomplete(initial, data, &path, hooks)
                .await;
            return;
        }
        let full_name = path.join(" ");
        let name = Some(full_name.as_str());
        let Some((route, handler)) = self
            .handlers
            .get(&data.id)
            .and_then(|route| route.find(sub_path(&path)).map(|handler| (route, handler)))
        else {
            hooks
                .reporter
                .report(ErrorSource::SlashCommand, name, &Error::CommandUnavailable);
            if let Some(text) = &hooks.messages.slash_command_unavailable
                && let Err(error) = initial.message(text, true).await
            {
                hooks.reporter.report(ErrorSource::Response, name, &error);
            }
            return;
        };

        let mut response_state = ResponseState::Pending;
        let mut ctx = SlashContext {
            initial,
            data,
            response_state: &mut response_state,
        };
        let result = match route.permissions.check(interaction) {
            Ok(()) => handler.call(&mut ctx).await,
            Err(error) => Err(error),
        };
        let check_failed = match result {
            Ok(()) => false,
            Err(error) => {
                hooks
                    .reporter
                    .report(ErrorSource::SlashCommand, name, &error);
                matches!(error, Error::Check(_))
            }
        };
        let failed_text = if check_failed {
            &hooks.messages.check_failed
        } else {
            &hooks.messages.slash_command_failed
        };
        // 応答が済んでいない（エラー、または応答の忘れ）ときは、文言が設定されていれば知らせる。
        let Some(text) = failed_text else {
            return;
        };
        if let Err(error) = ctx.reply_fallback(text, check_failed).await {
            hooks.reporter.report(ErrorSource::Response, name, &error);
        }
    }
}

/// 一回のスラッシュコマンド呼び出しを扱います。
pub struct SlashContext<'a> {
    initial: InitialResponder<'a>,
    data: &'a CommandData,
    response_state: &'a mut ResponseState,
}

impl std::fmt::Debug for SlashContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlashContext")
            .field("interaction_id", &self.initial.interaction.id)
            .field("timing", &self.initial.timing)
            .field("command", &self.data.name)
            .field("response_state", &self.response_state)
            .finish_non_exhaustive()
    }
}

impl<'a> SlashContext<'a> {
    async fn reply_fallback(&mut self, text: &str, check_failed: bool) -> Result<()> {
        match *self.response_state {
            ResponseState::Pending => self.reply_ephemeral(text).await,
            ResponseState::Deferred { ephemeral: false } if check_failed => {
                // Discord は遅延応答直後の追加応答を元のメッセージの編集として扱うことがある。
                // 公開の応答を先に完了させ、拒否文は非公開の追加応答として送る。
                self.reply("処理を終了しました。").await?;
                self.followup_ephemeral(text).await
            }
            ResponseState::Deferred { .. } => self.reply(text).await,
            ResponseState::Replied | ResponseState::ModalOpened => Ok(()),
        }
    }

    /// コマンドを呼び出したユーザーの ID を返します。
    pub fn author_id(&self) -> Option<Id<UserMarker>> {
        self.initial.interaction.author_id()
    }

    /// コマンドが呼ばれたサーバーの ID を返します。DM では `None` です。
    pub fn guild_id(&self) -> Option<Id<GuildMarker>> {
        self.initial.interaction.guild_id
    }

    /// 呼び出した人の、このチャンネルでの権限を返します。DM などで届かないときは `None` です。
    ///
    /// Discord が、ロールとチャンネルごとの上書きを反映して計算した値です。
    /// 管理者は、すべての権限を持つものとして返ります。
    pub fn permissions(&self) -> Option<Permissions> {
        interaction_permissions(self.initial.interaction)
    }

    /// サーバーの中で呼ばれたことを確認し、その ID を返します。
    ///
    /// # エラー
    ///
    /// DM などで呼ばれた場合は、[`CheckFailure::GuildOnly`] を返します。
    pub fn require_guild(&self) -> Result<Id<GuildMarker>> {
        self.guild_id()
            .ok_or_else(|| CheckFailure::GuildOnly.into())
    }

    /// 呼び出した人が、指定した権限をすべて持っていることを確認します。
    ///
    /// # エラー
    ///
    /// DM などでは [`CheckFailure::GuildOnly`]、権限が届かないときは
    /// [`CheckFailure::PermissionsUnavailable`]、足りないときは不足分を入れた
    /// [`CheckFailure::MissingPermissions`] を返します。
    pub fn require_permissions(&self, required: Permissions) -> Result<()> {
        check_permissions(self.guild_id(), self.permissions(), required).map_err(Error::from)
    }

    /// コマンドが呼ばれたチャンネルの ID を返します。
    /// 古い形式のペイロードでは、チャンネルオブジェクトに代わって送られる ID を使います。
    #[allow(
        deprecated,
        reason = "古い形式のペイロードでは、channel の代わりに channel_id が送られる。"
    )]
    pub fn channel_id(&self) -> Option<Id<ChannelMarker>> {
        self.initial
            .interaction
            .channel
            .as_ref()
            .map(|channel| channel.id)
            .or(self.initial.interaction.channel_id)
    }

    /// Discord の REST API を直接使うために、Bot の Twilight クライアントを借ります。
    pub fn http(&self) -> &'a Client {
        self.initial.http
    }

    /// Gateway の往復時間と、このコマンドの処理時間を確認します。
    pub fn timing(&self) -> Timing {
        self.initial.timing
    }

    pub(crate) fn reborrow(&mut self) -> SlashContext<'_> {
        SlashContext {
            initial: self.initial,
            data: self.data,
            response_state: self.response_state,
        }
    }

    /// 指定した名前の引数を Twilight の値として返します。
    ///
    /// サブコマンドで呼ばれたときは、その引数を探します。
    pub fn option(&self, name: &str) -> Option<&'a CommandOptionValue> {
        leaf_options(&self.data.options)
            .iter()
            .find(|option| option.name == name)
            .map(|option| &option.value)
    }

    /// コマンド名と、サブコマンドの名前を、呼ばれた順に返します。
    ///
    /// `/config set` なら `["config", "set"]`、サブコマンドのないコマンドなら `["ping"]` です。
    pub fn command_path(&self) -> Vec<&'a str> {
        command_path(self.data)
    }

    /// 文字列の引数を借ります。未入力または別の型なら `None` を返します。
    pub fn string(&self, name: &str) -> Option<&'a str> {
        match self.option(name)? {
            CommandOptionValue::String(value) => Some(value),
            _ => None,
        }
    }

    /// 整数の引数を返します。未入力または別の型なら `None` を返します。
    pub fn integer(&self, name: &str) -> Option<i64> {
        match self.option(name)? {
            CommandOptionValue::Integer(value) => Some(*value),
            _ => None,
        }
    }

    /// 数値（小数を含む）の引数を返します。未入力または別の型なら `None` を返します。
    pub fn number(&self, name: &str) -> Option<f64> {
        match self.option(name)? {
            CommandOptionValue::Number(value) => Some(*value),
            _ => None,
        }
    }

    /// 真偽値の引数を返します。未入力または別の型なら `None` を返します。
    pub fn boolean(&self, name: &str) -> Option<bool> {
        match self.option(name)? {
            CommandOptionValue::Boolean(value) => Some(*value),
            _ => None,
        }
    }

    /// 文字列の引数を [`FromStr`](std::str::FromStr) で変換します。
    ///
    /// 未入力なら `None`、入力があって変換に失敗したら `Some(Err(..))` を返します。
    /// エラーはそのまま `?` で返せます（ハンドラーは任意のエラー型を返せます）。
    pub fn parse<T: std::str::FromStr>(
        &self,
        name: &str,
    ) -> Option<std::result::Result<T, T::Err>> {
        self.string(name).map(|value| value.trim().parse())
    }

    /// チャンネルに初回応答を送るか、遅延応答の元メッセージを完成させます。
    /// 遅延応答の公開範囲は `defer` の時点で決まります。
    ///
    /// # エラー
    ///
    /// 二回目の応答や、Discord が受け付けなかった場合に失敗します。
    pub async fn reply(&mut self, content: &str) -> Result<()> {
        self.respond(content, false).await
    }

    /// 埋め込みやボタンを含む初回応答を送るか、遅延応答の元メッセージを完成させます。
    ///
    /// 初回応答はチャンネルに表示されます。遅延応答の公開範囲は `defer` の時点で決まります。
    /// メンション通知は抑止します。
    ///
    /// # エラー
    ///
    /// 応答済み、メッセージの内容が無効、または Discord が受け付けなかった場合に失敗します。
    pub async fn reply_ui(&mut self, message: &UiMessage) -> Result<()> {
        self.respond_ui(message, false).await
    }

    /// 呼び出した本人にだけ見える初回応答を送るか、非公開の遅延応答を完成させます。
    ///
    /// # エラー
    ///
    /// 応答済み、公開の遅延応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn reply_ephemeral(&mut self, content: &str) -> Result<()> {
        self.respond(content, true).await
    }

    /// 埋め込みやボタンを含む非公開の初回応答を送るか、非公開の遅延応答を完成させます。
    /// メンション通知は抑止します。
    ///
    /// # エラー
    ///
    /// 応答済み、公開の遅延応答済み、メッセージの内容が無効、または Discord が受け付けなかった場合に失敗します。
    pub async fn reply_ui_ephemeral(&mut self, message: &UiMessage) -> Result<()> {
        self.respond_ui(message, true).await
    }

    /// Components V2 の初回応答を送るか、`defer` 済みの応答を完成させます。
    ///
    /// # エラー
    /// 無効なコンポーネント、応答済み、または Discord が受け付けない場合に失敗します。
    pub async fn reply_v2(&mut self, message: &ComponentsV2) -> Result<()> {
        self.respond_v2(message, false).await
    }

    /// Components V2 を使い、実行者だけに見える応答を送ります。`defer_ephemeral` 後にも呼べます。
    ///
    /// # エラー
    /// 公開の遅延応答済み、応答済み、無効なコンポーネント、または送信失敗時にエラーになります。
    pub async fn reply_v2_ephemeral(&mut self, message: &ComponentsV2) -> Result<()> {
        self.respond_v2(message, true).await
    }

    /// 初回応答として入力フォームを開きます。送信後の処理は `Events::on_modal` に登録します。
    ///
    /// # エラー
    /// 応答済み、無効なフォーム、または Discord が受け付けない場合に失敗します。
    pub async fn show_modal(&mut self, modal: &Modal) -> Result<()> {
        if *self.response_state != ResponseState::Pending {
            return Err(ResponseStateError::ModalNotFirstResponse.into());
        }
        let response = modal.response()?;
        self.initial.send(&response).await?;
        *self.response_state = ResponseState::ModalOpened;
        Ok(())
    }

    /// 初回応答としてチャンネルに処理中の表示を出します。作業が終わったら `reply` で完成させます。
    ///
    /// # エラー
    ///
    /// 初回応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn defer(&mut self) -> Result<()> {
        self.defer_with_visibility(false).await
    }

    /// 初回応答として本人にだけ処理中の表示を出します。作業が終わったら `reply` で完成させます。
    ///
    /// # エラー
    ///
    /// 初回応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn defer_ephemeral(&mut self) -> Result<()> {
        self.defer_with_visibility(true).await
    }

    /// 初回メッセージを送った後、チャンネルに追加メッセージを送ります。
    ///
    /// # エラー
    ///
    /// 初回メッセージが未完成、または Discord が受け付けなかった場合に失敗します。
    pub async fn followup(&self, content: &str) -> Result<()> {
        self.send_followup(content, false).await
    }

    /// 初回メッセージを送った後、呼び出した本人にだけ追加メッセージを送ります。
    ///
    /// # エラー
    ///
    /// 初回メッセージが未完成、または Discord が受け付けなかった場合に失敗します。
    pub async fn followup_ephemeral(&self, content: &str) -> Result<()> {
        self.send_followup(content, true).await
    }

    async fn defer_with_visibility(&mut self, ephemeral: bool) -> Result<()> {
        self.response_state.defer(self.initial, ephemeral).await
    }

    async fn respond(&mut self, content: &str, ephemeral: bool) -> Result<()> {
        self.response_state
            .reply(self.initial, content, ephemeral)
            .await
    }

    async fn respond_ui(&mut self, message: &UiMessage, ephemeral: bool) -> Result<()> {
        message.validate()?;
        match self.response_state.reply_action(ephemeral)? {
            ReplyAction::Initial => {
                let response = initial_response(InitialResponse::Ui(message), ephemeral);
                self.initial.send(&response).await?;
            }
            ReplyAction::EditDeferred => {
                edit_ui_response(self.initial.http, self.initial.interaction, message).await?;
            }
        }
        *self.response_state = ResponseState::Replied;
        Ok(())
    }

    async fn send_followup(&self, content: &str, ephemeral: bool) -> Result<()> {
        self.response_state
            .followup(
                self.initial.http,
                self.initial.interaction,
                content,
                ephemeral,
            )
            .await
    }

    async fn respond_v2(&mut self, message: &ComponentsV2, ephemeral: bool) -> Result<()> {
        message.validate()?;
        match self.response_state.reply_action(ephemeral)? {
            ReplyAction::Initial => {
                let response = initial_response(InitialResponse::V2(message), ephemeral);
                self.initial.send(&response).await?;
            }
            ReplyAction::EditDeferred => {
                edit_v2_response(self.initial.http, self.initial.interaction, message).await?
            }
        }
        *self.response_state = ResponseState::Replied;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResponseState {
    Pending,
    Deferred { ephemeral: bool },
    Replied,
    ModalOpened,
}

impl ResponseState {
    fn reply_action(self, ephemeral: bool) -> Result<ReplyAction> {
        match self {
            Self::Pending => Ok(ReplyAction::Initial),
            Self::Deferred { ephemeral: false } if ephemeral => {
                Err(ResponseStateError::VisibilityLocked.into())
            }
            Self::Deferred { .. } => Ok(ReplyAction::EditDeferred),
            Self::Replied | Self::ModalOpened => Err(ResponseStateError::AlreadyResponded.into()),
        }
    }

    pub(crate) async fn defer(
        &mut self,
        initial: InitialResponder<'_>,
        ephemeral: bool,
    ) -> Result<()> {
        if *self != Self::Pending {
            return Err(ResponseStateError::AlreadyResponded.into());
        }
        initial
            .send(&initial_response(InitialResponse::Deferred, ephemeral))
            .await?;
        *self = Self::Deferred { ephemeral };
        Ok(())
    }

    pub(crate) async fn reply(
        &mut self,
        initial: InitialResponder<'_>,
        content: &str,
        ephemeral: bool,
    ) -> Result<()> {
        match self.reply_action(ephemeral)? {
            ReplyAction::Initial => initial.message(content, ephemeral).await?,
            ReplyAction::EditDeferred => {
                edit_text_response(initial.http, initial.interaction, content).await?
            }
        }
        *self = Self::Replied;
        Ok(())
    }

    pub(crate) async fn followup(
        &self,
        http: &Client,
        interaction: &Interaction,
        content: &str,
        ephemeral: bool,
    ) -> Result<()> {
        if *self != Self::Replied {
            return Err(ResponseStateError::FollowupBeforeResponse.into());
        }
        send_followup_message(http, interaction, content, ephemeral).await
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ReplyAction {
    Initial,
    EditDeferred,
}

enum InitialResponse<'a> {
    Message(&'a str),
    Ui(&'a UiMessage),
    V2(&'a ComponentsV2),
    Deferred,
}

fn initial_response(response: InitialResponse<'_>, ephemeral: bool) -> InteractionResponse {
    let flags = ephemeral.then_some(MessageFlags::EPHEMERAL);
    match response {
        InitialResponse::Message(content) => InteractionResponse {
            kind: InteractionResponseType::ChannelMessageWithSource,
            data: Some(InteractionResponseData {
                content: Some(content.into()),
                allowed_mentions: Some(AllowedMentions::default()),
                flags,
                ..Default::default()
            }),
        },
        InitialResponse::Ui(message) => InteractionResponse {
            kind: InteractionResponseType::ChannelMessageWithSource,
            data: Some(InteractionResponseData {
                content: message.content.clone(),
                embeds: (!message.embeds.is_empty()).then(|| message.embeds.clone()),
                components: (!message.components.is_empty()).then(|| message.components.clone()),
                allowed_mentions: Some(AllowedMentions::default()),
                flags,
                ..Default::default()
            }),
        },
        InitialResponse::V2(message) => InteractionResponse {
            kind: InteractionResponseType::ChannelMessageWithSource,
            data: Some(InteractionResponseData {
                components: Some(message.components.clone()),
                allowed_mentions: Some(AllowedMentions::default()),
                flags: Some(
                    flags.unwrap_or(MessageFlags::empty()) | MessageFlags::IS_COMPONENTS_V2,
                ),
                ..Default::default()
            }),
        },
        InitialResponse::Deferred => InteractionResponse {
            kind: InteractionResponseType::DeferredChannelMessageWithSource,
            data: flags.map(|flags| InteractionResponseData {
                flags: Some(flags),
                ..Default::default()
            }),
        },
    }
}

pub(crate) fn v2_update_response(message: &ComponentsV2) -> InteractionResponse {
    let mut response = initial_response(InitialResponse::V2(message), false);
    response.kind = InteractionResponseType::UpdateMessage;
    response
}

pub(crate) async fn edit_v2_response(
    http: &Client,
    interaction: &Interaction,
    message: &ComponentsV2,
) -> Result<()> {
    http.interaction(interaction.application_id)
        .update_response(&interaction.token)
        .flags(MessageFlags::IS_COMPONENTS_V2)
        .components(Some(&message.components))
        .allowed_mentions(Some(&AllowedMentions::default()))
        .await?;
    Ok(())
}

#[derive(Clone, Copy)]
pub(crate) struct InitialResponder<'a> {
    pub(crate) http: &'a Client,
    pub(crate) interaction: &'a Interaction,
    pub(crate) timing: Timing,
    pub(crate) info: &'a InfoReporter,
}

impl InitialResponder<'_> {
    pub(crate) async fn message(self, content: &str, ephemeral: bool) -> Result<()> {
        let response = initial_response(InitialResponse::Message(content), ephemeral);
        self.send(&response).await
    }

    pub(crate) async fn send(self, response: &InteractionResponse) -> Result<()> {
        let started_at = self.info.has_hook().then(Instant::now);
        self.http
            .interaction(self.interaction.application_id)
            .create_response(self.interaction.id, &self.interaction.token, response)
            .await?;
        if let Some(started_at) = started_at {
            let completed_at = Instant::now();
            self.info.report(InfoEvent::InteractionResponded {
                interaction_type: self.interaction.kind,
                response_type: response.kind,
                elapsed: self.timing.elapsed_since_receive(completed_at),
                rest_elapsed: completed_at.saturating_duration_since(started_at),
            });
        }
        Ok(())
    }
}

async fn edit_text_response(http: &Client, interaction: &Interaction, content: &str) -> Result<()> {
    http.interaction(interaction.application_id)
        .update_response(&interaction.token)
        .content(Some(content))
        .allowed_mentions(Some(&AllowedMentions::default()))
        .await?;
    Ok(())
}

pub(crate) async fn edit_ui_response(
    http: &Client,
    interaction: &Interaction,
    message: &UiMessage,
) -> Result<()> {
    http.interaction(interaction.application_id)
        .update_response(&interaction.token)
        .content(Some(message.content.as_deref().unwrap_or_default()))
        .embeds(Some(&message.embeds))
        .components(Some(&message.components))
        .allowed_mentions(Some(&AllowedMentions::default()))
        .await?;
    Ok(())
}

pub(crate) async fn send_followup_message(
    http: &Client,
    interaction: &Interaction,
    content: &str,
    ephemeral: bool,
) -> Result<()> {
    let mentions = AllowedMentions::default();
    let client = http.interaction(interaction.application_id);
    let mut followup = client
        .create_followup(&interaction.token)
        .content(content)
        .allowed_mentions(Some(&mentions));
    if ephemeral {
        followup = followup.flags(MessageFlags::EPHEMERAL);
    }
    followup.await?;
    Ok(())
}

trait SlashHandler: Send + Sync {
    fn call<'a, 'ctx>(
        &'a self,
        ctx: &'a mut SlashContext<'ctx>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>>;
}

impl<F> SlashHandler for F
where
    F: AsyncFn(&mut SlashContext<'_>) -> Result<()> + Send + Sync,
{
    fn call<'a, 'ctx>(
        &'a self,
        ctx: &'a mut SlashContext<'ctx>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        Box::pin(self(ctx))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        ActiveSlashCommands, AllowedMentions, AutocompleteContext, CommandData, CommandDataOption,
        CommandOptionType, CommandOptionValue, CommandType, Error, GuildMarker, Id,
        InitialResponder, InitialResponse, Interaction, InteractionData, InteractionResponseType,
        MessageFlags, PermissionPolicy, Permissions, ReplyAction, ResponseState,
        ResponseStateError, Result, Route, Scope, SlashCommands, SlashContext, command_path,
        initial_response, leaf_options, v2_update_response,
    };
    use crate::{
        RegistrationError,
        hooks::Hooks,
        ui::{Button, ComponentsV2, Embed, UiMessage},
    };
    use twilight_http::Client;

    async fn handler(_: &mut SlashContext<'_>) -> Result<()> {
        Ok(())
    }

    #[test]
    fn v2_responses_use_components_and_preserve_visibility_rules() {
        let message = ComponentsV2::new().text("V2 パネル");
        let private = initial_response(InitialResponse::V2(&message), true);
        assert_eq!(
            private.kind,
            InteractionResponseType::ChannelMessageWithSource
        );
        let data = private.data.unwrap();
        assert_eq!(
            data.flags,
            Some(MessageFlags::IS_COMPONENTS_V2 | MessageFlags::EPHEMERAL)
        );
        assert!(data.content.is_none());
        assert!(data.embeds.is_none());
        assert_eq!(data.components.unwrap(), message.components);
        assert_eq!(data.allowed_mentions, Some(AllowedMentions::default()));

        let update = v2_update_response(&message);
        assert_eq!(update.kind, InteractionResponseType::UpdateMessage);
        assert_eq!(
            update.data.unwrap().flags,
            Some(MessageFlags::IS_COMPONENTS_V2)
        );
        assert!(ResponseState::ModalOpened.reply_action(false).is_err());
    }

    #[allow(
        deprecated,
        reason = "テスト用の Interaction を組み立てるために、非推奨のフィールドを埋める。"
    )]
    fn command_interaction(
        interaction_guild: Option<Id<GuildMarker>>,
        command_guild: Option<Id<GuildMarker>>,
        command_id: u64,
    ) -> Interaction {
        use twilight_model::{
            application::interaction::{
                InteractionData, InteractionType, application_command::CommandData,
            },
            oauth::ApplicationIntegrationMap,
        };
        Interaction {
            app_permissions: None,
            application_id: Id::new(10),
            attachment_size_limit: 0,
            authorizing_integration_owners: ApplicationIntegrationMap {
                guild: None,
                user: None,
            },
            channel: None,
            channel_id: None,
            context: None,
            data: Some(InteractionData::ApplicationCommand(Box::new(CommandData {
                guild_id: command_guild,
                id: Id::new(command_id),
                kind: CommandType::ChatInput,
                name: "ping".into(),
                options: vec![],
                resolved: None,
                target_id: None,
            }))),
            entitlements: vec![],
            guild: None,
            guild_id: interaction_guild,
            guild_locale: None,
            id: Id::new(1),
            kind: InteractionType::ApplicationCommand,
            locale: None,
            member: None,
            message: None,
            token: String::new(),
            user: None,
        }
    }

    fn active(scope: Scope) -> ActiveSlashCommands {
        let mut handlers = HashMap::new();
        handlers.insert(
            Id::new(5),
            Route {
                root: super::RouteHandlers {
                    command: Some(Box::new(async |_: &mut SlashContext<'_>| -> Result<()> {
                        Ok(())
                    })),
                    autocompletes: HashMap::new(),
                },
                children: HashMap::new(),
                permissions: PermissionPolicy::Unrestricted,
            },
        );
        ActiveSlashCommands { scope, handlers }
    }

    #[tokio::test]
    async fn a_failed_check_after_public_defer_sends_the_reason_only_in_a_private_followup()
    -> Result<()> {
        use std::time::Duration;
        use tokio::{net::TcpListener, time::timeout};

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(Error::handler)?;
        let http = twilight_http::Client::builder()
            .proxy(
                listener.local_addr().map_err(Error::handler)?.to_string(),
                true,
            )
            .build();
        let mut commands = active(Scope::Global);
        commands.handlers.get_mut(&Id::new(5)).unwrap().root.command =
            Some(Box::new(async |ctx: &mut SlashContext<'_>| -> Result<()> {
                ctx.defer().await?;
                ctx.require_permissions(Permissions::MANAGE_MESSAGES)
            }));
        let mut interaction = command_interaction(None, None, 5);
        interaction.token = "test-token".into();
        let mut hooks = crate::hooks::Hooks::default();
        hooks.messages.check_failed = Some("private check failure".into());
        let (_, timing) = crate::timing::Pending::new((), None).begin();

        let server = async {
            for (method, contains_denial, private) in [
                ("POST", false, false),
                ("PATCH", false, false),
                ("POST", true, true),
            ] {
                let body = capture_response(&listener, method).await?;
                assert_eq!(body.contains("private check failure"), contains_denial);
                assert_eq!(body.contains("\"flags\":64"), private);
                if method == "PATCH" {
                    assert!(body.contains("処理を終了しました。"));
                }
            }
            Ok::<_, std::io::Error>(())
        };
        let (_, server) = timeout(Duration::from_secs(5), async {
            tokio::join!(
                commands.dispatch(&http, &interaction, timing, &hooks),
                server
            )
        })
        .await
        .map_err(Error::handler)?;
        server.map_err(Error::handler)
    }

    async fn capture_response(
        listener: &tokio::net::TcpListener,
        method: &str,
    ) -> std::io::Result<String> {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

        let (stream, _) = listener.accept().await?;
        let mut stream = BufReader::new(stream);
        let mut line = String::new();
        stream.read_line(&mut line).await?;
        assert_eq!(line.split_whitespace().next(), Some(method));
        let mut length = 0;
        loop {
            line.clear();
            if stream.read_line(&mut line).await? == 0 || line == "\r\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse().unwrap();
            }
        }
        let mut body = vec![0; length];
        stream.read_exact(&mut body).await?;
        stream
            .get_mut()
            .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
            .await?;
        Ok(String::from_utf8(body).unwrap())
    }

    #[tokio::test]
    async fn protected_subcommand_checks_permissions_for_execution_and_autocomplete() -> Result<()>
    {
        use twilight_model::application::interaction::InteractionType;

        let guild_id = Id::new(1);
        let command_id = Id::new(5);
        let mut slash = SlashCommands::for_guild(guild_id).replace_commands();
        slash
            .group_with_permissions("ping", "管理", Permissions::MANAGE_GUILD)?
            .group("advanced", "詳細")?
            .subcommand(
                "run",
                "実行",
                vec![SlashCommands::string_option("key", "対象", true)],
                async |ctx: &mut SlashContext<'_>| -> Result<()> {
                    ctx.reply_ephemeral("protected command ran").await
                },
            )?;
        slash.autocomplete(
            &["ping", "advanced", "run"],
            "key",
            async |ctx: &mut AutocompleteContext<'_>| -> Result<()> {
                ctx.respond(vec![SlashCommands::string_choice(
                    "protected suggestion",
                    "key",
                )])
                .await
            },
        )?;
        assert!(matches!(
            slash.default_member_permissions("ping", Permissions::empty()),
            Err(Error::Registration(
                RegistrationError::CommandPermissionsAlreadyRequired { .. }
            ))
        ));
        let mut registered = slash.replacement_commands()?;
        assert_eq!(
            registered[0].default_member_permissions,
            Some(Permissions::MANAGE_GUILD)
        );
        registered[0].id = Some(command_id);
        let commands = slash.bind(registered)?;
        let mut interaction = command_interaction(Some(guild_id), Some(guild_id), command_id.get());
        assert!(matches!(
            commands.handlers[&command_id]
                .permissions
                .check(&interaction),
            Err(Error::Check(crate::CheckFailure::PermissionsUnavailable))
        ));
        interaction.member = Some(twilight_model::guild::PartialMember {
            avatar: None,
            avatar_decoration_data: None,
            banner: None,
            communication_disabled_until: None,
            deaf: false,
            flags: twilight_model::guild::MemberFlags::empty(),
            joined_at: None,
            mute: false,
            nick: None,
            permissions: Some(Permissions::VIEW_CHANNEL),
            premium_since: None,
            roles: Vec::new(),
            user: None,
        });
        assert!(matches!(
            commands.handlers[&command_id].permissions.check(&interaction),
            Err(Error::Check(crate::CheckFailure::MissingPermissions(required))) if required == Permissions::MANAGE_GUILD
        ));
        let Some(InteractionData::ApplicationCommand(data)) = &mut interaction.data else {
            panic!("想定外の構造");
        };
        let nested_options = |value| {
            vec![option(
                "advanced",
                CommandOptionValue::SubCommandGroup(vec![option(
                    "run",
                    CommandOptionValue::SubCommand(vec![option("key", value)]),
                )]),
            )]
        };
        data.options = nested_options(CommandOptionValue::String("key".into()));
        interaction.token = "test-token".into();
        let mut autocomplete = interaction.clone();
        let Some(InteractionData::ApplicationCommand(data)) = &mut autocomplete.data else {
            panic!("想定外の構造");
        };
        data.options = nested_options(CommandOptionValue::Focused(
            "k".into(),
            CommandOptionType::String,
        ));
        autocomplete.kind = InteractionType::ApplicationCommandAutocomplete;

        let mut allowed = interaction.clone();
        allowed.member.as_mut().unwrap().permissions = Some(Permissions::MANAGE_GUILD);
        let mut allowed_autocomplete = autocomplete.clone();
        allowed_autocomplete.member.clone_from(&allowed.member);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(Error::handler)?;
        let http = Client::builder()
            .proxy(
                listener.local_addr().map_err(Error::handler)?.to_string(),
                true,
            )
            .build();
        let mut hooks = Hooks::default();
        hooks.messages.check_failed = Some("private check failure".into());
        let (_, timing) = crate::timing::Pending::new((), None).begin();
        for (input, expected, absent) in [
            (
                &interaction,
                "private check failure",
                "protected command ran",
            ),
            (&allowed, "protected command ran", "private check failure"),
            (&autocomplete, "\"choices\":[]", "protected suggestion"),
            (
                &allowed_autocomplete,
                "protected suggestion",
                "private check failure",
            ),
        ] {
            let (_, response) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                tokio::join!(
                    commands.dispatch(&http, input, timing, &hooks),
                    capture_response(&listener, "POST"),
                )
            })
            .await
            .map_err(Error::handler)?;
            let response = response.map_err(Error::handler)?;
            assert!(response.contains(expected), "{response}");
            assert!(!response.contains(absent), "{response}");
            if input.kind == InteractionType::ApplicationCommand {
                assert!(response.contains("\"flags\":64"), "{response}");
            }
        }
        Ok(())
    }

    #[test]
    fn global_commands_accept_dms_and_any_guild_but_not_guild_only_commands() {
        let global = active(Scope::Global);
        let guild = Id::new(1);
        assert!(global.accepts(&command_interaction(None, None, 5)));
        assert!(global.accepts(&command_interaction(Some(guild), None, 5)));
        // 登録していない ID や、サーバー限定のコマンドは受け付けない。
        assert!(!global.accepts(&command_interaction(None, None, 6)));
        assert!(!global.accepts(&command_interaction(Some(guild), Some(guild), 5)));
    }

    #[test]
    fn guild_commands_only_accept_their_own_guild() {
        let guild = Id::new(1);
        let scoped = active(Scope::Guild(guild));
        assert!(scoped.accepts(&command_interaction(Some(guild), None, 5)));
        assert!(!scoped.accepts(&command_interaction(None, None, 5)));
        assert!(!scoped.accepts(&command_interaction(Some(Id::new(2)), None, 5)));
        assert!(!scoped.accepts(&command_interaction(Some(guild), Some(Id::new(2)), 6)));
    }

    #[test]
    fn default_member_permissions_needs_a_registered_command() -> Result<()> {
        let mut slash = SlashCommands::for_guild(Id::new(1));
        assert!(
            slash
                .default_member_permissions("ban", Permissions::BAN_MEMBERS)
                .is_err()
        );
        slash.register("ban", "説明", vec![], handler)?;
        slash.default_member_permissions("ban", Permissions::BAN_MEMBERS)?;
        let commands = {
            let mut slash = slash;
            slash.replace = true;
            slash.replacement_commands()?
        };
        assert_eq!(
            commands[0].default_member_permissions,
            Some(Permissions::BAN_MEMBERS)
        );
        Ok(())
    }

    #[test]
    fn restricted_registration_sets_discord_default_permissions() -> Result<()> {
        let mut global = SlashCommands::global();
        assert!(
            global
                .register_with_permissions("ban", "追放", vec![], Permissions::BAN_MEMBERS, handler)
                .is_err()
        );
        let mut slash = SlashCommands::for_guild(Id::new(1)).replace_commands();
        slash.register_with_permissions(
            "admin",
            "管理",
            vec![],
            Permissions::ADMINISTRATOR,
            handler,
        )?;
        slash.register_with_permissions(
            "ban",
            "追放",
            vec![],
            Permissions::BAN_MEMBERS,
            handler,
        )?;
        let commands = slash.replacement_commands()?;
        assert_eq!(
            commands[0].default_member_permissions,
            Some(Permissions::empty())
        );
        assert_eq!(
            commands[1].default_member_permissions,
            Some(Permissions::BAN_MEMBERS)
        );
        Ok(())
    }

    fn option(name: &str, value: CommandOptionValue) -> CommandDataOption {
        CommandDataOption {
            name: name.into(),
            value,
        }
    }

    #[test]
    fn groups_build_a_nested_option_tree_and_route_to_the_right_handler() -> Result<()> {
        let mut slash = SlashCommands::for_guild(Id::new(1));
        {
            let mut config = slash.group("config", "設定")?;
            config.subcommand(
                "set",
                "値を設定",
                vec![SlashCommands::string_option("value", "値", true)],
                handler,
            )?;
            assert!(config.subcommand("set", "重複", vec![], handler).is_err());
            let mut advanced = config.group("advanced", "詳細")?;
            advanced.subcommand("reset", "初期化", vec![], handler)?;
            // グループの中に、さらにグループは作れない。
            assert!(advanced.group("deeper", "もっと").is_err());
        }
        // 同じ名前の普通のコマンドや、グループは作れない。
        assert!(slash.register("config", "重複", vec![], handler).is_err());
        assert!(slash.group("config", "重複").is_err());

        let commands = {
            let mut replacing = slash;
            replacing.replace = true;
            replacing.replacement_commands()?
        };
        let options = commands[0].options.clone();
        let names: Vec<_> = options.iter().map(|o| (o.name.as_str(), o.kind)).collect();
        assert_eq!(
            names,
            [
                ("advanced", CommandOptionType::SubCommandGroup),
                ("set", CommandOptionType::SubCommand)
            ]
        );
        let set = &options[1];
        assert_eq!(set.required, None);
        assert_eq!(set.options.as_ref().map(Vec::len), Some(1));
        let reset = &options[0].options.as_ref().unwrap()[0];
        assert_eq!(
            (reset.name.as_str(), reset.kind),
            ("reset", CommandOptionType::SubCommand)
        );
        Ok(())
    }

    #[test]
    fn subcommand_path_and_options_are_read_through_the_nesting() {
        let inner = vec![option("value", CommandOptionValue::String("x".into()))];
        let group = vec![option("set", CommandOptionValue::SubCommand(inner))];
        let options = vec![option(
            "advanced",
            CommandOptionValue::SubCommandGroup(group),
        )];
        let data = CommandData {
            guild_id: None,
            id: Id::new(1),
            kind: CommandType::ChatInput,
            name: "config".into(),
            options,
            resolved: None,
            target_id: None,
        };
        assert_eq!(command_path(&data), ["config", "advanced", "set"]);
        let leaf = leaf_options(&data.options);
        assert_eq!(leaf.len(), 1);
        assert_eq!(leaf[0].name, "value");

        let plain = CommandData {
            options: vec![option("value", CommandOptionValue::Integer(3))],
            ..data
        };
        assert_eq!(command_path(&plain), ["config"]);
        assert_eq!(leaf_options(&plain.options).len(), 1);
    }

    #[test]
    fn route_finds_subcommands_by_path_and_keeps_the_root_separate() -> Result<()> {
        let mut slash = SlashCommands::for_guild(Id::new(1));
        {
            let mut config = slash.group("config", "設定")?;
            config.subcommand("set", "値を設定", vec![], handler)?;
        }
        slash.register("ping", "応答", vec![], handler)?;
        let mut routes: HashMap<_, _> = slash
            .entries
            .into_iter()
            .map(|(name, definition)| (name, definition.into_route()))
            .collect();
        let config = routes.remove("config").unwrap();
        assert!(config.find(&[]).is_none());
        assert!(config.find(&["set"]).is_some());
        assert!(config.find(&["missing"]).is_none());
        let ping = routes.remove("ping").unwrap();
        assert!(ping.find(&[]).is_some());
        assert!(ping.find(&["set"]).is_none());
        Ok(())
    }

    async fn complete(_: &mut AutocompleteContext<'_>) -> Result<()> {
        Ok(())
    }

    #[test]
    fn autocomplete_marks_the_option_and_rejects_unsupported_targets() -> Result<()> {
        let mut slash = SlashCommands::for_guild(Id::new(1));
        let mut fixed = SlashCommands::string_option("kind", "種類", true);
        fixed.choices = Some(vec![SlashCommands::string_choice("a", "a")]);
        slash.register(
            "echo",
            "説明",
            vec![
                SlashCommands::string_option("text", "本文", true),
                fixed,
                SlashCommands::boolean_option("loud", "大きく", false),
            ],
            handler,
        )?;

        slash.autocomplete(&["echo"], "text", complete)?;
        assert_eq!(slash.entries["echo"].options[0].autocomplete, Some(true));
        // 二重、存在しない引数やコマンド、真偽値、選択肢つきは拒否する。
        assert!(slash.autocomplete(&["echo"], "text", complete).is_err());
        assert!(slash.autocomplete(&["echo"], "missing", complete).is_err());
        assert!(slash.autocomplete(&["nope"], "text", complete).is_err());
        assert!(slash.autocomplete(&["echo"], "loud", complete).is_err());
        assert!(slash.autocomplete(&["echo"], "kind", complete).is_err());
        assert!(slash.autocomplete(&[], "text", complete).is_err());
        assert_eq!(slash.entries["echo"].options[2].autocomplete, None);
        Ok(())
    }

    #[test]
    fn autocomplete_reaches_options_of_subcommands_and_routes_by_path() -> Result<()> {
        let mut slash = SlashCommands::for_guild(Id::new(1));
        {
            let mut config = slash.group("config", "設定")?;
            config.subcommand(
                "set",
                "設定",
                vec![SlashCommands::string_option("key", "名前", true)],
                handler,
            )?;
            config.group("advanced", "詳細設定")?.subcommand(
                "set",
                "設定",
                vec![SlashCommands::string_option("key", "名前", true)],
                handler,
            )?;
        }
        // サブコマンドを持つコマンドの直下には付けられない。
        assert!(slash.autocomplete(&["config"], "key", complete).is_err());
        slash.autocomplete(&["config", "set"], "key", complete)?;
        slash.autocomplete(&["config", "advanced", "set"], "key", complete)?;
        let route = slash.entries.remove("config").unwrap().into_route();
        assert!(route.find_autocomplete(&["set"], "key").is_some());
        assert!(
            route
                .find_autocomplete(&["advanced", "set"], "key")
                .is_some()
        );
        assert!(route.find_autocomplete(&["set"], "other").is_none());
        assert!(route.find_autocomplete(&["advanced"], "key").is_none());
        assert!(route.find_autocomplete(&[], "key").is_none());
        Ok(())
    }

    #[tokio::test]
    async fn autocomplete_response_checks_count_and_single_use_before_sending() {
        use twilight_model::application::interaction::InteractionType;
        let mut interaction = command_interaction(Some(Id::new(1)), None, 5);
        interaction.kind = InteractionType::ApplicationCommandAutocomplete;
        let client = twilight_http::Client::new("dummy-token".to_owned());
        let info = crate::hooks::InfoReporter::default();
        let Some(InteractionData::ApplicationCommand(data)) = &interaction.data else {
            panic!("想定外の構造");
        };
        let mut ctx = AutocompleteContext {
            initial: InitialResponder {
                http: &client,
                interaction: &interaction,
                timing: crate::timing::Pending::new((), None).begin().1,
                info: &info,
            },
            data,
            responded: false,
        };
        let many = (0..26)
            .map(|index| SlashCommands::integer_choice("n", index))
            .collect();
        assert!(matches!(
            ctx.respond(many).await,
            Err(Error::TooManyChoices { count: 26, max: 25 })
        ));
        let long = SlashCommands::string_choice(&"あ".repeat(101), "v");
        assert!(matches!(
            ctx.respond(vec![long]).await,
            Err(Error::CommandValidation(_))
        ));
        ctx.responded = true;
        assert!(matches!(
            ctx.respond(vec![]).await,
            Err(Error::ResponseState(ResponseStateError::AlreadyResponded))
        ));
        assert_eq!(ctx.focused(), "");
    }

    #[test]
    fn typed_option_helpers_set_the_option_kind() {
        let kinds = [
            (
                SlashCommands::integer_option("count", "個数", true),
                CommandOptionType::Integer,
            ),
            (
                SlashCommands::number_option("ratio", "割合", false),
                CommandOptionType::Number,
            ),
            (
                SlashCommands::boolean_option("loud", "大きく", false),
                CommandOptionType::Boolean,
            ),
        ];
        for (option, kind) in kinds {
            assert_eq!(option.kind, kind);
            assert!(option.choices.is_none());
        }
    }

    #[test]
    fn validates_definitions_before_sync_and_preserves_first_registration() -> Result<()> {
        let mut slash = SlashCommands::for_guild(Id::new(1));
        assert!(slash.register("", "description", vec![], handler).is_err());
        assert!(slash.register("echo", "", vec![], handler).is_err());

        let text = SlashCommands::string_option("text", "返信する内容", true);
        assert_eq!(text.kind, CommandOptionType::String);
        assert_eq!(text.required, Some(true));
        slash.register("echo", "最初の説明", vec![text], handler)?;
        assert!(
            slash
                .register("echo", "後から渡した説明", vec![], handler)
                .is_err()
        );
        assert_eq!(slash.entries["echo"].description, "最初の説明");
        assert_eq!(slash.entries["echo"].options[0].name, "text");
        Ok(())
    }

    fn two_commands() -> Result<SlashCommands> {
        let mut slash = SlashCommands::for_guild(Id::new(1)).replace_commands();
        slash.register("echo", "echo text", vec![], handler)?;
        slash.register("ping", "return pong", vec![], handler)?;
        Ok(slash)
    }

    #[test]
    fn replacement_binds_ids_by_name_and_accepts_empty_registry() -> Result<()> {
        let empty = SlashCommands::for_guild(Id::new(1)).replace_commands();
        assert!(empty.replacement_commands()?.is_empty());
        assert!(empty.bind(Vec::new())?.handlers.is_empty());

        let slash = two_commands()?;
        let mut returned = slash.replacement_commands()?;
        assert_eq!(returned[0].name, "echo");
        returned[0].id = Some(Id::new(12));
        returned[1].id = Some(Id::new(34));
        returned.reverse();
        let active = slash.bind(returned)?;
        assert_eq!(active.handlers.len(), 2);
        assert!(active.handlers.contains_key(&Id::new(12)));
        assert!(active.handlers.contains_key(&Id::new(34)));
        Ok(())
    }

    #[test]
    fn sync_rejects_malformed_command_results() -> Result<()> {
        let slash = two_commands()?;
        let mut returned = slash.replacement_commands()?;
        returned[0].id = Some(Id::new(12));
        returned[1].id = Some(Id::new(34));

        let mut bad = returned.clone();
        bad[1].id = bad[0].id;
        assert!(two_commands()?.bind(bad).is_err());
        let mut bad = returned.clone();
        bad[1].name = bad[0].name.clone();
        assert!(two_commands()?.bind(bad).is_err());
        let mut bad = returned.clone();
        bad[0].kind = CommandType::User;
        assert!(two_commands()?.bind(bad).is_err());
        let mut bad = returned.clone();
        bad[1].id = None;
        assert!(two_commands()?.bind(bad).is_err());
        assert!(slash.bind(vec![returned.remove(0)]).is_err());
        Ok(())
    }

    #[test]
    fn response_paths_preserve_deferred_visibility_and_reject_duplicate_initial_messages()
    -> Result<()> {
        let public_defer = ResponseState::Deferred { ephemeral: false };
        let private_defer = ResponseState::Deferred { ephemeral: true };
        assert_eq!(
            ResponseState::Pending.reply_action(false)?,
            ReplyAction::Initial
        );
        assert_eq!(public_defer.reply_action(false)?, ReplyAction::EditDeferred);
        assert!(public_defer.reply_action(true).is_err());
        assert_eq!(
            private_defer.reply_action(false)?,
            ReplyAction::EditDeferred
        );
        assert_eq!(private_defer.reply_action(true)?, ReplyAction::EditDeferred);
        assert!(ResponseState::Replied.reply_action(false).is_err());
        Ok(())
    }

    #[test]
    fn builds_immediate_and_deferred_callbacks_with_ephemeral_flags() {
        let immediate = initial_response(InitialResponse::Message("busy"), true);
        assert_eq!(
            immediate.kind,
            InteractionResponseType::ChannelMessageWithSource
        );
        let data = immediate.data.unwrap();
        assert_eq!(data.content.as_deref(), Some("busy"));
        assert_eq!(data.allowed_mentions, Some(AllowedMentions::default()));
        assert_eq!(data.flags, Some(MessageFlags::EPHEMERAL));

        let deferred = initial_response(InitialResponse::Deferred, false);
        assert_eq!(
            deferred.kind,
            InteractionResponseType::DeferredChannelMessageWithSource
        );
        assert_eq!(deferred.data, None);
        let data = initial_response(InitialResponse::Deferred, true)
            .data
            .unwrap();
        assert_eq!(data.content, None);
        assert_eq!(data.flags, Some(MessageFlags::EPHEMERAL));
    }

    #[test]
    fn ui_callback_only_includes_present_message_fields() -> Result<()> {
        let rich = UiMessage::new()
            .embed(Embed::new("埋め込み"))
            .buttons(vec![Button::primary("details", "詳細")]);
        rich.validate()?;
        let response = initial_response(InitialResponse::Ui(&rich), false);
        assert_eq!(
            response.kind,
            InteractionResponseType::ChannelMessageWithSource
        );
        let data = response.data.unwrap();
        assert_eq!(data.content, None);
        assert_eq!(data.embeds.as_deref(), Some(rich.embeds.as_slice()));
        assert_eq!(data.components.as_deref(), Some(rich.components.as_slice()));
        assert_eq!(data.allowed_mentions, Some(AllowedMentions::default()));
        assert_eq!(data.flags, None);

        let private = initial_response(InitialResponse::Ui(&rich), true)
            .data
            .unwrap();
        assert_eq!(private.flags, Some(MessageFlags::EPHEMERAL));
        assert_eq!(private.embeds.as_deref(), Some(rich.embeds.as_slice()));
        assert_eq!(
            private.components.as_deref(),
            Some(rich.components.as_slice())
        );

        let plain = UiMessage::new().content("本文のみ");
        let data = initial_response(InitialResponse::Ui(&plain), false)
            .data
            .unwrap();
        assert_eq!(data.content.as_deref(), Some("本文のみ"));
        assert_eq!(data.embeds, None);
        assert_eq!(data.components, None);
        Ok(())
    }
}
