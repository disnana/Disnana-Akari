use std::collections::HashMap;

use twilight_model::application::command::{
    CommandOption, CommandOptionChoiceValue, CommandOptionType,
};
use twilight_validate::command::CHOICES_LIMIT;

use crate::{RegistrationError, Result};

use super::settings::invalid_setting;

const COMMAND_TOTAL_LENGTH: usize = 8000;

pub(super) fn validate_command_size(
    name: &str,
    name_localizations: Option<&HashMap<String, String>>,
    description: &str,
    description_localizations: Option<&HashMap<String, String>>,
    options: &[CommandOption],
) -> Result<()> {
    // https://docs.discord.com/developers/interactions/application-commands#slash-commands
    let characters = longest(name, name_localizations)
        + longest(description, description_localizations)
        + options.iter().map(option_characters).sum::<usize>();
    if characters > COMMAND_TOTAL_LENGTH {
        return Err(RegistrationError::SlashCommandTooLarge { characters }.into());
    }
    Ok(())
}

fn longest(default: &str, localizations: Option<&HashMap<String, String>>) -> usize {
    localizations
        .into_iter()
        .flat_map(HashMap::values)
        .map(|value| value.chars().count())
        .fold(default.chars().count(), usize::max)
}

fn option_characters(option: &CommandOption) -> usize {
    let choices = option.choices.iter().flatten().map(|choice| {
        let value = match &choice.value {
            CommandOptionChoiceValue::String(value) => value.chars().count(),
            _ => 0,
        };
        longest(&choice.name, choice.name_localizations.as_ref()) + value
    });
    let children = option.options.iter().flatten().map(option_characters);
    longest(&option.name, option.name_localizations.as_ref())
        + longest(
            &option.description,
            option.description_localizations.as_ref(),
        )
        + choices.chain(children).sum::<usize>()
}

pub(super) fn validate_choice_settings(option: &CommandOption) -> Result<()> {
    let supports_choices = matches!(
        option.kind,
        CommandOptionType::String | CommandOptionType::Integer | CommandOptionType::Number
    );
    if option
        .choices
        .as_ref()
        .is_some_and(|choices| choices.len() > CHOICES_LIMIT || !supports_choices)
    {
        return Err(invalid_setting("choices"));
    }
    if option.autocomplete == Some(true) && (!supports_choices || option.choices.is_some()) {
        return Err(invalid_setting("autocomplete"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use twilight_http::{Client, request::TryIntoRequest};
    use twilight_model::{application::command::CommandType, id::Id};

    use super::*;
    use crate::{CommandSettings, Error, SlashCommands, SlashContext, hooks::InfoReporter};

    async fn command(_: &mut SlashContext<'_>) -> Result<()> {
        Ok(())
    }

    // 79 labels x 100 characters, plus four 1-character option names and descriptions = 7908.
    fn numeric_options() -> Result<Vec<CommandOption>> {
        let label = |index| format!("{}{:03}", "界".repeat(97), index);
        let mut options = Vec::new();
        for name in ["a", "b", "c"] {
            let mut option = SlashCommands::integer_option(name, "d", false);
            option.choices = Some(
                (0..25)
                    .map(|index| {
                        SlashCommands::localize_choice(
                            SlashCommands::integer_choice("word", index),
                            "ja",
                            label(index),
                        )
                    })
                    .collect::<Result<Vec<_>>>()?,
            );
            options.push(option);
        }
        let mut option = SlashCommands::number_option("d", "d", false);
        option.choices = Some(
            (0..4)
                .map(|index| {
                    let localized = SlashCommands::localize_choice(
                        SlashCommands::number_choice("word", index as f64 + 0.5),
                        "ja",
                        label(index),
                    )?;
                    SlashCommands::localize_choice(localized, "de", "z".repeat(99))
                })
                .collect::<Result<Vec<_>>>()?,
        );
        options.push(option);
        Ok(options)
    }

    #[test]
    fn counts_unicode_string_values_and_longest_translation_per_field() -> Result<()> {
        let mut option = SlashCommands::string_option("abc", "d", false);
        option.name_localizations = Some(HashMap::from([
            ("ja".into(), "長い名前".into()),
            ("de".into(), "a".into()),
        ]));
        option.description_localizations = Some(HashMap::from([
            ("ja".into(), "短".into()),
            ("de".into(), "longer".into()),
        ]));
        let choice = SlashCommands::localize_choice(
            SlashCommands::string_choice("abc", "音楽🎵"),
            "ja",
            "選択肢です",
        )?;
        option.choices = Some(vec![choice]);
        assert_eq!(option_characters(&option), 4 + 6 + 5 + 3);
        Ok(())
    }

    #[tokio::test]
    async fn accepts_exactly_8000_rejects_the_next_character_and_preserves_settings() -> Result<()>
    {
        use crate::slash::settings::settings_request;

        let http = Client::new("test".into());
        let application_id = Id::new(1);
        let client = http.interaction(application_id);
        let guild_id = Id::new(2);
        for mut slash in [SlashCommands::global(), SlashCommands::for_guild(guild_id)] {
            let boundary = "文".repeat(91);
            let too_long = "文".repeat(92);
            assert!(matches!(
                slash.register("x", &too_long, numeric_options()?, command),
                Err(Error::Registration(
                    RegistrationError::SlashCommandTooLarge { characters: 8001 }
                ))
            ));
            slash.register("x", &"文".repeat(90), numeric_options()?, command)?;
            slash.set_command_settings(
                CommandType::ChatInput,
                "x",
                CommandSettings::new().description_localization("ja", &boundary),
            )?;
            assert!(matches!(
                slash.set_command_settings(
                    CommandType::ChatInput,
                    "x",
                    CommandSettings::new().description_localization("ja", too_long)
                ),
                Err(Error::Registration(
                    RegistrationError::SlashCommandTooLarge { characters: 8001 }
                ))
            ));
            let models = slash.replacement_commands()?;
            let model = &models[0];
            assert_eq!(
                model.description_localizations.as_ref().unwrap()["ja"],
                boundary
            );
            assert!(settings_request(application_id, slash.scope, model).is_ok());
            let request = if slash.scope.guild_id().is_some() {
                client
                    .set_guild_commands(guild_id, &models)
                    .try_into_request()?
            } else {
                client.set_global_commands(&models).try_into_request()?
            };
            assert!(request.body().is_some());
        }
        Ok(())
    }

    #[tokio::test]
    async fn preflight_rejects_an_overfull_group_before_any_rest_in_every_scope_and_mode()
    -> Result<()> {
        let http = Client::builder().proxy("127.0.0.1:1".into(), true).build();
        for guild in [None, Some(Id::new(2))] {
            for replace in [false, true] {
                let slash = guild.map_or_else(SlashCommands::global, SlashCommands::for_guild);
                let mut slash = if replace {
                    slash.replace_commands()
                } else {
                    slash
                };
                slash.register("first", "valid earlier command", vec![], command)?;
                let mut group = slash.group("x", &"d".repeat(91))?;
                group.subcommand("s", "d", numeric_options()?, command)?;
                assert!(matches!(
                    slash.sync(&http, &InfoReporter::default()).await,
                    Err(Error::Registration(
                        RegistrationError::SlashCommandTooLarge { characters: 8002 }
                    ))
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn rejects_raw_choice_overflow_and_autocomplete_conflicts_before_registering() -> Result<()> {
        let mut slash = SlashCommands::global();
        let mut option = SlashCommands::integer_option("amount", "件数", true);
        option.choices = Some(
            (0..26)
                .map(|value| SlashCommands::integer_choice("label", value))
                .collect(),
        );
        assert!(matches!(
            slash.register("x", "d", vec![option.clone()], command),
            Err(Error::Registration(
                RegistrationError::InvalidApplicationCommandSetting { field: "choices" }
            ))
        ));
        option.choices.as_mut().unwrap().pop();
        option.autocomplete = Some(true);
        assert!(matches!(
            slash.register("x", "d", vec![option.clone()], command),
            Err(Error::Registration(
                RegistrationError::InvalidApplicationCommandSetting {
                    field: "autocomplete"
                }
            ))
        ));
        option.autocomplete = Some(false);
        slash.register("x", "d", vec![option], command)?;

        let mut flag = SlashCommands::boolean_option("flag", "フラグ", true);
        flag.autocomplete = Some(true);
        assert!(
            slash
                .group("group", "d")?
                .subcommand("leaf", "d", vec![flag], command)
                .is_err()
        );
        Ok(())
    }
}
