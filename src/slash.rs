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
        marker::{ChannelMarker, CommandMarker, GuildMarker, UserMarker},
    },
};
use twilight_validate::command;

use crate::{
    ErrorSource, Result, Timing,
    hooks::Hooks,
    ui::{ComponentsV2, Modal, UiMessage},
};

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
        let name = Some(data.name.as_str());
        let Some(handler) = self.handlers.get(&data.id) else {
            hooks.reporter.report_text(
                ErrorSource::SlashCommand,
                name,
                "Akari に登録されていないスラッシュコマンドが実行されました",
            );
            if let Some(text) = &hooks.messages.slash_command_unavailable
                && let Err(error) = send_initial_message(http, interaction, text, true).await
            {
                hooks.reporter.report(ErrorSource::Response, name, &*error);
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
            hooks
                .reporter
                .report(ErrorSource::SlashCommand, name, &*error);
        }
        // 応答が済んでいない（エラー、または応答の忘れ）ときは、文言が設定されていれば知らせる。
        let Some(text) = &hooks.messages.slash_command_failed else {
            return;
        };
        let fallback = match *ctx.response_state {
            ResponseState::Pending => ctx.reply_ephemeral(text).await,
            ResponseState::Deferred { .. } => ctx.reply(text).await,
            ResponseState::Replied | ResponseState::ModalOpened => Ok(()),
        };
        if let Err(error) = fallback {
            hooks.reporter.report(ErrorSource::Response, name, &*error);
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

    /// コマンドが呼ばれたチャンネルの ID を返します。
    /// 古い形式のペイロードでは、チャンネルオブジェクトに代わって送られる ID を使います。
    #[allow(deprecated)]
    pub fn channel_id(&self) -> Option<Id<ChannelMarker>> {
        self.interaction
            .channel
            .as_ref()
            .map(|channel| channel.id)
            .or(self.interaction.channel_id)
    }

    /// Discord の REST API を直接使うために、Bot の Twilight クライアントを借ります。
    pub fn http(&self) -> &'a Client {
        self.http
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
            return Err("フォームはスラッシュコマンドの初回応答で開いてください".into());
        }
        let response = modal.response()?;
        self.http
            .interaction(self.interaction.application_id)
            .create_response(self.interaction.id, &self.interaction.token, &response)
            .await?;
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
        self.response_state
            .defer(self.http, self.interaction, ephemeral)
            .await
    }

    async fn respond(&mut self, content: &str, ephemeral: bool) -> Result<()> {
        self.response_state
            .reply(self.http, self.interaction, content, ephemeral)
            .await
    }

    async fn respond_ui(&mut self, message: &UiMessage, ephemeral: bool) -> Result<()> {
        message.validate()?;
        match self.response_state.reply_action(ephemeral)? {
            ReplyAction::Initial => {
                let response = initial_response(InitialResponse::Ui(message), ephemeral);
                self.http
                    .interaction(self.interaction.application_id)
                    .create_response(self.interaction.id, &self.interaction.token, &response)
                    .await?;
            }
            ReplyAction::EditDeferred => {
                edit_ui_response(self.http, self.interaction, message).await?;
            }
        }
        *self.response_state = ResponseState::Replied;
        Ok(())
    }

    async fn send_followup(&self, content: &str, ephemeral: bool) -> Result<()> {
        self.response_state
            .followup(self.http, self.interaction, content, ephemeral)
            .await
    }

    async fn respond_v2(&mut self, message: &ComponentsV2, ephemeral: bool) -> Result<()> {
        message.validate()?;
        match self.response_state.reply_action(ephemeral)? {
            ReplyAction::Initial => {
                let response = initial_response(InitialResponse::V2(message), ephemeral);
                self.http
                    .interaction(self.interaction.application_id)
                    .create_response(self.interaction.id, &self.interaction.token, &response)
                    .await?;
            }
            ReplyAction::EditDeferred => {
                edit_v2_response(self.http, self.interaction, message).await?
            }
        }
        *self.response_state = ResponseState::Replied;
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
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
                Err("遅延応答の公開範囲は変更できません".into())
            }
            Self::Deferred { .. } => Ok(ReplyAction::EditDeferred),
            Self::Replied | Self::ModalOpened => Err("この操作にはすでに応答しました".into()),
        }
    }

    pub(crate) async fn defer(
        &mut self,
        http: &Client,
        interaction: &Interaction,
        ephemeral: bool,
    ) -> Result<()> {
        if *self != Self::Pending {
            return Err("この操作にはすでに初回応答を送りました".into());
        }
        send_deferred_message(http, interaction, ephemeral).await?;
        *self = Self::Deferred { ephemeral };
        Ok(())
    }

    pub(crate) async fn reply(
        &mut self,
        http: &Client,
        interaction: &Interaction,
        content: &str,
        ephemeral: bool,
    ) -> Result<()> {
        match self.reply_action(ephemeral)? {
            ReplyAction::Initial => {
                send_initial_message(http, interaction, content, ephemeral).await?
            }
            ReplyAction::EditDeferred => edit_text_response(http, interaction, content).await?,
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
            return Err("追加メッセージを送る前に reply で初回メッセージを送ってください".into());
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

async fn send_deferred_message(
    http: &Client,
    interaction: &Interaction,
    ephemeral: bool,
) -> Result<()> {
    let response = initial_response(InitialResponse::Deferred, ephemeral);
    http.interaction(interaction.application_id)
        .create_response(interaction.id, &interaction.token, &response)
        .await?;
    Ok(())
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
    use super::{
        AllowedMentions, CommandOptionType, CommandType, Id, InitialResponse,
        InteractionResponseType, MessageFlags, ReplyAction, ResponseState, Result, SlashCommands,
        SlashContext, initial_response, v2_update_response,
    };
    use crate::ui::{Button, ComponentsV2, Embed, UiMessage};

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
