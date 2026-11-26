use std::{
    collections::{BTreeMap, HashMap, btree_map::Entry},
    future::Future,
    pin::Pin,
};

use twilight_http::Client;
use twilight_model::{
    application::{
        command::{CommandOption, CommandOptionType, CommandType},
        interaction::{
            Interaction, InteractionData, InteractionType,
            application_command::{CommandData, CommandOptionValue},
        },
    },
    channel::message::{AllowedMentions, MessageFlags},
    http::interaction::{InteractionResponse, InteractionResponseData, InteractionResponseType},
    id::{
        Id,
        marker::{CommandMarker, GuildMarker, UserMarker},
    },
};
use twilight_validate::command;

use crate::{Result, Timing};

/// 一つのサーバーに登録するスラッシュコマンド一覧です。
pub struct SlashCommands {
    guild_id: Id<GuildMarker>,
    entries: BTreeMap<String, Definition>,
}

impl SlashCommands {
    /// 指定したサーバー向けに空の一覧を作ります。
    pub fn for_guild(guild_id: Id<GuildMarker>) -> Self {
        Self {
            guild_id,
            entries: BTreeMap::new(),
        }
    }

    /// Discord に表示するコマンド名、説明、引数と、呼び出す関数を設定します。
    ///
    /// # エラー
    ///
    /// Discord の制約に合わない定義や、重複した名前を拒否します。
    pub fn register(
        &mut self,
        name: &str,
        description: &str,
        options: Vec<CommandOption>,
        handler: impl AsyncFn(&mut SlashContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        command::chat_input_name(name)?;
        command::description(description)?;
        command::options(&options)?;
        match self.entries.entry(name.into()) {
            Entry::Vacant(entry) => {
                entry.insert(Definition {
                    description: description.into(),
                    options,
                    handler: Box::new(handler),
                });
                Ok(())
            }
            Entry::Occupied(_) => {
                Err(format!("スラッシュコマンド `/{name}` は登録済みです").into())
            }
        }
    }

    /// 文字列の引数を作ります。必要なら返された Twilight の定義で長さなどを設定できます。
    pub fn string_option(name: &str, description: &str, required: bool) -> CommandOption {
        CommandOption {
            autocomplete: None,
            channel_types: None,
            choices: None,
            description: description.into(),
            description_localizations: None,
            kind: CommandOptionType::String,
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

    pub(crate) async fn sync(self, http: &Client) -> Result<ActiveSlashCommands> {
        let mut handlers = HashMap::with_capacity(self.entries.len());
        if !self.entries.is_empty() {
            let application = http.current_user_application().await?.model().await?;
            let client = http.interaction(application.id);
            for (name, definition) in self.entries {
                let registered = client
                    .create_guild_command(self.guild_id)
                    .chat_input(&name, &definition.description)
                    .command_options(&definition.options)
                    .await
                    .map_err(|error| {
                        format!("スラッシュコマンド `/{name}` を登録できませんでした ({error})")
                    })?
                    .model()
                    .await?;
                if let Some(id) = registered.id {
                    handlers.insert(id, definition.handler);
                } else {
                    return Err(format!(
                        "スラッシュコマンド `/{name}` の登録結果に ID がありません"
                    )
                    .into());
                }
            }
        }
        Ok(ActiveSlashCommands {
            guild_id: self.guild_id,
            handlers,
        })
    }
}

struct Definition {
    description: String,
    options: Vec<CommandOption>,
    handler: Box<dyn SlashHandler>,
}

pub(crate) struct ActiveSlashCommands {
    guild_id: Id<GuildMarker>,
    handlers: HashMap<Id<CommandMarker>, Box<dyn SlashHandler>>,
}

impl ActiveSlashCommands {
    pub(crate) async fn dispatch(&self, http: &Client, interaction: &Interaction, timing: Timing) {
        if interaction.guild_id != Some(self.guild_id)
            || interaction.kind != InteractionType::ApplicationCommand
        {
            return;
        }
        let Some(InteractionData::ApplicationCommand(data)) = &interaction.data else {
            return;
        };
        if data.kind != CommandType::ChatInput {
            return;
        }
        let Some(handler) = self.handlers.get(&data.id) else {
            return;
        };

        let mut responded = false;
        let mut ctx = SlashContext {
            http,
            interaction,
            data,
            responded: &mut responded,
            timing,
        };
        if let Err(error) = handler.call(&mut ctx).await {
            eprintln!(
                "スラッシュコマンド `/{}` の処理に失敗しました ({error})",
                data.name
            );
        }
        if !*ctx.responded
            && let Err(error) = ctx
                .reply_ephemeral("コマンドを処理できませんでした。")
                .await
        {
            eprintln!("スラッシュコマンドのエラー応答に失敗しました ({error})");
        }
    }
}

/// 一回のスラッシュコマンド呼び出しを扱います。
pub struct SlashContext<'a> {
    http: &'a Client,
    interaction: &'a Interaction,
    data: &'a CommandData,
    responded: &'a mut bool,
    timing: Timing,
}

impl<'a> SlashContext<'a> {
    /// コマンドを呼び出したユーザーの ID を返します。
    pub fn author_id(&self) -> Option<Id<UserMarker>> {
        self.interaction.author_id()
    }

    /// Gateway の往復時間と、このコマンドの処理時間を確認します。
    pub fn timing(&self) -> Timing {
        self.timing
    }

    pub(crate) fn reborrow(&mut self) -> SlashContext<'_> {
        SlashContext {
            http: self.http,
            interaction: self.interaction,
            data: self.data,
            responded: self.responded,
            timing: self.timing,
        }
    }

    /// 指定した名前の引数を Twilight の値として返します。
    pub fn option(&self, name: &str) -> Option<&'a CommandOptionValue> {
        self.data
            .options
            .iter()
            .find(|option| option.name == name)
            .map(|option| &option.value)
    }

    /// 文字列の引数を借ります。未入力または別の型なら `None` を返します。
    pub fn string(&self, name: &str) -> Option<&'a str> {
        match self.option(name)? {
            CommandOptionValue::String(value) => Some(value),
            _ => None,
        }
    }

    /// チャンネルに表示する初回応答を送ります。
    ///
    /// # エラー
    ///
    /// 二回目の応答や、Discord が受け付けなかった場合に失敗します。
    pub async fn reply(&mut self, content: &str) -> Result<()> {
        self.respond(content, None).await
    }

    /// 呼び出した本人にだけ見える初回応答を送ります。
    ///
    /// # エラー
    ///
    /// 二回目の応答や、Discord が受け付けなかった場合に失敗します。
    pub async fn reply_ephemeral(&mut self, content: &str) -> Result<()> {
        self.respond(content, Some(MessageFlags::EPHEMERAL)).await
    }

    async fn respond(&mut self, content: &str, flags: Option<MessageFlags>) -> Result<()> {
        if *self.responded {
            return Err("このスラッシュコマンドにはすでに応答しました".into());
        }
        let response = InteractionResponse {
            kind: InteractionResponseType::ChannelMessageWithSource,
            data: Some(InteractionResponseData {
                content: Some(content.into()),
                allowed_mentions: Some(AllowedMentions::default()),
                flags,
                ..Default::default()
            }),
        };
        self.http
            .interaction(self.interaction.application_id)
            .create_response(self.interaction.id, &self.interaction.token, &response)
            .await?;
        *self.responded = true;
        Ok(())
    }
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
    use super::{CommandOptionType, Id, Result, SlashCommands, SlashContext};

    async fn handler(_: &mut SlashContext<'_>) -> Result<()> {
        Ok(())
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
}
