//! スラッシュコマンドの引数に、入力中の候補を返す（オートコンプリート）。

use std::{future::Future, pin::Pin};

use twilight_model::{
    application::{
        command::{CommandOptionChoice, CommandOptionChoiceValue},
        interaction::application_command::{CommandData, CommandDataOption, CommandOptionValue},
    },
    http::interaction::{InteractionResponse, InteractionResponseData, InteractionResponseType},
    id::{
        Id,
        marker::{GuildMarker, UserMarker},
    },
};
use twilight_validate::command::{self, CHOICES_LIMIT};

use crate::{
    Error, Result, Timing,
    error::ResponseStateError,
    slash::{InitialResponder, command_path, leaf_options},
};

/// 引数の入力中に、Discord が候補を求めてきたときの呼び出しです。
///
/// [`SlashCommands::autocomplete`](crate::SlashCommands::autocomplete) に登録した関数に渡されます。
/// 3 秒以内に [`respond`](Self::respond) で候補を返してください。
pub struct AutocompleteContext<'a> {
    pub(crate) initial: InitialResponder<'a>,
    pub(crate) data: &'a CommandData,
    pub(crate) responded: bool,
}

impl std::fmt::Debug for AutocompleteContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutocompleteContext")
            .field("interaction_id", &self.initial.interaction.id)
            .field("command", &self.data.name)
            .field("responded", &self.responded)
            .finish_non_exhaustive()
    }
}

impl<'a> AutocompleteContext<'a> {
    /// Akari が使用している Twilight の REST クライアントを借ります。
    pub fn http(&self) -> &'a crate::http::Client {
        self.initial.http
    }

    /// Gateway の往復時間と、このイベントの処理時間を確認します。
    pub fn timing(&self) -> Timing {
        self.initial.timing
    }

    /// 入力している人の ID を返します。
    pub fn author_id(&self) -> Option<Id<UserMarker>> {
        self.initial.interaction.author_id()
    }

    /// 入力しているサーバーの ID を返します。DM では `None` です。
    pub fn guild_id(&self) -> Option<Id<GuildMarker>> {
        self.initial.interaction.guild_id
    }

    /// コマンド名とサブコマンド名の経路を返します。
    pub fn command_path(&self) -> Vec<&'a str> {
        command_path(self.data)
    }

    /// いま入力している引数の名前を返します。
    pub fn focused_name(&self) -> &'a str {
        self.focused_option()
            .map_or("", |option| option.name.as_str())
    }

    /// いま入力している途中の文字列を返します（何も入力していなければ空です）。
    pub fn focused(&self) -> &'a str {
        match self.focused_option().map(|option| &option.value) {
            Some(CommandOptionValue::Focused(value, _)) => value,
            _ => "",
        }
    }

    /// 同じコマンドで、すでに入力済みの文字列の引数を借ります。
    pub fn string(&self, name: &str) -> Option<&'a str> {
        let option = leaf_options(&self.data.options)
            .iter()
            .find(|option| option.name == name)?;
        match &option.value {
            CommandOptionValue::String(value) => Some(value),
            _ => None,
        }
    }

    fn focused_option(&self) -> Option<&'a CommandDataOption> {
        leaf_options(&self.data.options)
            .iter()
            .find(|option| matches!(option.value, CommandOptionValue::Focused(_, _)))
    }

    /// 候補を返します。最大 25 件で、一度だけ返せます。
    ///
    /// 候補は [`string_choice`](crate::SlashCommands::string_choice)、
    /// [`integer_choice`](crate::SlashCommands::integer_choice)、
    /// [`number_choice`](crate::SlashCommands::number_choice) で作れます。
    ///
    /// # エラー
    ///
    /// 26 件以上の場合は [`Error::TooManyChoices`]、候補の名前や値が Discord の制約に合わない場合は
    /// 検証のエラー、すでに返した場合は [`ResponseStateError::AlreadyResponded`] を返します。
    pub async fn respond(&mut self, choices: Vec<CommandOptionChoice>) -> Result<()> {
        if self.responded {
            return Err(ResponseStateError::AlreadyResponded.into());
        }
        if choices.len() > CHOICES_LIMIT {
            return Err(Error::TooManyChoices {
                count: choices.len(),
                max: CHOICES_LIMIT,
            });
        }
        for choice in &choices {
            command::choice(choice)?;
        }
        self.send(choices).await
    }

    async fn send(&mut self, choices: Vec<CommandOptionChoice>) -> Result<()> {
        let response = InteractionResponse {
            kind: InteractionResponseType::ApplicationCommandAutocompleteResult,
            data: Some(InteractionResponseData {
                choices: Some(choices),
                ..Default::default()
            }),
        };
        self.initial.send(&response).await?;
        self.responded = true;
        Ok(())
    }

    /// 関数が返さずに終わったときに、入力欄が読み込み中のままにならないよう、空の候補を返す。
    pub(crate) async fn respond_empty_if_needed(&mut self) -> Result<()> {
        if self.responded {
            return Ok(());
        }
        self.send(Vec::new()).await
    }
}

/// 候補の値を作る補助。
pub(crate) fn choice(name: &str, value: CommandOptionChoiceValue) -> CommandOptionChoice {
    CommandOptionChoice {
        name: name.into(),
        name_localizations: None,
        value,
    }
}

pub(crate) trait AutocompleteHandler: Send + Sync {
    fn call<'a, 'ctx>(
        &'a self,
        ctx: &'a mut AutocompleteContext<'ctx>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>>;
}

impl<F> AutocompleteHandler for F
where
    F: AsyncFn(&mut AutocompleteContext<'_>) -> Result<()> + Send + Sync,
{
    fn call<'a, 'ctx>(
        &'a self,
        ctx: &'a mut AutocompleteContext<'ctx>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        Box::pin(self(ctx))
    }
}
