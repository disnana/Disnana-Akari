use std::collections::{HashMap, HashSet};

use serde::Serialize;
use twilight_http::{Client, request::Request, routing::Route};
use twilight_model::{
    application::{
        command::{Command, CommandOption, CommandType},
        interaction::InteractionContextType,
    },
    guild::Permissions,
    id::{Id, marker::ApplicationMarker},
    oauth::ApplicationIntegrationType,
};
use twilight_validate::command;

use super::{Scope, SlashCommands, context_menu::MenuKind, validation::validate_command_size};
use crate::{Error, RegistrationError, Result};

/// Discord が表示するアプリコマンドの利用場所、インストール先、翻訳を設定します。
///
/// 指定しない項目には Discord の既定値が使われます。
#[derive(Clone, Debug, Default)]
pub struct CommandSettings {
    contexts: Option<Vec<InteractionContextType>>,
    installs: Option<Vec<ApplicationIntegrationType>>,
    names: HashMap<String, String>,
    descriptions: HashMap<String, String>,
    nsfw: Option<bool>,
}

impl CommandSettings {
    /// Discord の既定値を使う設定を作ります。
    pub fn new() -> Self {
        Self::default()
    }

    /// グローバルコマンドを使える場所を設定します。
    pub fn contexts(mut self, values: impl IntoIterator<Item = InteractionContextType>) -> Self {
        self.contexts = Some(values.into_iter().collect());
        self
    }

    /// グローバルコマンドのインストール先を設定します。
    pub fn install_types(
        mut self,
        values: impl IntoIterator<Item = ApplicationIntegrationType>,
    ) -> Self {
        self.installs = Some(values.into_iter().collect());
        self
    }

    /// 指定した Discord ロケールで表示する名前を設定します。
    pub fn name_localization(mut self, locale: impl Into<String>, name: impl Into<String>) -> Self {
        self.names.insert(locale.into(), name.into());
        self
    }

    /// スラッシュコマンドの説明を、指定した Discord ロケール向けに設定します。
    pub fn description_localization(
        mut self,
        locale: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        self.descriptions.insert(locale.into(), description.into());
        self
    }

    /// Discord 側で年齢制限のあるコマンドとして表示するかを指定します。
    pub fn nsfw(mut self, value: bool) -> Self {
        self.nsfw = Some(value);
        self
    }

    fn validate(&self, scope: Scope, kind: CommandType) -> Result<()> {
        if scope != Scope::Global && (self.contexts.is_some() || self.installs.is_some()) {
            return Err(invalid_setting("contexts / integration_types"));
        }
        if let Some(contexts) = &self.contexts
            && (!unique_nonempty(contexts)
                || contexts.iter().any(|kind| {
                    !matches!(
                        kind,
                        InteractionContextType::Guild
                            | InteractionContextType::BotDm
                            | InteractionContextType::PrivateChannel
                    )
                }))
        {
            return Err(invalid_setting("contexts"));
        }
        if let Some(installs) = &self.installs {
            if !unique_nonempty(installs)
                || installs.iter().any(|kind| {
                    !matches!(
                        kind,
                        ApplicationIntegrationType::GuildInstall
                            | ApplicationIntegrationType::UserInstall
                    )
                })
            {
                return Err(invalid_setting("integration_types"));
            }
            if !installs.contains(&ApplicationIntegrationType::UserInstall)
                && self.contexts.as_ref().is_some_and(|contexts| {
                    contexts.contains(&InteractionContextType::PrivateChannel)
                })
            {
                return Err(invalid_setting("contexts / integration_types"));
            }
        }
        if kind != CommandType::ChatInput && !self.descriptions.is_empty() {
            return Err(invalid_setting("description_localizations"));
        }
        for (locale, name) in &self.names {
            validate_locale(locale)?;
            if kind == CommandType::ChatInput {
                command::chat_input_name(name)?;
            } else {
                command::name(name)?;
            }
        }
        for (locale, description) in &self.descriptions {
            validate_locale(locale)?;
            command::description(description)?;
        }
        Ok(())
    }

    pub(super) fn apply(&self, command: &mut Command) {
        command.contexts.clone_from(&self.contexts);
        command.integration_types.clone_from(&self.installs);
        command.name_localizations = (!self.names.is_empty()).then(|| self.names.clone());
        command.description_localizations =
            (!self.descriptions.is_empty()).then(|| self.descriptions.clone());
        command.nsfw = self.nsfw;
    }
}

fn unique_nonempty<T: Eq + std::hash::Hash>(values: &[T]) -> bool {
    !values.is_empty() && values.iter().collect::<HashSet<_>>().len() == values.len()
}

pub(super) fn validate_locale(locale: &str) -> Result<()> {
    // https://docs.discord.com/developers/reference#locales
    if !matches!(
        locale,
        "id" | "da"
            | "de"
            | "en-GB"
            | "en-US"
            | "es-ES"
            | "es-419"
            | "fr"
            | "hr"
            | "it"
            | "lt"
            | "hu"
            | "nl"
            | "no"
            | "pl"
            | "pt-BR"
            | "ro"
            | "fi"
            | "sv-SE"
            | "vi"
            | "tr"
            | "cs"
            | "el"
            | "bg"
            | "ru"
            | "uk"
            | "hi"
            | "th"
            | "zh-CN"
            | "ja"
            | "zh-TW"
            | "ko"
    ) {
        return Err(invalid_setting("locale"));
    }
    Ok(())
}

pub(super) fn invalid_setting(field: &'static str) -> Error {
    RegistrationError::InvalidApplicationCommandSetting { field }.into()
}

pub(super) type SettingsByKind = HashMap<CommandType, HashMap<String, CommandSettings>>;

impl SlashCommands {
    /// 登録済みのコマンドに利用場所、インストール先、翻訳などの設定を付けます。
    ///
    /// `kind` と `name` は登録種別と翻訳前の名前です。何度か設定する場合は最後の値を使います。
    ///
    /// # エラー
    /// 指定したコマンドがない、または種別や同期先と設定の組み合わせが無効な場合に失敗します。
    /// 失敗した場合、以前の設定は維持されます。
    pub fn set_command_settings(
        &mut self,
        kind: CommandType,
        name: &str,
        settings: CommandSettings,
    ) -> Result<()> {
        let exists = if kind == CommandType::ChatInput {
            self.entries.contains_key(name)
        } else {
            MenuKind::from_command_type(kind).is_some_and(|kind| self.menus.contains(kind, name))
        };
        if !exists {
            return Err(RegistrationError::UnknownCommand {
                name: name.to_owned(),
            }
            .into());
        }
        settings.validate(self.scope, kind)?;
        if kind == CommandType::ChatInput
            && let Some(definition) = self.entries.get(name)
        {
            validate_command_size(
                name,
                Some(&settings.names),
                &definition.description,
                Some(&settings.descriptions),
                &definition.option_tree(),
            )?;
        }
        self.settings
            .entry(kind)
            .or_default()
            .insert(name.to_owned(), settings);
        Ok(())
    }

    pub(super) fn configure_command(&self, command: &mut Command) {
        if let Some(settings) = self
            .settings
            .get(&command.kind)
            .and_then(|settings| settings.get(&command.name))
        {
            settings.apply(command);
        }
    }
}

#[derive(Serialize)]
pub(super) struct CommandPayload<'a> {
    #[serde(rename = "type")]
    kind: CommandType,
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<&'a [CommandOption]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default_member_permissions: Option<Permissions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    contexts: Option<&'a [InteractionContextType]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    integration_types: Option<&'a [ApplicationIntegrationType]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name_localizations: Option<&'a HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description_localizations: Option<&'a HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nsfw: Option<bool>,
}

impl<'a> From<&'a Command> for CommandPayload<'a> {
    fn from(command: &'a Command) -> Self {
        let chat = command.kind == CommandType::ChatInput;
        Self {
            kind: command.kind,
            name: &command.name,
            description: chat.then_some(&command.description),
            options: chat.then_some(&command.options),
            default_member_permissions: command.default_member_permissions,
            contexts: command.contexts.as_deref(),
            integration_types: command.integration_types.as_deref(),
            name_localizations: command.name_localizations.as_ref(),
            description_localizations: command.description_localizations.as_ref(),
            nsfw: command.nsfw,
        }
    }
}

pub(super) fn settings_request(
    application_id: Id<ApplicationMarker>,
    scope: Scope,
    command: &Command,
) -> std::result::Result<Request, twilight_http::Error> {
    let application_id = application_id.get();
    let route = match scope {
        Scope::Global => Route::CreateGlobalCommand { application_id },
        Scope::Guild(guild) => Route::CreateGuildCommand {
            application_id,
            guild_id: guild.get(),
        },
    };
    Request::builder(&route)
        .json(&CommandPayload::from(command))
        .build()
}

pub(super) async fn create_configured_command(
    http: &Client,
    application_id: Id<ApplicationMarker>,
    scope: Scope,
    command: &Command,
) -> Result<Command> {
    let registration = |source| Error::CommandRegistration {
        name: command.name.clone(),
        source,
    };
    let request = settings_request(application_id, scope, command).map_err(registration)?;
    Ok(http
        .request::<Command>(request)
        .await
        .map_err(registration)?
        .model()
        .await?)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::{SlashContext, UserMenuTarget};

    async fn ping(_: &mut SlashContext<'_>) -> Result<()> {
        Ok(())
    }

    async fn user(_: &mut SlashContext<'_>, _: UserMenuTarget<'_>) -> Result<()> {
        Ok(())
    }

    #[test]
    fn sync_payloads_preserve_kind_and_configured_metadata() -> Result<()> {
        let mut slash = SlashCommands::global().replace_commands();
        slash.register("ping", "latency", vec![], ping)?;
        slash.register_user_menu("ping", user)?;
        slash.set_command_settings(
            CommandType::ChatInput,
            "ping",
            CommandSettings::new()
                .install_types([ApplicationIntegrationType::UserInstall])
                .contexts([
                    InteractionContextType::Guild,
                    InteractionContextType::PrivateChannel,
                ])
                .name_localization("ja", "ピング")
                .description_localization("ja", "応答を確認する")
                .nsfw(false),
        )?;
        slash.set_command_settings(
            CommandType::User,
            "ping",
            CommandSettings::new().name_localization("ja", "ユーザー確認"),
        )?;
        let commands = slash.replacement_commands()?;
        let wire: Vec<Value> = commands
            .iter()
            .map(|command| {
                let request = settings_request(Id::new(1), slash.scope, command).unwrap();
                assert_eq!(request.path(), "applications/1/commands");
                serde_json::from_slice(request.body().unwrap()).unwrap()
            })
            .collect();
        assert_eq!(
            wire,
            [
                json!({"type":1,"name":"ping","description":"latency","options":[],
                   "contexts":[0,2],"integration_types":[1],"name_localizations":{"ja":"ピング"},
                   "description_localizations":{"ja":"応答を確認する"},"nsfw":false}),
                json!({"type":2,"name":"ping","name_localizations":{"ja":"ユーザー確認"}}),
            ]
        );
        assert_eq!(commands[0].contexts.as_ref().unwrap().len(), 2);
        assert!(commands[1].contexts.is_none());
        Ok(())
    }

    #[test]
    fn invalid_overrides_keep_the_previous_settings() -> Result<()> {
        let mut guild = SlashCommands::for_guild(Id::new(2));
        guild.register("ping", "latency", vec![], ping)?;
        guild.set_command_settings(
            CommandType::ChatInput,
            "ping",
            CommandSettings::new().name_localization("ja", "ピング"),
        )?;
        assert!(
            guild
                .set_command_settings(
                    CommandType::ChatInput,
                    "ping",
                    CommandSettings::new().contexts([InteractionContextType::Guild]),
                )
                .is_err()
        );
        assert_eq!(
            guild.replacement_commands()?[0]
                .name_localizations
                .as_ref()
                .unwrap()["ja"],
            "ピング"
        );
        let mut global = SlashCommands::global();
        global.register_user_menu("ping", user)?;
        for settings in [
            CommandSettings::new()
                .contexts([InteractionContextType::Guild, InteractionContextType::Guild]),
            CommandSettings::new().install_types([]),
            CommandSettings::new()
                .install_types([ApplicationIntegrationType::GuildInstall])
                .contexts([InteractionContextType::PrivateChannel]),
            CommandSettings::new().description_localization("ja", "使えない項目"),
        ] {
            assert!(
                global
                    .set_command_settings(CommandType::User, "ping", settings)
                    .is_err()
            );
        }
        assert!(
            global
                .set_command_settings(CommandType::ChatInput, "ping", CommandSettings::new())
                .is_err()
        );
        assert!(
            global.replacement_commands()?[0]
                .name_localizations
                .is_none()
        );
        Ok(())
    }
}
