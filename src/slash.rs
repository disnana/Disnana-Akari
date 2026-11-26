use std::{
    collections::{BTreeMap, HashMap, btree_map::Entry},
    future::Future,
    pin::Pin,
};

use twilight_http::Client;
use twilight_model::{
    application::{
        command::{Command, CommandOption, CommandOptionType, CommandType},
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

use crate::{Result, Timing, ui::UiMessage};

const COMMAND_FAILED: &str = "コマンドを処理できませんでした。";

/// 一つのサーバーに登録するスラッシュコマンド一覧です。
pub struct SlashCommands {
    guild_id: Id<GuildMarker>,
    entries: BTreeMap<String, Definition>,
    replace: bool,
}

impl SlashCommands {
    /// 指定したサーバー向けに空の一覧を作ります。
    pub fn for_guild(guild_id: Id<GuildMarker>) -> Self {
        Self {
            guild_id,
            entries: BTreeMap::new(),
            replace: false,
        }
    }

    /// 起動時に、このアプリのサーバーコマンド一覧を登録済みの定義で置き換えます。
    ///
    /// 一覧に含まれないスラッシュコマンドや、ユーザー・メッセージのコマンドも削除されます。
    /// 空の一覧を指定した場合は、このアプリの対象サーバーのコマンドがすべて削除されます。
    pub fn replace_guild_commands(mut self) -> Self {
        self.replace = true;
        self
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
        if self.entries.is_empty() && !self.replace {
            return self.bind(Vec::new());
        }

        let application = http.current_user_application().await?.model().await?;
        let client = http.interaction(application.id);
        if self.replace {
            let commands = self.replacement_commands()?;
            let registered = client
                .set_guild_commands(self.guild_id, &commands)
                .await?
                .models()
                .await?;
            return self.bind(registered);
        }

        let mut registered = Vec::with_capacity(self.entries.len());
        for (name, definition) in &self.entries {
            let command = client
                .create_guild_command(self.guild_id)
                .chat_input(name, &definition.description)
                .command_options(&definition.options)
                .await
                .map_err(|error| {
                    format!("スラッシュコマンド `/{name}` を登録できませんでした ({error})")
                })?
                .model()
                .await?;
            registered.push(command);
        }

        self.bind(registered)
    }

    #[allow(deprecated)] // Twilight の Command モデルに含まれる dm_permission は送信しない。
    fn replacement_commands(&self) -> Result<Vec<Command>> {
        self.entries
            .iter()
            .map(|(name, definition)| {
                let command = Command {
                    application_id: None,
                    contexts: None,
                    default_member_permissions: None,
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
                    options: definition.options.clone(),
                    version: Id::new(1),
                };
                command::command(&command)?;
                Ok(command)
            })
            .collect()
    }

    fn bind(mut self, registered: Vec<Command>) -> Result<ActiveSlashCommands> {
        if registered.len() != self.entries.len() {
            return Err("Discord が返したコマンド件数が登録件数と一致しません".into());
        }

        let mut handlers = HashMap::with_capacity(registered.len());
        for command in registered {
            let name = command.name;
            if command.kind != CommandType::ChatInput {
                return Err(
                    format!("`/{name}` の登録結果がスラッシュコマンドではありません").into(),
                );
            }
            let id = command
                .id
                .ok_or_else(|| format!("`/{name}` の登録結果に ID がありません"))?;
            let definition = self.entries.remove(&name).ok_or_else(|| {
                format!("`/{name}` の登録結果が重複しているか、登録要求に含まれていません")
            })?;
            if let std::collections::hash_map::Entry::Vacant(entry) = handlers.entry(id) {
                entry.insert(definition.handler);
            } else {
                return Err(format!("`/{name}` の登録結果の ID が重複しています").into());
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
    pub(crate) fn accepts(&self, interaction: &Interaction) -> bool {
        self.command_data(interaction).is_some()
    }

    fn command_data<'a>(&self, interaction: &'a Interaction) -> Option<&'a CommandData> {
        if interaction.guild_id != Some(self.guild_id)
            || interaction.kind != InteractionType::ApplicationCommand
        {
            return None;
        }
        let Some(InteractionData::ApplicationCommand(data)) = &interaction.data else {
            return None;
        };
        let in_scope = match data.guild_id {
            Some(guild_id) => guild_id == self.guild_id,
            None => self.handlers.contains_key(&data.id),
        };
        (data.kind == CommandType::ChatInput && in_scope).then_some(data)
    }

    pub(crate) async fn dispatch(&self, http: &Client, interaction: &Interaction, timing: Timing) {
        let Some(data) = self.command_data(interaction) else {
            return;
        };
        let Some(handler) = self.handlers.get(&data.id) else {
            if send_initial_message(
                http,
                interaction,
                "このコマンドは現在利用できません。",
                true,
            )
            .await
            .is_err()
            {
                eprintln!("未登録スラッシュコマンドへの応答に失敗しました");
            }
            return;
        };

        let mut response_state = ResponseState::Pending;
        let mut ctx = SlashContext {
            http,
            interaction,
            data,
            response_state: &mut response_state,
            timing,
        };
        if let Err(error) = handler.call(&mut ctx).await {
            eprintln!(
                "スラッシュコマンド `/{}` の処理に失敗しました ({error})",
                data.name
            );
        }
        let fallback = match *ctx.response_state {
            ResponseState::Pending => ctx.reply_ephemeral(COMMAND_FAILED).await,
            ResponseState::Deferred { .. } => ctx.reply(COMMAND_FAILED).await,
            ResponseState::Replied => Ok(()),
        };
        if fallback.is_err() {
            eprintln!("スラッシュコマンドのエラー応答に失敗しました");
        }
    }
}

/// 一回のスラッシュコマンド呼び出しを扱います。
pub struct SlashContext<'a> {
    http: &'a Client,
    interaction: &'a Interaction,
    data: &'a CommandData,
    response_state: &'a mut ResponseState,
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
            response_state: self.response_state,
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
        message.validate()?;
        let client = self.http.interaction(self.interaction.application_id);

        match self.response_state.reply_action(false)? {
            ReplyAction::Initial => {
                let response = initial_response(InitialResponse::Ui(message), false);
                client
                    .create_response(self.interaction.id, &self.interaction.token, &response)
                    .await?;
            }
            ReplyAction::EditDeferred => {
                let mentions = AllowedMentions::default();
                let mut update = client
                    .update_response(&self.interaction.token)
                    .allowed_mentions(Some(&mentions));
                if let Some(content) = &message.content {
                    update = update.content(Some(content));
                }
                if !message.embeds.is_empty() {
                    update = update.embeds(Some(&message.embeds));
                }
                if !message.components.is_empty() {
                    update = update.components(Some(&message.components));
                }
                update.await?;
            }
        }

        *self.response_state = ResponseState::Replied;
        Ok(())
    }

    /// 呼び出した本人にだけ見える初回応答を送るか、非公開の遅延応答を完成させます。
    ///
    /// # エラー
    ///
    /// 応答済み、公開の遅延応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn reply_ephemeral(&mut self, content: &str) -> Result<()> {
        self.respond(content, true).await
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
        if *self.response_state != ResponseState::Pending {
            return Err("このスラッシュコマンドにはすでに初回応答を送りました".into());
        }
        let response = initial_response(InitialResponse::Deferred, ephemeral);
        self.http
            .interaction(self.interaction.application_id)
            .create_response(self.interaction.id, &self.interaction.token, &response)
            .await?;
        *self.response_state = ResponseState::Deferred { ephemeral };
        Ok(())
    }

    async fn respond(&mut self, content: &str, ephemeral: bool) -> Result<()> {
        match self.response_state.reply_action(ephemeral)? {
            ReplyAction::Initial => {
                send_initial_message(self.http, self.interaction, content, ephemeral).await?;
            }
            ReplyAction::EditDeferred => {
                self.http
                    .interaction(self.interaction.application_id)
                    .update_response(&self.interaction.token)
                    .content(Some(content))
                    .allowed_mentions(Some(&AllowedMentions::default()))
                    .await?;
            }
        }
        *self.response_state = ResponseState::Replied;
        Ok(())
    }

    async fn send_followup(&self, content: &str, ephemeral: bool) -> Result<()> {
        if *self.response_state != ResponseState::Replied {
            return Err("追加メッセージを送る前に reply で初回メッセージを送ってください".into());
        }
        let mentions = AllowedMentions::default();
        let client = self.http.interaction(self.interaction.application_id);
        let mut followup = client
            .create_followup(&self.interaction.token)
            .content(content)
            .allowed_mentions(Some(&mentions));
        if ephemeral {
            followup = followup.flags(MessageFlags::EPHEMERAL);
        }
        followup.await?;
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResponseState {
    Pending,
    Deferred { ephemeral: bool },
    Replied,
}

impl ResponseState {
    fn reply_action(self, ephemeral: bool) -> Result<ReplyAction> {
        match self {
            Self::Pending => Ok(ReplyAction::Initial),
            Self::Deferred { ephemeral: false } if ephemeral => {
                Err("遅延応答の公開範囲は変更できません".into())
            }
            Self::Deferred { .. } => Ok(ReplyAction::EditDeferred),
            Self::Replied => Err("このスラッシュコマンドにはすでに応答しました".into()),
        }
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
        InitialResponse::Deferred => InteractionResponse {
            kind: InteractionResponseType::DeferredChannelMessageWithSource,
            data: flags.map(|flags| InteractionResponseData {
                flags: Some(flags),
                ..Default::default()
            }),
        },
    }
}

pub(crate) async fn send_initial_message(
    http: &Client,
    interaction: &Interaction,
    content: &str,
    ephemeral: bool,
) -> Result<()> {
    let response = initial_response(InitialResponse::Message(content), ephemeral);
    http.interaction(interaction.application_id)
        .create_response(interaction.id, &interaction.token, &response)
        .await?;
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
    use super::{
        AllowedMentions, CommandOptionType, CommandType, Id, InitialResponse,
        InteractionResponseType, MessageFlags, ReplyAction, ResponseState, Result, SlashCommands,
        SlashContext, initial_response,
    };
    use crate::ui::{Button, Embed, UiMessage};

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

    fn two_commands() -> Result<SlashCommands> {
        let mut slash = SlashCommands::for_guild(Id::new(1)).replace_guild_commands();
        slash.register("echo", "echo text", vec![], handler)?;
        slash.register("ping", "return pong", vec![], handler)?;
        Ok(slash)
    }

    #[test]
    fn replacement_binds_ids_by_name_and_accepts_empty_registry() -> Result<()> {
        let empty = SlashCommands::for_guild(Id::new(1)).replace_guild_commands();
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
