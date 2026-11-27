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
            Interaction, InteractionData, InteractionDataResolved, InteractionType,
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
    CheckFailure, CommandCheck, CommandCheckContext, CommandCheckSource, CommandSyncError, Error,
    ErrorSource, HandlerKind, InfoEvent, RegistrationError, ResponseStateError, Result, Timing,
    autocomplete::{AutocompleteContext, AutocompleteHandler, choice},
    command_check::run_checks,
    error::adapt_handler,
    hooks::{Hooks, InfoReporter},
    permissions::check_permissions,
    ui::{ComponentsV2, Modal, UiMessage},
};

mod context_menu;
mod file_response;
mod invoke;
mod localizations;
mod original_response;
mod settings;
mod v2_attachments;
mod validation;
pub use context_menu::UserMenuTarget;
use context_menu::{MenuCommands, MenuKind, replace_application_commands};
pub(crate) use file_response::send_followup_files;
pub(crate) use invoke::InvocationHooks;
use invoke::{PreparedSlashHandler, slash_handler};
pub use localizations::CommandOptionLocalizations;
use localizations::validate_distinct_names;
pub(crate) use localizations::validate_options;
pub use settings::CommandSettings;
use settings::{SettingsByKind, create_configured_command};
pub(crate) use v2_attachments::edit_v2_response;
use validation::validate_command_size;

/// 一つのサーバー、またはアプリ全体に登録するアプリコマンド一覧です。
///
/// スラッシュコマンドと、ユーザー・メッセージのコンテキストメニューを登録できます。
pub struct SlashCommands {
    scope: Scope,
    entries: BTreeMap<String, Definition>,
    menus: MenuCommands,
    settings: SettingsByKind,
    checks: Vec<CommandCheck>,
    invocation_hooks: InvocationHooks,
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
            .field("menus", &self.menus.names().collect::<Vec<_>>())
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
            menus: MenuCommands::default(),
            settings: SettingsByKind::default(),
            checks: Vec::new(),
            invocation_hooks: InvocationHooks::default(),
            replace: false,
        }
    }

    /// この一覧で実行するアプリコマンドと補完にチェックを追加します。
    /// 登録順に実行し、最初のエラーで処理を止めます。補完で失敗した場合は空の候補を返します。
    pub fn add_check(&mut self, check: CommandCheck) {
        self.checks.push(check);
    }

    /// アプリコマンドの関数の直前に呼ぶ非同期フックを設定します。以前の設定は置き換えます。
    /// 実行前のチェック、型付き引数の変換、メニューの選択対象の解決を通過した場合に呼びます。
    /// フックの失敗時はコマンド関数と実行後フックを呼びません。補完には適用しません。
    pub fn set_before_invoke(
        &mut self,
        hook: impl AsyncFn(&SlashContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) {
        self.invocation_hooks.set_before(hook);
    }

    /// アプリコマンドの関数の直後に呼ぶ非同期フックを設定します。以前の設定は置き換えます。
    /// 関数がエラーや panic で終わった場合も呼び、そのエラーを渡します。両方が失敗した場合は関数のエラーを優先します。
    /// 補完には適用しません。Future が破棄される場合の後始末には `Drop` を使ってください。
    pub fn set_after_invoke(
        &mut self,
        hook: impl AsyncFn(&SlashContext<'_>, Option<&Error>) -> Result<()> + Send + Sync + 'static,
    ) {
        self.invocation_hooks.set_after(hook);
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
            Some(slash_handler(handler)),
            PermissionPolicy::Unrestricted,
        )?;
        Ok(())
    }

    pub(crate) fn register_prepared(
        &mut self,
        name: &str,
        description: &str,
        options: Vec<CommandOption>,
        handler: impl AsyncFn(&mut SlashContext<'_>, &InvocationHooks) -> Result<()>
        + Send
        + Sync
        + 'static,
    ) -> Result<()> {
        self.insert(
            name,
            description,
            options,
            Some(Box::new(PreparedSlashHandler(handler))),
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
        validate_options(&options)?;
        validate_command_size(name, None, description, None, &options)?;
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

    pub(crate) fn ensure_hybrid_group_available(&self, name: &str) -> Result<()> {
        if self.entries.contains_key(name) {
            return Err(duplicate_sub(name));
        }
        Ok(())
    }

    pub(crate) fn stage_hybrid_group(&self) -> Self {
        Self::new(self.scope)
    }

    pub(crate) fn validate_staged_hybrid_group(&self, name: &str) -> Result<()> {
        let root = self
            .entries
            .get(name)
            .ok_or_else(|| RegistrationError::UnknownCommand { name: name.into() })?;
        if root.children.is_empty() {
            return Err(RegistrationError::EmptyHybridGroup { name: name.into() }.into());
        }
        for (child_name, child) in &root.children {
            if matches!(child, Child::Group { subs, .. } if subs.is_empty()) {
                return Err(RegistrationError::EmptyHybridGroup {
                    name: format!("{name} {child_name}"),
                }
                .into());
            }
        }
        self.chat_model(name, root)?;
        Ok(())
    }

    pub(crate) fn append_staged_hybrid_group(&mut self, staged: Self) {
        self.entries.extend(staged.entries);
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
            Some(slash_handler(handler)),
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

    /// コマンドの実行に必要な権限を宣言します。ハンドラーの前に、実行者と Bot の権限を確認します。
    ///
    /// 足りなければ、ハンドラーを実行せず、[`CheckFailure::MissingPermissions`] か
    /// [`CheckFailure::BotMissingPermissions`] で失敗します。補完にも適用されます。
    /// グループやコマンドを指定すると、その配下のサブコマンドにも適用されます。
    /// 実行者の権限は、最上位のコマンドで、Discord 側の既定の表示条件にも反映します（すでに
    /// [`default_member_permissions`](Self::default_member_permissions) などで設定済みなら、変えません）。
    /// コンテキストメニューには使えません。[`add_check`](Self::add_check) を使ってください。
    /// [`register`](Self::register) のあとに呼びます。
    ///
    /// # エラー
    ///
    /// 登録されていないコマンドやサブコマンド（空白区切り）を指定した場合に失敗します。
    pub fn require(&mut self, path: &str, needs: crate::Needs) -> Result<()> {
        let parts: Vec<String> = path.split_whitespace().map(str::to_owned).collect();
        let unknown = || RegistrationError::UnknownCommand {
            name: path.to_owned(),
        };
        let (root, rest) = parts.split_first().ok_or_else(unknown)?;
        let definition = self.entries.get_mut(root).ok_or_else(unknown)?;
        if !definition.has_path(rest) {
            return Err(unknown().into());
        }
        if rest.is_empty()
            && !needs.author_permissions().is_empty()
            && matches!(definition.permissions, PermissionPolicy::Unrestricted)
        {
            definition.permissions = PermissionPolicy::Visibility(needs.author_permissions());
        }
        self.checks.push(needs.into_check(parts));
        Ok(())
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

    /// ユーザーの選択肢を、ユーザー ID として受け取るための引数を作ります。
    pub fn user_option(name: &str, description: &str, required: bool) -> CommandOption {
        Self::typed_option(CommandOptionType::User, name, description, required)
    }

    /// ロールの選択肢を、ロール ID として受け取るための引数を作ります。
    pub fn role_option(name: &str, description: &str, required: bool) -> CommandOption {
        Self::typed_option(CommandOptionType::Role, name, description, required)
    }

    /// チャンネルの選択肢を、チャンネル ID として受け取るための引数を作ります。
    pub fn channel_option(name: &str, description: &str, required: bool) -> CommandOption {
        Self::typed_option(CommandOptionType::Channel, name, description, required)
    }

    /// ユーザーまたはロールを一つ選ぶ引数を作ります。
    pub fn mentionable_option(name: &str, description: &str, required: bool) -> CommandOption {
        Self::typed_option(CommandOptionType::Mentionable, name, description, required)
    }

    /// 添付ファイルを受け取る引数を作ります。
    pub fn attachment_option(name: &str, description: &str, required: bool) -> CommandOption {
        Self::typed_option(CommandOptionType::Attachment, name, description, required)
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
        if self.entries.is_empty() && self.menus.is_empty() && !self.replace {
            return self.bind(Vec::new());
        }

        let commands = self.replacement_commands()?;
        let application = http.current_user_application().await?.model().await?;
        let client = http.interaction(application.id);
        let registered = if self.replace {
            if self.menus.is_empty() {
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
                replace_application_commands(http, application.id, self.scope, &commands).await?
            }
        } else {
            let mut registered = Vec::with_capacity(commands.len());
            for model in &commands {
                let name = &model.name;
                if model.kind != CommandType::ChatInput
                    || self
                        .settings
                        .get(&model.kind)
                        .is_some_and(|settings| settings.contains_key(name))
                {
                    let command =
                        create_configured_command(http, application.id, self.scope, model).await?;
                    if model.kind != CommandType::ChatInput
                        && (command.kind != model.kind || command.name != *name)
                    {
                        return Err(CommandSyncError::UnexpectedName { name: command.name }.into());
                    }
                    registered.push(command);
                    continue;
                }
                let registration = |source| Error::CommandRegistration {
                    name: name.clone(),
                    source,
                };
                let command = match self.scope {
                    Scope::Guild(guild_id) => {
                        let mut request = client
                            .create_guild_command(guild_id)
                            .chat_input(name, &model.description)
                            .command_options(&model.options);
                        if let Some(permissions) = model.default_member_permissions {
                            request = request.default_member_permissions(permissions);
                        }
                        request.await
                    }
                    Scope::Global => {
                        let mut request = client
                            .create_global_command()
                            .chat_input(name, &model.description)
                            .command_options(&model.options);
                        if let Some(permissions) = model.default_member_permissions {
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

    fn replacement_commands(&self) -> Result<Vec<Command>> {
        self.entries
            .iter()
            .map(|(name, definition)| self.chat_model(name, definition))
            .chain(
                self.menus
                    .names()
                    .map(|(kind, name)| self.menu_model(kind, name)),
            )
            .collect()
    }

    fn chat_model(&self, name: &str, definition: &Definition) -> Result<Command> {
        let mut command = command_model(
            CommandType::ChatInput,
            name,
            &definition.description,
            definition.option_tree(),
            definition.permissions.discord_default(),
        );
        self.configure_command(&mut command);
        validate_options(&command.options)?;
        validate_command_size(
            &command.name,
            command.name_localizations.as_ref(),
            &command.description,
            command.description_localizations.as_ref(),
            &command.options,
        )?;
        Ok(command)
    }

    fn menu_model(&self, kind: MenuKind, name: &str) -> Result<Command> {
        let mut command = command_model(kind.into(), name, "", Vec::new(), None);
        self.configure_command(&mut command);
        command::command(&command)?;
        Ok(command)
    }

    fn bind(mut self, registered: Vec<Command>) -> Result<ActiveSlashCommands> {
        let expected = self.entries.len() + self.menus.len();
        if registered.len() != expected {
            return Err(CommandSyncError::CountMismatch {
                expected,
                returned: registered.len(),
            }
            .into());
        }

        let only_chat_input = self.menus.is_empty();
        let mut handlers = HashMap::with_capacity(registered.len());
        for command in registered {
            let name = command.name;
            if only_chat_input && command.kind != CommandType::ChatInput {
                return Err(CommandSyncError::NotChatInput { name }.into());
            }
            let id = command
                .id
                .ok_or_else(|| CommandSyncError::MissingId { name: name.clone() })?;
            let route = if command.kind == CommandType::ChatInput {
                self.entries
                    .remove(&name)
                    .map(|definition| definition.into_route(name.clone()))
            } else {
                MenuKind::from_command_type(command.kind)
                    .and_then(|kind| self.menus.remove(kind, &name))
                    .map(|handler| Route::menu(name.clone(), command.kind, handler))
            }
            .ok_or_else(|| CommandSyncError::UnexpectedName { name: name.clone() })?;
            if let std::collections::hash_map::Entry::Vacant(entry) = handlers.entry(id) {
                entry.insert(route);
            } else {
                return Err(CommandSyncError::DuplicateId { name }.into());
            }
        }

        Ok(ActiveSlashCommands {
            scope: self.scope,
            handlers,
            checks: self.checks,
            invocation_hooks: self.invocation_hooks,
        })
    }
}

#[allow(
    deprecated,
    reason = "Twilight の Command モデルに含まれる dm_permission は送信しない。"
)]
fn command_model(
    kind: CommandType,
    name: &str,
    description: &str,
    options: Vec<CommandOption>,
    default_member_permissions: Option<Permissions>,
) -> Command {
    Command {
        application_id: None,
        contexts: None,
        default_member_permissions,
        description: description.into(),
        description_localizations: None,
        dm_permission: None,
        guild_id: None,
        id: None,
        integration_types: None,
        kind,
        name: name.into(),
        name_localizations: None,
        nsfw: None,
        options,
        version: Id::new(1),
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
    /// この階層にある登録済みのサブコマンドまたは中間グループの翻訳を置き換えます。
    /// `name` には翻訳前の名前を指定します。空の設定で翻訳を消せます。
    ///
    /// # エラー
    /// 指定した子がない、翻訳が無効、または同じ階層の名前と衝突した場合に失敗します。
    /// 失敗した場合は以前の翻訳を維持します。
    pub fn set_child_localizations(
        &mut self,
        name: &str,
        localizations: CommandOptionLocalizations,
    ) -> Result<()> {
        localizations.validate()?;
        let unknown = || RegistrationError::UnknownCommand {
            name: name.to_owned(),
        };
        if let Some(group) = &self.group {
            let Some(Child::Group { subs, .. }) = self.root.children.get_mut(group) else {
                return Err(unknown().into());
            };
            validate_distinct_names(
                name,
                localizations.names(),
                subs.iter()
                    .map(|(sibling, sub)| (sibling.as_str(), sub.localizations.names())),
            )?;
            subs.get_mut(name).ok_or_else(unknown)?.localizations = localizations;
        } else {
            validate_distinct_names(
                name,
                localizations.names(),
                self.root
                    .children
                    .iter()
                    .map(|(sibling, child)| (sibling.as_str(), child.localizations().names())),
            )?;
            *self
                .root
                .children
                .get_mut(name)
                .ok_or_else(unknown)?
                .localizations_mut() = localizations;
        }
        Ok(())
    }

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
        self.insert_subcommand(name, description, options, slash_handler(handler))
    }

    pub(crate) fn subcommand_prepared(
        &mut self,
        name: &str,
        description: &str,
        options: Vec<CommandOption>,
        handler: impl AsyncFn(&mut SlashContext<'_>, &InvocationHooks) -> Result<()>
        + Send
        + Sync
        + 'static,
    ) -> Result<()> {
        self.insert_subcommand(
            name,
            description,
            options,
            Box::new(PreparedSlashHandler(handler)),
        )
    }

    fn insert_subcommand(
        &mut self,
        name: &str,
        description: &str,
        options: Vec<CommandOption>,
        handler: Box<dyn SlashHandler>,
    ) -> Result<()> {
        command::chat_input_name(name)?;
        command::description(description)?;
        validate_options(&options)?;
        let definition = SubDefinition {
            description: description.into(),
            options,
            localizations: CommandOptionLocalizations::default(),
            handlers: RouteHandlers {
                command: Some(handler),
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
        validate_distinct_names(
            name,
            None,
            siblings
                .iter()
                .map(|(sibling, child)| (sibling.as_str(), child.localizations().names())),
        )?;
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
        validate_distinct_names(
            name,
            None,
            self.root
                .children
                .iter()
                .map(|(sibling, child)| (sibling.as_str(), child.localizations().names())),
        )?;
        match self.root.children.entry(name.into()) {
            Entry::Vacant(entry) => {
                entry.insert(Child::Group {
                    description: description.into(),
                    localizations: CommandOptionLocalizations::default(),
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
    validate_distinct_names(
        name,
        None,
        subs.iter()
            .map(|(sibling, sub)| (sibling.as_str(), sub.localizations.names())),
    )?;
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

/// コマンド名に、呼ばれたサブコマンドとグループの名前を続けた経路。
pub(crate) fn command_path(data: &CommandData) -> Vec<&str> {
    let mut path = vec![data.name.as_str()];
    let mut options = data.options.as_slice();
    while let Some((name, inner)) = next_subcommand(options) {
        path.push(name);
        options = inner;
    }
    path
}

/// サブコマンドの入れ子をたどった先の、実際の引数。
pub(crate) fn leaf_options(mut options: &[CommandDataOption]) -> &[CommandDataOption] {
    while let Some((_, inner)) = next_subcommand(options) {
        options = inner;
    }
    options
}

fn next_subcommand(options: &[CommandDataOption]) -> Option<(&str, &[CommandDataOption])> {
    let [option] = options else { return None };
    match &option.value {
        CommandOptionValue::SubCommand(inner) | CommandOptionValue::SubCommandGroup(inner) => {
            Some((&option.name, inner))
        }
        _ => None,
    }
}

fn routing_path<'data, 'path>(
    mut options: &'data [CommandDataOption],
    path: &'path mut [&'data str; 2],
) -> Option<&'path [&'data str]> {
    // Discord の最上位コマンドからは、グループとサブコマンドの2段までたどれる。
    let mut depth = 0;
    while let Some((name, inner)) = next_subcommand(options) {
        *path.get_mut(depth)? = name;
        depth += 1;
        options = inner;
    }
    path.get(..depth)
}

fn report_command_error(hooks: &Hooks, source: ErrorSource, data: &CommandData, error: &Error) {
    if next_subcommand(&data.options).is_none() {
        hooks.reporter.report(source, Some(&data.name), error);
    } else {
        hooks
            .reporter
            .report(source, Some(&command_path(data).join(" ")), error);
    }
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

pub(crate) fn interaction_permissions(interaction: &Interaction) -> Option<Permissions> {
    interaction
        .member
        .as_ref()
        .and_then(|member| member.permissions)
}

#[allow(
    deprecated,
    reason = "古い形式のペイロードでは、channel の代わりに channel_id が送られる。"
)]
pub(crate) fn interaction_channel_id(interaction: &Interaction) -> Option<Id<ChannelMarker>> {
    interaction
        .channel
        .as_ref()
        .map(|channel| channel.id)
        .or(interaction.channel_id)
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

impl Definition {
    /// 最上位のコマンドの下に、指定したサブコマンドやグループがあるか。
    fn has_path(&self, rest: &[String]) -> bool {
        match rest {
            [] => true,
            [name] => self.children.contains_key(name),
            [group, sub] => matches!(
                self.children.get(group),
                Some(Child::Group { subs, .. }) if subs.contains_key(sub)
            ),
            _ => false,
        }
    }
}

enum Child {
    Sub(SubDefinition),
    Group {
        description: String,
        localizations: CommandOptionLocalizations,
        subs: BTreeMap<String, SubDefinition>,
    },
}

impl Child {
    fn localizations(&self) -> &CommandOptionLocalizations {
        match self {
            Self::Sub(sub) => &sub.localizations,
            Self::Group { localizations, .. } => localizations,
        }
    }

    fn localizations_mut(&mut self) -> &mut CommandOptionLocalizations {
        match self {
            Self::Sub(sub) => &mut sub.localizations,
            Self::Group { localizations, .. } => localizations,
        }
    }
}

struct SubDefinition {
    description: String,
    options: Vec<CommandOption>,
    localizations: CommandOptionLocalizations,
    handlers: RouteHandlers,
}

impl Definition {
    /// Discord に送る引数の一覧。サブコマンドを持つ場合は、その入れ子を作る。
    fn option_tree(&self) -> Vec<CommandOption> {
        if self.children.is_empty() {
            return self.options.clone();
        }
        let nested = |kind,
                      name: &String,
                      description: &str,
                      options: Vec<CommandOption>,
                      localizations: &CommandOptionLocalizations| {
            let mut option = SlashCommands::typed_option(kind, name, description, false);
            option.required = None;
            option.options = Some(options);
            localizations.apply_borrowed(&mut option);
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
                    &sub.localizations,
                ),
                Child::Group {
                    description,
                    localizations,
                    subs,
                } => nested(
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
                                &sub.localizations,
                            )
                        })
                        .collect(),
                    localizations,
                ),
            })
            .collect()
    }

    /// 受信側で、呼ぶ関数を引くための表に変える。
    fn into_route(self, name: String) -> Route {
        Route {
            name,
            kind: CommandType::ChatInput,
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
    name: String,
    kind: CommandType,
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
    fn menu(name: String, kind: CommandType, handler: Box<dyn SlashHandler>) -> Self {
        Self {
            name,
            kind,
            root: RouteHandlers {
                command: Some(handler),
                autocompletes: HashMap::new(),
            },
            children: HashMap::new(),
            permissions: PermissionPolicy::Unrestricted,
        }
    }

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
    checks: Vec<CommandCheck>,
    invocation_hooks: InvocationHooks,
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
        let supported = data.kind == CommandType::ChatInput
            || (interaction.kind == InteractionType::ApplicationCommand
                && MenuKind::from_command_type(data.kind).is_some());
        (supported && in_scope).then_some(data)
    }

    fn route(&self, data: &CommandData) -> Option<&Route> {
        self.handlers
            .get(&data.id)
            .filter(|route| route.kind == data.kind)
    }

    async fn dispatch_autocomplete(
        &self,
        initial: InitialResponder<'_>,
        data: &CommandData,
        path: Option<&[&str]>,
        hooks: &Hooks,
    ) {
        let mut ctx = AutocompleteContext {
            initial,
            data,
            response_finished: false,
        };
        let handler = path.and_then(|path| {
            self.route(data)
                .filter(|route| route.permissions.check(initial.interaction).is_ok())
                .and_then(|route| {
                    route
                        .find_autocomplete(path, ctx.focused_name())
                        .map(|handler| (route, path, handler))
                })
        });
        if let Some((route, path, handler)) = handler {
            let checked = self
                .check_invocation(initial, route, path, CommandCheckSource::Autocomplete)
                .await;
            let result = match checked {
                Ok(()) => handler.call(&mut ctx).await,
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                report_command_error(hooks, ErrorSource::SlashCommand, data, &error);
            }
        }
        // 権限や関数がない場合も、空の候補を返す。
        if let Err(error) = ctx.respond_empty_if_needed().await {
            report_command_error(hooks, ErrorSource::Response, data, &error);
        }
    }

    async fn check_invocation(
        &self,
        initial: InitialResponder<'_>,
        route: &Route,
        path: &[&str],
        source: CommandCheckSource,
    ) -> Result<()> {
        if !self.checks.is_empty() {
            let access = crate::Access::for_interaction(
                initial.http,
                initial.permissions,
                initial.interaction,
            );
            run_checks(
                &self.checks,
                CommandCheckContext::application(
                    initial.interaction,
                    access,
                    source,
                    &route.name,
                    path,
                ),
            )
            .await?;
        }
        Ok(())
    }

    async fn call_command(
        &self,
        ctx: &mut SlashContext<'_>,
        route: &Route,
        path: &[&str],
        handler: &dyn SlashHandler,
    ) -> Result<()> {
        route.permissions.check(ctx.initial.interaction)?;
        self.check_invocation(
            ctx.initial,
            route,
            path,
            CommandCheckSource::Application(route.kind),
        )
        .await?;
        handler.call(ctx, &self.invocation_hooks).await
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
        let mut path_buffer = [""; 2];
        let path = routing_path(&data.options, &mut path_buffer);
        let initial = InitialResponder {
            http,
            interaction,
            timing,
            info: &hooks.info,
            permissions: &hooks.permissions,
        };
        if interaction.kind == InteractionType::ApplicationCommandAutocomplete {
            self.dispatch_autocomplete(initial, data, path, hooks).await;
            return;
        }
        let Some((route, path, handler)) = path.and_then(|path| {
            self.route(data)
                .and_then(|route| route.find(path).map(|handler| (route, path, handler)))
        }) else {
            report_command_error(
                hooks,
                ErrorSource::SlashCommand,
                data,
                &Error::CommandUnavailable,
            );
            if let Some(text) = &hooks.messages.slash_command_unavailable
                && let Err(error) = initial.message(text, true).await
            {
                report_command_error(hooks, ErrorSource::Response, data, &error);
            }
            return;
        };

        let mut response_state = ResponseState::Pending;
        let mut ctx = SlashContext {
            initial,
            data,
            response_state: &mut response_state,
        };
        let result = self.call_command(&mut ctx, route, path, handler).await;
        let (check_failed, failed_text) = match result {
            Ok(()) => (false, &hooks.messages.slash_command_failed),
            Err(error) => {
                report_command_error(hooks, ErrorSource::SlashCommand, data, &error);
                if *ctx.response_state == ResponseState::Pending && callback_unavailable(&error) {
                    return;
                }
                match &error {
                    Error::Check(failure) => (true, hooks.messages.for_check(failure)),
                    _ => (false, &hooks.messages.slash_command_failed),
                }
            }
        };
        // 応答が済んでいない（エラー、または応答の忘れ）ときは、文言が設定されていれば知らせる。
        let Some(text) = failed_text else {
            return;
        };
        if let Err(error) = ctx.reply_fallback(text, check_failed).await {
            report_command_error(hooks, ErrorSource::Response, data, &error);
        }
    }
}

pub(crate) fn callback_unavailable(error: &Error) -> bool {
    use twilight_http::{api_error::ApiError, error::ErrorType};

    // Discord が操作を失効済み・応答済みと判定した場合、初回応答を繰り返しても届きません。
    matches!(error, Error::Http(error) if matches!(
        error.kind(),
        ErrorType::Response { error: ApiError::General(response), .. }
            if matches!(response.code, 10062 | 40060)
    ))
}

/// 一回のスラッシュコマンドまたはコンテキストメニューの呼び出しを扱います。
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
            ResponseState::Replied
            | ResponseState::RepliedV2
            | ResponseState::OriginalDeleted { .. }
            | ResponseState::ModalOpened => Ok(()),
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

    /// Discord で呼び出したユーザーが選んでいるロケールを返します。
    pub fn locale(&self) -> Option<&str> {
        self.initial.interaction.locale.as_deref()
    }

    /// Discord が通知したサーバーのロケールを返します。
    pub fn guild_locale(&self) -> Option<&str> {
        self.initial.interaction.guild_locale.as_deref()
    }

    /// 権限の確認や、Bot とほかのメンバーの権限の取得に使う [`Access`](crate::Access) を返します。
    ///
    /// 実行者と Bot の権限は Discord が送るので、通信は要りません。
    pub fn access(&self) -> crate::Access<'a> {
        crate::Access::for_interaction(
            self.initial.http,
            self.initial.permissions,
            self.initial.interaction,
        )
    }

    /// メンバー、ロール、VC の管理操作に使う [`Manage`](crate::Manage) を返します。
    pub fn manage(&self) -> crate::Manage<'a> {
        crate::Manage::new(self.access())
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
    pub fn channel_id(&self) -> Option<Id<ChannelMarker>> {
        interaction_channel_id(self.initial.interaction)
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
        self.leaf_options()
            .iter()
            .find(|option| option.name == name)
            .map(|option| &option.value)
    }

    pub(crate) fn leaf_options(&self) -> &'a [CommandDataOption] {
        leaf_options(&self.data.options)
    }

    /// Discord がこのコマンドと一緒に送った、解決済みの添付ファイルやユーザーなどの情報を借ります。
    pub fn resolved(&self) -> Option<&'a InteractionDataResolved> {
        self.data.resolved.as_ref()
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
        self.initial.modal(modal).await?;
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
                self.initial.v2_reply(message, ephemeral).await?;
            }
            ReplyAction::EditDeferred => {
                edit_v2_response(
                    self.initial.http,
                    self.initial.interaction,
                    message,
                    Some(&[]),
                )
                .await?
            }
        }
        *self.response_state = ResponseState::RepliedV2;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResponseState {
    Pending,
    Deferred { ephemeral: bool },
    Replied,
    RepliedV2,
    OriginalDeleted { completed: bool },
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
            Self::Replied | Self::RepliedV2 | Self::OriginalDeleted { .. } | Self::ModalOpened => {
                Err(ResponseStateError::AlreadyResponded.into())
            }
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
        self.require_followup()?;
        send_followup_message(http, interaction, content, ephemeral).await
    }

    pub(crate) fn require_followup(&self) -> Result<()> {
        // 遅延応答の直後の followup は、Discord が元の応答の編集として扱うことがある。
        if !matches!(
            self,
            Self::Replied | Self::RepliedV2 | Self::OriginalDeleted { completed: true }
        ) {
            return Err(ResponseStateError::FollowupBeforeResponse.into());
        }
        Ok(())
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

#[derive(Clone, Copy)]
pub(crate) struct InitialResponder<'a> {
    pub(crate) http: &'a Client,
    pub(crate) interaction: &'a Interaction,
    pub(crate) timing: Timing,
    pub(crate) info: &'a InfoReporter,
    pub(crate) permissions: &'a crate::permissions::PermissionResolver,
}

impl InitialResponder<'_> {
    pub(crate) async fn message(self, content: &str, ephemeral: bool) -> Result<()> {
        crate::validation::message_content(content)?;
        let response = initial_response(InitialResponse::Message(content), ephemeral);
        self.send(&response).await
    }

    pub(crate) async fn send(self, response: &InteractionResponse) -> Result<()> {
        let started_at = self.info.measures_timing().then(Instant::now);
        let result = self
            .http
            .interaction(self.interaction.application_id)
            .create_response(self.interaction.id, &self.interaction.token, response)
            .await;
        self.report_response(response.kind, started_at, result)
    }

    pub(crate) fn report_response<T>(
        self,
        response_type: InteractionResponseType,
        started_at: Option<Instant>,
        result: std::result::Result<T, twilight_http::Error>,
    ) -> Result<()> {
        if let Some(started_at) = started_at {
            let completed_at = Instant::now();
            let elapsed = self.timing.elapsed_since_receive(completed_at);
            let rest_elapsed = completed_at.saturating_duration_since(started_at);
            let interaction_type = self.interaction.kind;
            let event = match &result {
                Ok(_) => InfoEvent::InteractionResponded {
                    interaction_type,
                    response_type,
                    elapsed,
                    rest_elapsed,
                },
                Err(error) => {
                    use twilight_http::{api_error::ApiError, error::ErrorType};
                    let (http_status, discord_code) = match error.kind() {
                        ErrorType::Response { status, error, .. } => (
                            Some(status.get()),
                            match error {
                                ApiError::General(error) => Some(error.code),
                                _ => None,
                            },
                        ),
                        _ => (None, None),
                    };
                    let command_id = match &self.interaction.data {
                        Some(InteractionData::ApplicationCommand(command)) => Some(command.id),
                        _ => None,
                    };
                    InfoEvent::InteractionResponseFailed {
                        interaction_id: self.interaction.id,
                        command_id,
                        interaction_type,
                        response_type,
                        elapsed,
                        queue_wait: self.timing.queue_wait(),
                        rest_elapsed,
                        http_status,
                        discord_code,
                    }
                }
            };
            self.info.report(event);
        }
        result?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "slash/response_diagnostics_tests.rs"]
pub(crate) mod response_diagnostics_tests;

async fn edit_text_response(http: &Client, interaction: &Interaction, content: &str) -> Result<()> {
    crate::validation::message_content(content)?;
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
    crate::validation::message_content(content)?;
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
        hooks: &'a InvocationHooks,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>>;
}

impl<F> SlashHandler for F
where
    F: AsyncFn(&mut SlashContext<'_>) -> Result<()> + Send + Sync,
{
    fn call<'a, 'ctx>(
        &'a self,
        ctx: &'a mut SlashContext<'ctx>,
        hooks: &'a InvocationHooks,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        Box::pin(hooks.run(ctx, self))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;

    use super::{
        ActiveSlashCommands, AllowedMentions, AutocompleteContext, CommandData, CommandDataOption,
        CommandOptionType, CommandOptionValue, CommandType, Error, GuildMarker, Id,
        InitialResponder, InitialResponse, Interaction, InteractionData, InteractionResponseType,
        MessageFlags, PermissionPolicy, Permissions, ReplyAction, ResponseState,
        ResponseStateError, Result, Route, Scope, SlashCommands, SlashContext, command_path,
        initial_response, leaf_options, routing_path, v2_update_response,
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
    pub(super) fn command_interaction(
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

    pub(super) fn active(scope: Scope) -> ActiveSlashCommands {
        let mut handlers = HashMap::new();
        handlers.insert(
            Id::new(5),
            Route {
                name: "ping".into(),
                kind: CommandType::ChatInput,
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
        ActiveSlashCommands {
            scope,
            handlers,
            checks: Vec::new(),
            invocation_hooks: super::InvocationHooks::default(),
        }
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
        exchange_response(listener, method, None, None).await
    }

    pub(crate) async fn exchange_response(
        listener: &tokio::net::TcpListener,
        method: &str,
        path_suffix: Option<&str>,
        response: Option<&str>,
    ) -> std::io::Result<String> {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

        let (stream, _) = listener.accept().await?;
        let mut stream = BufReader::new(stream);
        let mut line = String::new();
        stream.read_line(&mut line).await?;
        let mut request = line.split_whitespace();
        assert_eq!(request.next(), Some(method));
        if let Some(suffix) = path_suffix {
            let uri = request.next().unwrap();
            let path = uri.split_once('?').map_or(uri, |(path, _)| path);
            assert!(path.ends_with(suffix), "{line}");
        }
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
        if let Some(response) = response {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response.len()
            );
            stream.get_mut().write_all(head.as_bytes()).await?;
            stream.get_mut().write_all(response.as_bytes()).await?;
        } else {
            stream
                .get_mut()
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .await?;
        }
        Ok(String::from_utf8(body).unwrap())
    }

    #[tokio::test]
    async fn manages_a_private_original_response_through_discord_routes() -> Result<()> {
        use serde_json::{Value, json};
        use std::time::Duration;
        use tokio::{net::TcpListener, time::timeout};

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(Error::handler)?;
        let http = Client::builder()
            .proxy(
                listener.local_addr().map_err(Error::handler)?.to_string(),
                true,
            )
            .build();
        let mut commands = active(Scope::Global);
        commands.handlers.get_mut(&Id::new(5)).unwrap().root.command =
            Some(Box::new(async |ctx: &mut SlashContext<'_>| -> Result<()> {
                assert!(matches!(
                    ctx.original_response().await,
                    Err(Error::ResponseState(
                        ResponseStateError::OriginalResponseUnavailable
                    ))
                ));
                ctx.defer_ephemeral().await?;
                ctx.edit_original_response("<@123>").await?;
                assert_eq!(ctx.original_response().await?.id.get(), 7);
                let v2 = ComponentsV2::new().text("不一致");
                assert!(matches!(
                    ctx.edit_original_v2(&v2).await,
                    Err(Error::ResponseState(
                        ResponseStateError::OriginalResponseFormatMismatch
                    ))
                ));
                ctx.delete_original_response().await?;
                assert!(matches!(
                    ctx.delete_original_response().await,
                    Err(Error::ResponseState(
                        ResponseStateError::OriginalResponseUnavailable
                    ))
                ));
                ctx.followup_ephemeral("deleted").await
            }));
        let mut interaction = command_interaction(None, None, 5);
        interaction.token = "test-token".into();
        let hooks = Hooks::default();
        let (_, timing) = crate::timing::Pending::new((), None).begin();
        let server = async {
            let callback: Value = serde_json::from_str(
                &exchange_response(
                    &listener,
                    "POST",
                    Some("/interactions/1/test-token/callback"),
                    None,
                )
                .await?,
            )
            .unwrap();
            assert_eq!(callback["data"]["flags"], 64);
            let original = Some("/webhooks/10/test-token/messages/@original");
            let patch: Value =
                serde_json::from_str(&exchange_response(&listener, "PATCH", original, None).await?)
                    .unwrap();
            assert_eq!(patch["content"], "<@123>");
            assert_eq!(patch["allowed_mentions"]["parse"], json!([]));
            assert!(patch.get("flags").is_none());
            assert!(patch.get("attachments").is_none());
            let payload = json!({
                "id": "7", "channel_id": "2", "type": 0,
                "author": {"id": "4", "username": "bot", "discriminator": "0"},
                "content": "<@123>", "timestamp": "2026-01-01T00:00:00.000000+00:00",
                "attachments": [], "embeds": [], "mention_everyone": false, "mention_roles": [],
                "mentions": [], "pinned": false, "tts": false, "flags": 64
            })
            .to_string();
            exchange_response(&listener, "GET", original, Some(&payload)).await?;
            exchange_response(&listener, "DELETE", original, None).await?;
            let followup: Value = serde_json::from_str(
                &exchange_response(&listener, "POST", Some("/webhooks/10/test-token"), None)
                    .await?,
            )
            .unwrap();
            assert_eq!(followup["flags"], 64);
            assert_eq!(followup["content"], "deleted");
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

    #[tokio::test]
    async fn deleted_defer_does_not_unlock_followup_visibility() {
        let pending = ResponseState::OriginalDeleted { completed: false };
        let interaction = command_interaction(None, None, 5);
        let http = Client::new("test".into());
        assert!(matches!(
            pending.followup(&http, &interaction, "private", true).await,
            Err(Error::ResponseState(
                ResponseStateError::FollowupBeforeResponse
            ))
        ));
    }

    #[tokio::test]
    async fn protected_subcommand_checks_permissions_for_execution_and_autocomplete() -> Result<()>
    {
        use twilight_model::application::interaction::InteractionType;

        let guild_id = Id::new(1);
        let command_id = Id::new(5);
        let mut slash = SlashCommands::for_guild(guild_id).replace_commands();
        let invocations = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let before = std::sync::Arc::clone(&invocations);
        slash.set_before_invoke(async move |_: &SlashContext<'_>| {
            before.lock().unwrap().push("before");
            Ok(())
        });
        let after = std::sync::Arc::clone(&invocations);
        slash.set_after_invoke(async move |_: &SlashContext<'_>, error: Option<&Error>| {
            assert!(error.is_none());
            after.lock().unwrap().push("after");
            Ok(())
        });
        let custom_checks = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let calls = std::sync::Arc::clone(&custom_checks);
        slash.add_check(crate::CommandCheck::new(
            async move |ctx: crate::CommandCheckContext<'_>| {
                assert!(ctx.command_path().eq(["ping", "advanced", "run"]));
                calls.lock().unwrap().push(ctx.source());
                ctx.require_permissions(Permissions::MANAGE_GUILD).await
            },
        ));
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
        assert_eq!(
            *custom_checks.lock().unwrap(),
            [
                crate::CommandCheckSource::Application(CommandType::ChatInput),
                crate::CommandCheckSource::Autocomplete,
            ]
        );
        assert_eq!(*invocations.lock().unwrap(), ["before", "after"]);
        Ok(())
    }

    #[tokio::test]
    async fn custom_check_rejects_invocation_before_handler_and_hides_suggestions() -> Result<()> {
        use crate::{CheckFailure, CommandCheck, CommandCheckContext};
        use twilight_model::application::interaction::InteractionType;

        let mut slash = SlashCommands::global();
        slash.register(
            "ping",
            "確認",
            vec![SlashCommands::string_option("key", "対象", true)],
            async |_: &mut SlashContext<'_>| -> Result<()> { panic!("denied command ran") },
        )?;
        slash.autocomplete(
            &["ping"],
            "key",
            async |_: &mut AutocompleteContext<'_>| -> Result<()> {
                panic!("denied autocomplete ran");
            },
        )?;
        slash.add_check(CommandCheck::new(async |ctx: CommandCheckContext<'_>| {
            // Authorization uses the registered root even if the payload name differs.
            if ctx.command_path().eq(["ping"]) {
                Err(CheckFailure::Rejected.into())
            } else {
                Ok(())
            }
        }));
        let mut registered = slash.replacement_commands()?;
        registered[0].id = Some(Id::new(5));
        assert_eq!(registered[0].default_member_permissions, None);
        let active = slash.bind(registered)?;
        let mut command = command_interaction(None, None, 5);
        let Some(InteractionData::ApplicationCommand(data)) = &mut command.data else {
            return Err(Error::msg(
                "試験用の Interaction にコマンドデータがありません",
            ));
        };
        data.name = "different".into();
        let mut autocomplete = command.clone();
        let Some(InteractionData::ApplicationCommand(data)) = &mut autocomplete.data else {
            return Err(Error::msg(
                "試験用の Interaction にコマンドデータがありません",
            ));
        };
        data.options = vec![option(
            "key",
            CommandOptionValue::Focused("k".into(), CommandOptionType::String),
        )];
        autocomplete.kind = InteractionType::ApplicationCommandAutocomplete;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(Error::handler)?;
        let http = Client::builder()
            .proxy(
                listener.local_addr().map_err(Error::handler)?.to_string(),
                true,
            )
            .build();
        let errors = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = std::sync::Arc::clone(&errors);
        let hook = crate::hooks::Reporter::new(move |report| {
            observed
                .lock()
                .unwrap()
                .push(matches!(report.error, Error::Check(CheckFailure::Rejected)));
        });
        let mut hooks = Hooks {
            reporter: hook,
            ..Hooks::default()
        };
        hooks.messages.check_failed = Some("private custom denial".into());
        let (_, timing) = crate::timing::Pending::new((), None).begin();
        for (input, expected) in [
            (&command, "private custom denial"),
            (&autocomplete, "\"choices\":[]"),
        ] {
            let (_, response) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                tokio::join!(
                    active.dispatch(&http, input, timing, &hooks),
                    capture_response(&listener, "POST")
                )
            })
            .await
            .map_err(Error::handler)?;
            let response = response.map_err(Error::handler)?;
            assert!(response.contains(expected), "{response}");
            if input.kind == InteractionType::ApplicationCommand {
                assert!(response.contains("\"flags\":64"), "{response}");
            }
        }
        assert_eq!(*errors.lock().unwrap(), [true, true]);
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

    #[tokio::test]
    async fn initial_reply_checks_message_length_before_sending() -> Result<()> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(Error::handler)?;
        let http = Client::builder()
            .proxy(
                listener.local_addr().map_err(Error::handler)?.to_string(),
                true,
            )
            .build();
        let interaction = command_interaction(None, None, 5);
        let hooks = Hooks::default();
        let (_, timing) = crate::timing::Pending::new((), None).begin();
        let initial = crate::slash::InitialResponder {
            http: &http,
            interaction: &interaction,
            timing,
            info: &hooks.info,
            permissions: &hooks.permissions,
        };
        let invalid = "あ".repeat(2001);
        let rejected = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            initial.message(&invalid, true),
        )
        .await
        .map_err(Error::handler)?;
        assert!(matches!(rejected, Err(Error::MessageValidation(_))));

        let valid = "あ".repeat(2000);
        let (result, payload) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(
                initial.message(&valid, true),
                capture_response(&listener, "POST")
            )
        })
        .await
        .map_err(Error::handler)?;
        result?;
        let payload: serde_json::Value =
            serde_json::from_str(&payload.map_err(Error::handler)?).map_err(Error::handler)?;
        assert_eq!(payload["data"]["content"].as_str(), Some(valid.as_str()));

        Ok(())
    }

    #[tokio::test]
    async fn busy_autocomplete_sends_empty_choices_instead_of_a_message() -> Result<()> {
        use twilight_model::application::interaction::InteractionType;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(Error::handler)?;
        let http = Client::builder()
            .proxy(
                listener.local_addr().map_err(Error::handler)?.to_string(),
                true,
            )
            .build();
        let mut interaction = command_interaction(None, None, 5);
        interaction.kind = InteractionType::ApplicationCommandAutocomplete;
        let mut hooks = Hooks::default();
        for message in [Some("busy".into()), None] {
            hooks.messages.interaction_busy = message;
            let pending = crate::timing::Pending::new(
                crate::InteractionEvent {
                    interaction: interaction.clone(),
                    radio: Vec::new(),
                },
                None,
            );
            let (_, response) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                tokio::join!(
                    crate::respond_interaction_busy(pending, &http, &hooks),
                    capture_response(&listener, "POST")
                )
            })
            .await
            .map_err(Error::handler)?;
            let response: serde_json::Value =
                serde_json::from_str(&response.map_err(Error::handler)?).map_err(Error::handler)?;
            assert_eq!(
                response,
                serde_json::json!({ "type": 8, "data": { "choices": [] } })
            );
        }
        Ok(())
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
        assert_eq!(
            routing_path(&data.options, &mut [""; 2]),
            Some(["advanced", "set"].as_slice())
        );
        let leaf = leaf_options(&data.options);
        assert_eq!(leaf.len(), 1);
        assert_eq!(leaf[0].name, "value");

        let plain = CommandData {
            options: vec![option("value", CommandOptionValue::Integer(3))],
            ..data
        };
        assert_eq!(command_path(&plain), ["config"]);
        assert_eq!(
            routing_path(&plain.options, &mut [""; 2]),
            Some([].as_slice())
        );
        assert_eq!(leaf_options(&plain.options).len(), 1);

        let malformed = [option(
            "one",
            CommandOptionValue::SubCommandGroup(vec![option(
                "two",
                CommandOptionValue::SubCommandGroup(vec![option(
                    "three",
                    CommandOptionValue::SubCommand(vec![]),
                )]),
            )]),
        )];
        assert!(routing_path(&malformed, &mut [""; 2]).is_none());
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
            .map(|(name, definition)| {
                let route = definition.into_route(name.clone());
                (name, route)
            })
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
        let route = slash
            .entries
            .remove("config")
            .unwrap()
            .into_route("config".into());
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
                permissions: &crate::permissions::PermissionResolver::default(),
            },
            data,
            response_finished: false,
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
        ctx.response_finished = true;
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

    pub(super) fn test_command_message(content: &str) -> Result<twilight_model::channel::Message> {
        let mut message: twilight_model::channel::Message = serde_json::from_str(
            r#"{
            "id": "1", "channel_id": "2", "type": 0,
            "author": {"id": "3", "username": "member", "discriminator": "0"},
            "content": "", "timestamp": "2026-01-01T00:00:00.000000+00:00",
            "attachments": [], "embeds": [], "mention_everyone": false, "mention_roles": [],
            "mentions": [], "pinned": false, "tts": false
        }"#,
        )
        .map_err(Error::handler)?;
        message.content = content.into();
        Ok(message)
    }

    #[tokio::test]
    async fn registered_typed_hybrid_dispatches_both_sources_and_rejects_invalid_input()
    -> Result<()> {
        use crate::{
            CommandCheck, CommandCheckContext, CommandCheckSource, CommandSource, Commands,
            Context, HybridArgumentError, HybridArguments, HybridContext, hooks::Reporter,
            permissions::PermissionResolver, register_typed_hybrid, timing::Pending,
        };
        use std::sync::{Arc, Mutex};

        let mut text = Commands::new("!")?;
        let mut slash = SlashCommands::global();
        let checked = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&checked);
        let check = CommandCheck::new(async move |ctx: CommandCheckContext<'_>| {
            assert!(ctx.command_path().eq(["inspect"]));
            observed.lock().unwrap().push(ctx.source());
            Ok(())
        });
        text.add_check(check.clone());
        slash.add_check(check);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&calls);
        register_typed_hybrid(
            &mut text,
            Some(&mut slash),
            "inspect",
            "対象を確認",
            vec![
                SlashCommands::user_option("user", "利用者", true),
                SlashCommands::integer_option("count", "個数", false),
            ],
            async move |ctx: &mut HybridContext<'_>, args: HybridArguments<'_>| -> Result<()> {
                recorded.lock().unwrap().push((
                    ctx.source(),
                    args.user_id("user"),
                    args.integer("count"),
                ));
                Ok(())
            },
        )?;

        let http = Client::new("test".into());
        let permissions = PermissionResolver::new(None);
        let message = test_command_message("!inspect <@55> 2")?;
        let (_, timing) = Pending::new((), None).begin();
        let make_context = |message| Context {
            http: &http,
            message,
            timing,
            permissions: &permissions,
        };
        text.dispatch(make_context(&message)).await?;
        let mut bad_text = message.clone();
        bad_text.content = "!inspect <#55> 2".into();
        assert!(matches!(
            text.dispatch(make_context(&bad_text)).await,
            Err(Error::HybridArgument(
                HybridArgumentError::InvalidValue { .. }
            ))
        ));

        let mut registered = slash.replacement_commands()?;
        registered.first_mut().unwrap().id = Some(Id::new(5));
        let active = slash.bind(registered)?;
        let mut interaction = command_interaction(None, None, 5);
        let Some(InteractionData::ApplicationCommand(data)) = &mut interaction.data else {
            panic!("test interaction contains command data");
        };
        data.name = "inspect".into();
        data.options = vec![
            option("count", CommandOptionValue::Integer(2)),
            option("user", CommandOptionValue::User(Id::new(55))),
        ];
        let mut bad_slash = interaction.clone();
        let Some(InteractionData::ApplicationCommand(data)) = &mut bad_slash.data else {
            panic!("test interaction contains command data");
        };
        data.options.pop();
        let invalid = Arc::new(Mutex::new(0));
        let notices = Arc::clone(&invalid);
        let mut hooks = Hooks {
            reporter: Reporter::new(move |report| {
                assert!(matches!(
                    report.error,
                    Error::HybridArgument(HybridArgumentError::Missing { .. })
                ));
                *notices.lock().unwrap() += 1;
            }),
            ..Hooks::default()
        };
        hooks.messages.slash_command_failed = None;
        active.dispatch(&http, &interaction, timing, &hooks).await;
        active.dispatch(&http, &bad_slash, timing, &hooks).await;
        assert_eq!(*invalid.lock().unwrap(), 1);
        assert_eq!(
            *checked.lock().unwrap(),
            [
                CommandCheckSource::Text,
                CommandCheckSource::Text,
                CommandCheckSource::Application(CommandType::ChatInput),
                CommandCheckSource::Application(CommandType::ChatInput),
            ]
        );
        assert_eq!(
            *calls.lock().unwrap(),
            [
                (CommandSource::Text, Some(Id::new(55)), Some(2)),
                (CommandSource::Slash, Some(Id::new(55)), Some(2)),
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn registered_hybrid_group_dispatches_aliased_nested_and_fallback_routes() -> Result<()> {
        use crate::{
            CommandSource, Commands, Context, HybridArguments, HybridContext, Mentionable,
            permissions::PermissionResolver, register_hybrid_group_with_aliases, timing::Pending,
        };
        use std::sync::{Arc, Mutex};
        use twilight_model::application::interaction::InteractionDataResolved;

        let mut text = Commands::new("!")?;
        let mut slash = SlashCommands::global();
        let calls = Arc::new(Mutex::new(Vec::new()));
        register_hybrid_group_with_aliases(
            &mut text,
            Some(&mut slash),
            "config",
            "設定",
            &["c"],
            |group| {
                let seen = Arc::clone(&calls);
                group.fallback(
                    "show",
                    "確認",
                    async move |ctx: &mut HybridContext<'_>,
                                _: HybridArguments<'_>|
                                -> Result<()> {
                        seen.lock().unwrap().push((ctx.source(), None, None, None));
                        Ok(())
                    },
                )?;
                let mut advanced = group.group_with_aliases("advanced", "詳細", &["a"])?;
                let seen = Arc::clone(&calls);
                advanced.subcommand_with_aliases(
                    "set",
                    "変更",
                    &["s"],
                    vec![
                        SlashCommands::attachment_option("file", "添付", true),
                        SlashCommands::integer_option("count", "個数", true),
                        SlashCommands::mentionable_option("actor", "対象", true),
                    ],
                    async move |ctx: &mut HybridContext<'_>,
                                args: HybridArguments<'_>|
                                -> Result<()> {
                        seen.lock().unwrap().push((
                            ctx.source(),
                            args.integer("count"),
                            args.attachment("file").map(|attachment| attachment.id),
                            args.mentionable("actor"),
                        ));
                        Ok(())
                    },
                )
            },
        )?;

        let http = Client::new("test".into());
        let permissions = PermissionResolver::new(None);
        let (_, timing) = Pending::new((), None).begin();
        let resolved: InteractionDataResolved = serde_json::from_value(serde_json::json!({
            "attachments": {"17": {"id":"17", "filename":"test.txt", "size":2,
                "url":"https://example.test/file", "proxy_url":"https://example.test/file"}},
            "users": {"19": {"id":"19", "username":"member", "discriminator":"0"}}
        }))
        .map_err(Error::handler)?;
        for content in [
            "!c a s 2 <@19>",
            "!c unknown",
            "!c a missing 2",
            "!c show",
            "!c",
        ] {
            let mut message = test_command_message(content)?;
            message
                .attachments
                .extend(resolved.attachments.values().cloned());
            text.dispatch(Context {
                http: &http,
                message: &message,
                timing,
                permissions: &permissions,
            })
            .await?;
        }

        let mut registered = slash.replacement_commands()?;
        let command = registered.first_mut().unwrap();
        assert_eq!(command.name, "config");
        assert_eq!(
            command
                .options
                .iter()
                .map(|option| option.name.as_str())
                .collect::<Vec<_>>(),
            ["advanced", "show"]
        );
        command.id = Some(Id::new(5));
        let active = slash.bind(registered)?;
        let mut hooks = Hooks::default();
        hooks.messages.slash_command_failed = None;
        hooks.messages.slash_command_unavailable = None;
        for options in [
            vec![option(
                "advanced",
                CommandOptionValue::SubCommandGroup(vec![option(
                    "set",
                    CommandOptionValue::SubCommand(vec![
                        option("count", CommandOptionValue::Integer(2)),
                        option("actor", CommandOptionValue::Mentionable(Id::new(19))),
                        option("file", CommandOptionValue::Attachment(Id::new(17))),
                    ]),
                )]),
            )],
            vec![option("show", CommandOptionValue::SubCommand(Vec::new()))],
            Vec::new(),
        ] {
            let mut interaction = command_interaction(None, None, 5);
            let Some(InteractionData::ApplicationCommand(data)) = &mut interaction.data else {
                panic!("test interaction contains command data");
            };
            data.name = "config".into();
            data.options = options;
            data.resolved = Some(resolved.clone());
            active.dispatch(&http, &interaction, timing, &hooks).await;
        }
        assert_eq!(
            *calls.lock().unwrap(),
            [
                (
                    CommandSource::Text,
                    Some(2),
                    Some(Id::new(17)),
                    Some(Mentionable::User(Id::new(19)))
                ),
                (CommandSource::Text, None, None, None),
                (
                    CommandSource::Slash,
                    Some(2),
                    Some(Id::new(17)),
                    Some(Mentionable::User(Id::new(19)))
                ),
                (CommandSource::Slash, None, None, None),
            ]
        );
        Ok(())
    }

    #[test]
    fn context_menus_sync_by_kind_and_name() -> Result<()> {
        use super::UserMenuTarget;
        use crate::HandlerKind;
        use twilight_model::channel::Message;

        let registry = || -> Result<SlashCommands> {
            let mut commands = SlashCommands::global().replace_commands();
            commands.register("inspect", "対象を確認", vec![], handler)?;
            let inspect_user =
                async |_: &mut SlashContext<'_>, _: UserMenuTarget<'_>| -> Result<()> { Ok(()) };
            let inspect_message =
                async |_: &mut SlashContext<'_>, _: &Message| -> Result<()> { Ok(()) };
            commands.register_user_menu("inspect", inspect_user)?;
            commands.register_message_menu("inspect", inspect_message)?;
            commands.register_user_menu("Show User Info", inspect_user)?;
            assert!(matches!(
                commands.register_user_menu("inspect", inspect_user),
                Err(Error::Registration(RegistrationError::Duplicate {
                    kind: HandlerKind::UserCommand,
                    ..
                }))
            ));
            assert!(commands.register_message_menu("", inspect_message).is_err());
            assert!(
                commands
                    .register_message_menu(&"あ".repeat(33), inspect_message)
                    .is_err()
            );
            Ok(commands)
        };
        let commands = registry()?;
        let mut returned = commands.replacement_commands()?;
        assert_eq!(returned.len(), 4);
        let ids = [11, 12, 13, 14];
        let expected: Vec<_> = returned
            .iter_mut()
            .zip(ids)
            .map(|(command, id)| {
                assert_eq!(
                    command.description.is_empty(),
                    command.kind != CommandType::ChatInput
                );
                assert!(command.options.is_empty());
                command.id = Some(Id::new(id));
                (Id::new(id), command.kind)
            })
            .collect();
        let mut malformed = returned.clone();
        malformed
            .iter_mut()
            .find(|command| command.kind == CommandType::Message)
            .unwrap()
            .kind = CommandType::User;
        assert!(matches!(
            registry()?.bind(malformed),
            Err(Error::CommandSync(
                crate::CommandSyncError::UnexpectedName { .. }
            ))
        ));
        returned.reverse();
        let active = commands.bind(returned)?;
        for (id, kind) in expected {
            assert_eq!(active.handlers.get(&id).map(|route| route.kind), Some(kind));
        }
        Ok(())
    }

    fn menu_interaction(
        id: u64,
        kind: CommandType,
        target_id: u64,
        resolved: serde_json::Value,
    ) -> Interaction {
        let mut interaction = command_interaction(None, None, id);
        let Some(InteractionData::ApplicationCommand(data)) = &mut interaction.data else {
            panic!("test interaction contains command data");
        };
        data.name = "Inspect".into();
        data.kind = kind;
        data.target_id = Some(Id::new(target_id));
        data.resolved = serde_json::from_value(resolved).unwrap();
        interaction
    }

    #[tokio::test]
    async fn context_menus_deliver_typed_targets_and_use_standard_failure_hooks() -> Result<()> {
        use std::sync::{Arc, Mutex};

        use super::UserMenuTarget;
        use crate::{CheckFailure, ErrorSource, hooks::Reporter, timing::Pending};
        use serde_json::json;
        use twilight_model::channel::Message;

        let mut commands = SlashCommands::global();
        let held = Arc::new(Mutex::new(Vec::new()));
        let seen_users = Arc::clone(&held);
        commands.register_user_menu(
            "Inspect",
            async move |_: &mut SlashContext<'_>, target: UserMenuTarget<'_>| -> Result<()> {
                assert!(target.member.is_none());
                seen_users.lock().unwrap().push(target.user.id.get());
                Ok(())
            },
        )?;
        let seen_messages = Arc::clone(&held);
        commands.register_message_menu(
            "Inspect",
            async move |ctx: &mut SlashContext<'_>, target: &Message| -> Result<()> {
                assert_eq!(target.content, "probe");
                seen_messages.lock().unwrap().push(target.id.get());
                ctx.require_guild()?;
                Ok(())
            },
        )?;
        let mut registered = commands.replacement_commands()?;
        for (command, id) in registered.iter_mut().zip([11, 12]) {
            command.id = Some(Id::new(id));
        }
        let active = commands.bind(registered)?;

        let errors = Arc::new(Mutex::new(Vec::new()));
        let collected_errors = Arc::clone(&errors);
        let mut hooks = Hooks {
            reporter: Reporter::new(move |report| {
                assert_eq!(report.source, ErrorSource::SlashCommand);
                assert_eq!(report.name, Some("Inspect"));
                let label = match report.error {
                    Error::Check(CheckFailure::GuildOnly) => "check",
                    Error::CommandTargetUnavailable => "target",
                    Error::CommandUnavailable => "route",
                    _ => panic!("unexpected command error"),
                };
                collected_errors.lock().unwrap().push(label);
            }),
            ..Hooks::default()
        };
        hooks.messages.slash_command_failed = None;
        hooks.messages.slash_command_unavailable = None;
        hooks.messages.check_failed = None;
        let (_, timing) = Pending::new((), None).begin();
        let http = Client::new("test".into());

        let user =
            json!({"users": {"5": {"id": "5", "username": "selected", "discriminator": "0"}}});
        let message = json!({"messages": {"7": {
            "id": "7", "channel_id": "2", "type": 0,
            "author": {"id": "4", "username": "member", "discriminator": "0"},
            "content": "probe", "timestamp": "2026-01-01T00:00:00.000000+00:00",
            "attachments": [], "embeds": [], "mention_everyone": false, "mention_roles": [],
            "mentions": [], "pinned": false, "tts": false
        }}});
        for interaction in [
            menu_interaction(11, CommandType::User, 5, user.clone()),
            menu_interaction(12, CommandType::Message, 7, message),
            menu_interaction(11, CommandType::User, 9, user.clone()),
            menu_interaction(12, CommandType::User, 5, user.clone()),
        ] {
            assert!(active.accepts(&interaction));
            active.dispatch(&http, &interaction, timing, &hooks).await;
        }
        // Autocomplete は、ユーザー・メッセージコマンドに対して処理しません。
        let mut autocomplete = menu_interaction(11, CommandType::User, 5, user);
        autocomplete.kind = twilight_model::application::interaction::InteractionType::ApplicationCommandAutocomplete;
        assert!(!active.accepts(&autocomplete));
        assert_eq!(*held.lock().unwrap(), [5, 7]);
        assert_eq!(*errors.lock().unwrap(), ["check", "target", "route"]);
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

    #[test]
    fn require_validates_the_path_and_keeps_an_explicit_visibility() -> Result<()> {
        use crate::Needs;

        let mut slash = SlashCommands::for_guild(Id::new(1));
        slash.register(
            "purge",
            "削除",
            vec![],
            async |_: &mut SlashContext<'_>| -> Result<()> { Ok(()) },
        )?;
        slash.register_with_permissions(
            "ban",
            "BAN",
            vec![],
            Permissions::BAN_MEMBERS,
            async |_: &mut SlashContext<'_>| -> Result<()> { Ok(()) },
        )?;
        slash.group("config", "設定")?.subcommand(
            "set",
            "値",
            vec![],
            async |_: &mut SlashContext<'_>| -> Result<()> { Ok(()) },
        )?;
        let needs = Needs::new().author(Permissions::MANAGE_MESSAGES);

        // 登録されていないコマンドやサブコマンドは、確認が効かないまま見逃さず、失敗する。
        for unknown in ["", "nope", "purge extra", "config nope", "config set extra"] {
            assert!(slash.require(unknown, needs).is_err(), "{unknown:?}");
        }
        assert!(slash.checks.is_empty());

        slash.require("purge", needs)?;
        assert!(matches!(
            slash.entries["purge"].permissions,
            PermissionPolicy::Visibility(visible) if visible == Permissions::MANAGE_MESSAGES
        ));
        // 明示した設定は、変えない。
        slash.require("ban", needs)?;
        assert!(matches!(
            slash.entries["ban"].permissions,
            PermissionPolicy::Required(required) if required == Permissions::BAN_MEMBERS
        ));
        // サブコマンドへの宣言は、最上位の表示条件を変えない。
        slash.require("config set", needs)?;
        assert!(matches!(
            slash.entries["config"].permissions,
            PermissionPolicy::Unrestricted
        ));
        assert_eq!(slash.checks.len(), 3);
        Ok(())
    }
}
