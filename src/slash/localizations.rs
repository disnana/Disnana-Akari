use std::collections::HashMap;

use twilight_model::application::command::{CommandOption, CommandOptionChoice};
use twilight_validate::command;

use crate::Result;

use super::settings::{invalid_setting, validate_locale};

type Names = HashMap<String, String>;

/// サブコマンド、中間グループ、引数の表示名と説明を翻訳します。
#[derive(Clone, Debug, Default)]
pub struct CommandOptionLocalizations {
    names: Names,
    descriptions: Names,
}

impl CommandOptionLocalizations {
    /// 翻訳を含まない設定を作ります。
    pub fn new() -> Self {
        Self::default()
    }

    /// 指定した Discord ロケール向けの表示名を設定します。
    pub fn name_localization(mut self, locale: impl Into<String>, name: impl Into<String>) -> Self {
        self.names.insert(locale.into(), name.into());
        self
    }

    /// 指定した Discord ロケール向けの説明を設定します。
    pub fn description_localization(
        mut self,
        locale: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        self.descriptions.insert(locale.into(), description.into());
        self
    }

    /// 引数の翻訳を置き換えます。正式名と値は変わりません。
    ///
    /// # エラー
    /// ロケール、翻訳名、翻訳された説明が Discord の制約に合わない場合に失敗します。
    pub fn apply(self, mut option: CommandOption) -> Result<CommandOption> {
        self.validate()?;
        option.name_localizations = Some(self.names).filter(|names| !names.is_empty());
        option.description_localizations =
            Some(self.descriptions).filter(|descriptions| !descriptions.is_empty());
        Ok(option)
    }

    pub(super) fn apply_borrowed(&self, option: &mut CommandOption) {
        option.name_localizations = (!self.names.is_empty()).then(|| self.names.clone());
        option.description_localizations =
            (!self.descriptions.is_empty()).then(|| self.descriptions.clone());
    }

    pub(super) fn validate(&self) -> Result<()> {
        validate_translations(self.names.iter(), self.descriptions.iter())
    }

    pub(super) fn names(&self) -> Option<&Names> {
        (!self.names.is_empty()).then_some(&self.names)
    }
}

impl super::SlashCommands {
    /// 引数の候補にロケール別の表示名を付けます。候補の値は変わりません。
    ///
    /// # エラー
    /// ロケールまたは翻訳名が Discord の制約に合わない場合に失敗します。
    pub fn localize_choice(
        mut choice: CommandOptionChoice,
        locale: impl Into<String>,
        name: impl Into<String>,
    ) -> Result<CommandOptionChoice> {
        let locale = locale.into();
        let name = name.into();
        validate_locale(&locale)?;
        command::choice_name(&name)?;
        choice
            .name_localizations
            .get_or_insert_with(Names::new)
            .insert(locale, name);
        Ok(choice)
    }
}

fn validate_translations<'a>(
    names: impl IntoIterator<Item = (&'a String, &'a String)>,
    descriptions: impl IntoIterator<Item = (&'a String, &'a String)>,
) -> Result<()> {
    for (locale, name) in names {
        validate_locale(locale)?;
        command::option_name(name)?;
    }
    for (locale, description) in descriptions {
        validate_locale(locale)?;
        command::description(description)?;
    }
    Ok(())
}

// Discord checks localized option names against each other and against siblings' default names.
pub(super) fn validate_distinct_names<'a>(
    name: &str,
    names: Option<&Names>,
    siblings: impl IntoIterator<Item = (&'a str, Option<&'a Names>)>,
) -> Result<()> {
    for (sibling, translations) in siblings {
        if sibling == name {
            continue;
        }
        let name_conflicts = names.into_iter().flatten().any(|(locale, translated)| {
            translated == sibling
                || translations.and_then(|other| other.get(locale)) == Some(translated)
        });
        let sibling_conflicts = translations
            .into_iter()
            .flatten()
            .any(|(_, translated)| translated == name);
        if name_conflicts || sibling_conflicts {
            return Err(invalid_setting("name_localizations"));
        }
    }
    Ok(())
}

pub(crate) fn validate_options(options: &[CommandOption]) -> Result<()> {
    command::options(options)?;
    validate_option_localizations(options)
}

fn validate_option_localizations(options: &[CommandOption]) -> Result<()> {
    for (index, option) in options.iter().enumerate() {
        validate_translations(
            option.name_localizations.iter().flatten(),
            option.description_localizations.iter().flatten(),
        )?;
        validate_distinct_names(
            &option.name,
            option.name_localizations.as_ref(),
            options
                .iter()
                .skip(index + 1)
                .map(|other| (other.name.as_str(), other.name_localizations.as_ref())),
        )?;
        for choice in option.choices.iter().flatten() {
            for (locale, name) in choice.name_localizations.iter().flatten() {
                validate_locale(locale)?;
                command::choice_name(name)?;
            }
        }
        if let Some(children) = &option.options {
            validate_option_localizations(children)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use twilight_http::{Client, request::TryIntoRequest};
    use twilight_model::id::Id;

    use super::*;
    use crate::{
        Commands, Error, HybridArguments, HybridContext, RegistrationError, SlashCommands,
        SlashContext, register_hybrid_group, register_typed_hybrid,
    };

    async fn handler(_: &mut SlashContext<'_>) -> Result<()> {
        Ok(())
    }

    async fn hybrid(_: &mut HybridContext<'_>, _: HybridArguments<'_>) -> Result<()> {
        Ok(())
    }

    #[tokio::test]
    async fn localized_groups_options_and_choices_are_serialized_on_both_sync_paths() -> Result<()>
    {
        let choice = SlashCommands::localize_choice(
            SlashCommands::string_choice("dark", "dark"),
            "ja",
            "暗い",
        )?;
        let mut mode = CommandOptionLocalizations::new()
            .name_localization("ja", "表示")
            .description_localization("ja", "モードを選択")
            .apply(SlashCommands::string_option("mode", "view mode", true))?;
        mode.choices = Some(vec![choice]);

        let mut slash = SlashCommands::global();
        let mut root = slash.group("config", "settings")?;
        let mut nested = root.group("advanced", "advanced settings")?;
        nested.subcommand("set", "update", vec![mode], handler)?;
        nested.set_child_localizations(
            "set",
            CommandOptionLocalizations::new().name_localization("ja", "設定"),
        )?;
        root.set_child_localizations(
            "advanced",
            CommandOptionLocalizations::new()
                .name_localization("ja", "詳細")
                .description_localization("ja", "詳しい設定"),
        )?;

        let commands = slash.replacement_commands()?;
        let command = &commands[0];
        let http = Client::new("test".into());
        let client = http.interaction(Id::new(1));
        let post = client
            .create_global_command()
            .chat_input(&command.name, &command.description)
            .command_options(&command.options)
            .try_into_request()?;
        let post: Value = serde_json::from_slice(post.body().unwrap()).map_err(Error::handler)?;
        let put = client.set_global_commands(&commands).try_into_request()?;
        let put: Value = serde_json::from_slice(put.body().unwrap()).map_err(Error::handler)?;
        assert_eq!(post["options"], put[0]["options"]);
        let group = &post["options"][0];
        assert_eq!(group["name"], "advanced");
        assert_eq!(group["name_localizations"], json!({"ja":"詳細"}));
        assert_eq!(
            group["description_localizations"],
            json!({"ja":"詳しい設定"})
        );
        let sub = &group["options"][0];
        assert_eq!(sub["name"], "set");
        assert_eq!(sub["name_localizations"], json!({"ja":"設定"}));
        let option = &sub["options"][0];
        assert_eq!(option["name"], "mode");
        assert_eq!(option["name_localizations"], json!({"ja":"表示"}));
        assert_eq!(
            option["description_localizations"],
            json!({"ja":"モードを選択"})
        );
        assert_eq!(
            option["choices"][0],
            json!({"name":"dark","name_localizations":{"ja":"暗い"},"value":"dark"})
        );
        Ok(())
    }

    #[test]
    fn collisions_and_invalid_locales_keep_the_previous_registration() -> Result<()> {
        let translations =
            |locale, name| CommandOptionLocalizations::new().name_localization(locale, name);
        let mut slash = SlashCommands::global();
        let mut root = slash.group("config", "settings")?;
        root.subcommand("set", "update", vec![], handler)?;
        root.subcommand("get", "read", vec![], handler)?;
        root.set_child_localizations("set", translations("ja", "設定"))?;
        assert!(
            root.set_child_localizations("get", translations("ja", "設定"))
                .is_err()
        );
        assert!(
            root.set_child_localizations("set", translations("ja", "get"))
                .is_err()
        );
        assert!(
            root.set_child_localizations("set", translations("xx", "更新"))
                .is_err()
        );
        assert!(
            root.subcommand("設定", "conflict", vec![], handler)
                .is_err()
        );
        root.set_child_localizations("get", translations("en-US", "設定"))?;

        let commands = slash.replacement_commands()?;
        let set = commands[0]
            .options
            .iter()
            .find(|option| option.name == "set")
            .unwrap();
        assert_eq!(set.name_localizations.as_ref().unwrap()["ja"], "設定");
        assert_eq!(commands[0].options.len(), 2);

        let raw_conflict =
            translations("ja", "get").apply(SlashCommands::string_option("set", "write", true))?;
        assert!(matches!(
            slash.register(
                "bad",
                "invalid",
                vec![
                    raw_conflict,
                    SlashCommands::string_option("get", "read", true)
                ],
                handler
            ),
            Err(Error::Registration(
                RegistrationError::InvalidApplicationCommandSetting {
                    field: "name_localizations"
                }
            ))
        ));
        slash.register("bad", "valid now", vec![], handler)?;
        Ok(())
    }

    #[test]
    fn hybrid_localization_preserves_option_names_and_staged_rollback() -> Result<()> {
        let mut text = Commands::new("!")?;
        let mut slash = SlashCommands::global();
        let mut invalid = SlashCommands::string_option("mode", "view mode", true);
        invalid.description_localizations = Some(HashMap::from([("ja".into(), String::new())]));
        assert!(
            register_typed_hybrid(&mut text, None, "inspect", "info", vec![invalid], hybrid)
                .is_err()
        );
        assert!(
            register_hybrid_group(&mut text, Some(&mut slash), "inspect", "info", |group| {
                group.subcommand("set", "update", vec![], hybrid)?;
                group.set_child_localizations(
                    "set",
                    CommandOptionLocalizations::new().name_localization("ja", "INVALID"),
                )
            })
            .is_err()
        );

        let option = CommandOptionLocalizations::new()
            .name_localization("ja", "表示")
            .apply(SlashCommands::string_option("mode", "view mode", true))?;
        register_hybrid_group(&mut text, Some(&mut slash), "inspect", "info", |group| {
            group.subcommand("set", "update", vec![option], hybrid)?;
            group.set_child_localizations(
                "set",
                CommandOptionLocalizations::new().name_localization("ja", "設定"),
            )
        })?;
        let commands = slash.replacement_commands()?;
        let sub = &commands[0].options[0];
        assert_eq!(sub.name, "set");
        assert_eq!(sub.name_localizations.as_ref().unwrap()["ja"], "設定");
        let option = &sub.options.as_ref().unwrap()[0];
        assert_eq!(option.name, "mode");
        assert_eq!(option.name_localizations.as_ref().unwrap()["ja"], "表示");
        assert!(text.matches_message("!inspect set dark"));
        assert!(!text.matches_message("!inspect 設定 dark"));
        Ok(())
    }
}
