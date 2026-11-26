use std::{
    collections::{HashMap, hash_map::Entry},
    future::Future,
    pin::Pin,
};

use twilight_http::Client;
use twilight_model::{
    application::interaction::{
        Interaction, InteractionData, InteractionDataResolved, InteractionType,
        message_component::MessageComponentInteractionData,
        modal::{ModalInteractionComponent, ModalInteractionData},
    },
    channel::{
        Message,
        message::{AllowedMentions, MessageFlags, component::ComponentType},
    },
    http::interaction::{InteractionResponse, InteractionResponseData, InteractionResponseType},
    id::{
        Id,
        marker::{ChannelMarker, GenericMarker, GuildMarker, RoleMarker, UserMarker},
    },
};

mod owner;

use crate::{
    Error, ErrorSource, Messages, Result, Timing,
    hooks::Hooks,
    slash::{
        ResponseState, edit_ui_response, edit_v2_response, send_followup_message,
        send_initial_message, v2_update_response,
    },
    ui::{ComponentsV2, Modal, UiMessage},
};

#[derive(Default)]
pub(crate) struct InteractionHandlers {
    buttons: HashMap<String, Box<dyn ButtonHandler>>,
    selects: HashMap<String, Box<dyn SelectHandler>>,
    user_selects: HashMap<String, Box<dyn EntitySelectHandler<UserMarker>>>,
    role_selects: HashMap<String, Box<dyn EntitySelectHandler<RoleMarker>>>,
    channel_selects: HashMap<String, Box<dyn EntitySelectHandler<ChannelMarker>>>,
    mentionable_selects: HashMap<String, Box<dyn MentionableSelectHandler>>,
    modals: HashMap<String, Box<dyn ModalHandler>>,
}

impl std::fmt::Debug for InteractionHandlers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InteractionHandlers")
            .field("buttons", &self.buttons.len())
            .field("selects", &self.selects.len())
            .field("user_selects", &self.user_selects.len())
            .field("role_selects", &self.role_selects.len())
            .field("channel_selects", &self.channel_selects.len())
            .field("mentionable_selects", &self.mentionable_selects.len())
            .field("modals", &self.modals.len())
            .finish()
    }
}

impl InteractionHandlers {
    pub(crate) fn register_button(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut ButtonContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        register_handler(&mut self.buttons, "ボタン", custom_id, Box::new(handler))
    }

    pub(crate) fn register_select(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut SelectContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        register_handler(&mut self.selects, "セレクト", custom_id, Box::new(handler))
    }

    pub(crate) fn register_user_select(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut UserSelectContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        register_handler(
            &mut self.user_selects,
            "ユーザーセレクト",
            custom_id,
            Box::new(handler),
        )
    }

    pub(crate) fn register_role_select(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut RoleSelectContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        register_handler(
            &mut self.role_selects,
            "ロールセレクト",
            custom_id,
            Box::new(handler),
        )
    }

    pub(crate) fn register_channel_select(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut ChannelSelectContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        register_handler(
            &mut self.channel_selects,
            "チャンネルセレクト",
            custom_id,
            Box::new(handler),
        )
    }

    pub(crate) fn register_mentionable_select(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut MentionableSelectContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        register_handler(
            &mut self.mentionable_selects,
            "メンショナブルセレクト",
            custom_id,
            Box::new(handler),
        )
    }

    pub(crate) fn register_modal(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut ModalContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        register_handler(&mut self.modals, "モーダル", custom_id, Box::new(handler))
    }

    pub(crate) fn has_handlers(&self) -> bool {
        !(self.buttons.is_empty()
            && self.selects.is_empty()
            && self.user_selects.is_empty()
            && self.role_selects.is_empty()
            && self.channel_selects.is_empty()
            && self.mentionable_selects.is_empty()
            && self.modals.is_empty())
    }

    pub(crate) fn accepts(&self, interaction: &Interaction) -> bool {
        match ui_input(interaction) {
            Some(UiInput::Button(id)) => self.buttons.contains_key(id),
            Some(UiInput::Select(data)) => self.selects.contains_key(&data.custom_id),
            Some(UiInput::UserSelect(data)) => self.user_selects.contains_key(&data.custom_id),
            Some(UiInput::RoleSelect(data)) => self.role_selects.contains_key(&data.custom_id),
            Some(UiInput::ChannelSelect(data)) => {
                self.channel_selects.contains_key(&data.custom_id)
            }
            Some(UiInput::MentionableSelect(data)) => {
                self.mentionable_selects.contains_key(&data.custom_id)
            }
            Some(UiInput::Modal(data)) => self.modals.contains_key(&data.custom_id),
            None => false,
        }
    }

    pub(crate) async fn dispatch(
        &self,
        http: &Client,
        interaction: &Interaction,
        timing: Timing,
        hooks: &Hooks,
    ) {
        match ui_input(interaction) {
            Some(UiInput::Button(id)) => {
                let Some(handler) = self.buttons.get(id) else {
                    return;
                };
                let mut ctx = ButtonContext::new(http, interaction, id, timing);
                let result = handler.call(&mut ctx).await;
                finish_component(
                    &mut ctx,
                    result,
                    ErrorSource::Button,
                    &hooks.messages.button_failed,
                    hooks,
                )
                .await;
            }
            Some(UiInput::Select(data)) => {
                let Some(handler) = self.selects.get(&data.custom_id) else {
                    return;
                };
                let mut ctx = SelectContext {
                    inner: ButtonContext::new(http, interaction, &data.custom_id, timing),
                    values: &data.values,
                };
                let result = handler.call(&mut ctx).await;
                finish_component(
                    &mut ctx.inner,
                    result,
                    ErrorSource::Select,
                    &hooks.messages.select_failed,
                    hooks,
                )
                .await;
            }
            Some(UiInput::UserSelect(data)) => {
                dispatch_entity(&self.user_selects, http, interaction, data, timing, hooks).await;
            }
            Some(UiInput::RoleSelect(data)) => {
                dispatch_entity(&self.role_selects, http, interaction, data, timing, hooks).await;
            }
            Some(UiInput::ChannelSelect(data)) => {
                dispatch_entity(
                    &self.channel_selects,
                    http,
                    interaction,
                    data,
                    timing,
                    hooks,
                )
                .await;
            }
            Some(UiInput::MentionableSelect(data)) => {
                let Some(handler) = self.mentionable_selects.get(&data.custom_id) else {
                    return;
                };
                let mut inner = ButtonContext::new(http, interaction, &data.custom_id, timing);
                let resolved = data.resolved.as_ref();
                let Ok(values) = parse_mentionables(&data.values, resolved) else {
                    reject_selection(&mut inner, hooks).await;
                    return;
                };
                let mut ctx = MentionableSelectContext {
                    inner,
                    values,
                    resolved,
                };
                let result = handler.call(&mut ctx).await;
                finish_component(
                    &mut ctx.inner,
                    result,
                    ErrorSource::Select,
                    &hooks.messages.select_failed,
                    hooks,
                )
                .await;
            }
            Some(UiInput::Modal(data)) => {
                let Some(handler) = self.modals.get(&data.custom_id) else {
                    return;
                };
                let mut ctx = ModalContext {
                    http,
                    interaction,
                    data,
                    timing,
                    response: ResponseState::Pending,
                };
                let result = handler.call(&mut ctx).await;
                let name = Some(data.custom_id.as_str());
                if let Err(error) = &result {
                    hooks.reporter.report(ErrorSource::Modal, name, error);
                }
                if let Some((content, ephemeral)) =
                    modal_fallback(ctx.response, result.is_ok(), &hooks.messages)
                    && let Err(error) = ctx
                        .response
                        .reply(http, interaction, content, ephemeral)
                        .await
                {
                    hooks.reporter.report(ErrorSource::Response, name, &error);
                }
            }
            None => {}
        }
    }
}

async fn dispatch_entity<M>(
    handlers: &HashMap<String, Box<dyn EntitySelectHandler<M>>>,
    http: &Client,
    interaction: &Interaction,
    data: &MessageComponentInteractionData,
    timing: Timing,
    hooks: &Hooks,
) {
    let Some(handler) = handlers.get(&data.custom_id) else {
        return;
    };
    let mut inner = ButtonContext::new(http, interaction, &data.custom_id, timing);
    let Ok(values) = parse_entity_ids(&data.values) else {
        reject_selection(&mut inner, hooks).await;
        return;
    };
    let mut ctx = EntitySelectContext {
        inner,
        values,
        resolved: data.resolved.as_ref(),
    };
    let result = handler.call(&mut ctx).await;
    finish_component(
        &mut ctx.inner,
        result,
        ErrorSource::Select,
        &hooks.messages.select_failed,
        hooks,
    )
    .await;
}

/// 選択内容を読み取れなかったことを報告し、文言が設定されていれば本人にだけ知らせる。
async fn reject_selection(ctx: &mut ButtonContext<'_>, hooks: &Hooks) {
    let name = Some(ctx.custom_id());
    hooks.reporter.report(
        ErrorSource::Select,
        name,
        &Error::InvalidSelection("Discord が送った選択内容を読み取れませんでした"),
    );
    if let Some(text) = &hooks.messages.select_failed
        && let Err(error) = ctx.reply_ephemeral(text).await
    {
        hooks.reporter.report(ErrorSource::Response, name, &error);
    }
}

fn parse_entity_ids<M>(values: &[String]) -> Result<Vec<Id<M>>> {
    parse_select_values(values, |id| Ok(id.cast()))
}

fn parse_select_values<T: Eq>(
    values: &[String],
    resolve: impl Fn(Id<GenericMarker>) -> Result<T>,
) -> Result<Vec<T>> {
    if values.len() > 25 {
        return Err(Error::InvalidSelection(
            "セレクトの選択数が上限を超えています",
        ));
    }
    let mut selections = Vec::with_capacity(values.len());
    for value in values {
        if value.starts_with('0') || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(Error::InvalidSelection(
                "セレクトの値が Discord ID ではありません",
            ));
        }
        let id = value
            .parse()
            .map_err(|_| Error::InvalidSelection("セレクトの値が Discord ID ではありません"))?;
        let selection = resolve(id)?;
        if selections.contains(&selection) {
            return Err(Error::InvalidSelection("セレクトの値が重複しています"));
        }
        selections.push(selection);
    }
    Ok(selections)
}

fn parse_mentionables(
    values: &[String],
    resolved: Option<&InteractionDataResolved>,
) -> Result<Vec<Mentionable>> {
    parse_select_values(values, |id| {
        let resolved = resolved.ok_or(Error::InvalidSelection(
            "メンショナブルの解決済みデータがありません",
        ))?;
        let user_id = id.cast();
        let role_id = id.cast();
        match (resolved.users.get(&user_id), resolved.roles.get(&role_id)) {
            (Some(user), None) if user.id == user_id => Ok(Mentionable::User(user_id)),
            (None, Some(role)) if role.id == role_id => Ok(Mentionable::Role(role_id)),
            _ => Err(Error::InvalidSelection(
                "メンショナブルの値をユーザーまたはロールの一方に特定できません",
            )),
        }
    })
}

/// 入力フォームの関数が応答しなかったときに、自動で送る文言と、本人にだけ見せるかどうか。
fn modal_fallback(
    state: ResponseState,
    succeeded: bool,
    messages: &Messages,
) -> Option<(&str, bool)> {
    let content = if succeeded {
        messages.modal_accepted.as_deref()?
    } else {
        messages.modal_failed.as_deref()?
    };
    match state {
        ResponseState::Pending => Some((content, true)),
        ResponseState::Deferred { .. } => Some((content, false)),
        ResponseState::Replied | ResponseState::ModalOpened => None,
    }
}

fn register_handler<T: ?Sized>(
    handlers: &mut HashMap<String, Box<T>>,
    label: &str,
    custom_id: &str,
    handler: Box<T>,
) -> Result<()> {
    if !(1..=100).contains(&custom_id.chars().count()) {
        return Err(Error::InvalidRegistration(format!(
            "{label} ID は 1 文字以上 100 文字以下で指定してください"
        )));
    }
    match handlers.entry(custom_id.into()) {
        Entry::Vacant(entry) => {
            entry.insert(handler);
            Ok(())
        }
        Entry::Occupied(_) => Err(Error::InvalidRegistration(format!(
            "{label} ID `{custom_id}` は登録済みです"
        ))),
    }
}

/// ボタンや選択メニューの関数の結果を処理する。
///
/// 失敗はエラーの報告先へ渡し、文言が設定されていれば本人にだけ知らせる。
/// 成功して応答がなければ、メッセージを出さずに操作を受け付ける。
async fn finish_component(
    ctx: &mut ButtonContext<'_>,
    result: Result<()>,
    source: ErrorSource,
    failure: &Option<String>,
    hooks: &Hooks,
) {
    let name = Some(ctx.custom_id());
    match result {
        Err(error) => {
            hooks.reporter.report(source, name, &error);
            let Some(text) = failure else {
                return;
            };
            let fallback = match ctx.response {
                ButtonResponse::Pending => ctx.reply_ephemeral(text).await,
                ButtonResponse::DeferredUpdate => ctx.followup_ephemeral(text).await,
                ButtonResponse::Sent => Ok(()),
            };
            if let Err(error) = fallback {
                hooks.reporter.report(ErrorSource::Response, name, &error);
            }
        }
        Ok(()) if ctx.response == ButtonResponse::Pending => {
            if let Err(error) = ctx.acknowledge().await {
                hooks.reporter.report(ErrorSource::Response, name, &error);
            }
        }
        Ok(()) => {}
    }
}

enum UiInput<'a> {
    Button(&'a str),
    Select(&'a MessageComponentInteractionData),
    UserSelect(&'a MessageComponentInteractionData),
    RoleSelect(&'a MessageComponentInteractionData),
    ChannelSelect(&'a MessageComponentInteractionData),
    MentionableSelect(&'a MessageComponentInteractionData),
    Modal(&'a ModalInteractionData),
}

fn ui_input(interaction: &Interaction) -> Option<UiInput<'_>> {
    match (interaction.kind, interaction.data.as_ref()) {
        (InteractionType::MessageComponent, Some(InteractionData::MessageComponent(data))) => {
            match data.component_type {
                ComponentType::Button => Some(UiInput::Button(&data.custom_id)),
                ComponentType::TextSelectMenu => Some(UiInput::Select(data)),
                ComponentType::UserSelectMenu => Some(UiInput::UserSelect(data)),
                ComponentType::RoleSelectMenu => Some(UiInput::RoleSelect(data)),
                ComponentType::ChannelSelectMenu => Some(UiInput::ChannelSelect(data)),
                ComponentType::MentionableSelectMenu => Some(UiInput::MentionableSelect(data)),
                _ => None,
            }
        }
        (InteractionType::ModalSubmit, Some(InteractionData::ModalSubmit(data))) => {
            Some(UiInput::Modal(data))
        }
        _ => None,
    }
}

/// ボタンが一回押されたときにハンドラーへ渡す情報です。
pub struct ButtonContext<'a> {
    http: &'a Client,
    interaction: &'a Interaction,
    custom_id: &'a str,
    timing: Timing,
    response: ButtonResponse,
}

impl std::fmt::Debug for ButtonContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ButtonContext")
            .field("interaction_id", &self.interaction.id)
            .field("timing", &self.timing)
            .field("custom_id", &self.custom_id)
            .field("response", &self.response)
            .finish_non_exhaustive()
    }
}

impl<'a> ButtonContext<'a> {
    fn new(
        http: &'a Client,
        interaction: &'a Interaction,
        custom_id: &'a str,
        timing: Timing,
    ) -> Self {
        Self {
            http,
            interaction,
            custom_id,
            timing,
            response: ButtonResponse::Pending,
        }
    }

    /// Akari が使用している Twilight の REST クライアントを借ります。
    pub fn http(&self) -> &'a Client {
        self.http
    }

    /// ボタン作成時に設定した ID を返します。
    pub fn custom_id(&self) -> &'a str {
        self.custom_id
    }

    /// ボタンを押したユーザーの ID を返します。
    pub fn author_id(&self) -> Option<Id<UserMarker>> {
        self.interaction.author_id()
    }

    /// 元メッセージを作るきっかけになったユーザーの ID を返します。
    /// Interaction の情報、または Bot が返信した実際の人間のメッセージが必要です。
    /// ボタンを押したユーザーは [`Self::author_id`] で取得できます。
    pub fn invoker_id(&self) -> Option<Id<UserMarker>> {
        owner::message_owner(self.message()?)
    }

    /// 元の実行者 ID を確認します。操作データに返信先の投稿がなければ Discord から取得します。
    /// HTTP の完了を待つ前に [`Self::defer_update`] で操作を受け付けられます。
    ///
    /// # エラー
    /// 閲覧権限不足や通信エラーで返信先の取得に失敗した場合にエラーを返します。
    /// 返信先が削除されている場合や返信先を照合できない場合は `Ok(None)` です。
    pub async fn resolve_invoker_id(&self) -> Result<Option<Id<UserMarker>>> {
        let Some(message) = self.message() else {
            return Ok(None);
        };
        owner::resolve_owner(self.http, message).await
    }

    /// ボタンが押されたサーバーの ID を返します。DM では `None` です。
    pub fn guild_id(&self) -> Option<Id<GuildMarker>> {
        self.interaction.guild_id
    }

    /// ボタンが付いた Discord メッセージを借ります。
    pub fn message(&self) -> Option<&'a Message> {
        self.interaction.message.as_ref()
    }

    /// Gateway の往復時間と、ボタン操作を処理するまでの時間を返します。
    pub fn timing(&self) -> Timing {
        self.timing
    }

    /// ボタンを押した本人にだけ表示する初回応答を送ります。
    ///
    /// # エラー
    ///
    /// 応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn reply_ephemeral(&mut self, content: &str) -> Result<()> {
        self.response.require_pending()?;
        send_initial_message(self.http, self.interaction, content, true).await?;
        self.response = ButtonResponse::Sent;
        Ok(())
    }

    /// メッセージを変更せずに操作を受け付けます。[`Self::defer_update`] の別名です。
    ///
    /// # エラー
    ///
    /// 応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn acknowledge(&mut self) -> Result<()> {
        self.defer_update().await
    }

    /// ボタン操作を先に受け付け、元のメッセージを後から更新できるようにします。
    /// 時間のかかる処理を始める前に呼び、その後 [`Self::update_message`] で反映します。
    ///
    /// # エラー
    /// 初回応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn defer_update(&mut self) -> Result<()> {
        self.response.require_pending()?;
        let response = InteractionResponse {
            kind: InteractionResponseType::DeferredUpdateMessage,
            data: None,
        };
        self.http
            .interaction(self.interaction.application_id)
            .create_response(self.interaction.id, &self.interaction.token, &response)
            .await?;
        self.response = ButtonResponse::DeferredUpdate;
        Ok(())
    }

    /// ボタンが付いたメッセージの本文、Embed、コンポーネントを置き換えます。
    /// 省略した本文や Embed、コンポーネントは取り除かれます。`defer_update` 後にも一度呼べます。
    ///
    /// # エラー
    /// 更新できない応答状態、無効なメッセージ、または Discord が受け付けない場合に失敗します。
    pub async fn update_message(&mut self, message: &UiMessage) -> Result<()> {
        let action = self.response.update_action()?;
        if self.uses_components_v2() {
            return Err(Error::InvalidResponseState(
                "Components V2 のメッセージには update_v2 を使ってください",
            ));
        }
        message.validate()?;
        match action {
            UpdateAction::Initial => {
                self.http
                    .interaction(self.interaction.application_id)
                    .create_response(
                        self.interaction.id,
                        &self.interaction.token,
                        &update_response(message),
                    )
                    .await?;
            }
            UpdateAction::Edit => edit_ui_response(self.http, self.interaction, message).await?,
        }
        self.response = ButtonResponse::Sent;
        Ok(())
    }

    /// 元の Components V2 メッセージのコンポーネントを置き換えます。
    /// `defer_update` 後にも一度呼べます。
    ///
    /// # エラー
    /// 更新できない応答状態、元メッセージが Components V2 以外、無効なコンポーネント、または通信の失敗でエラーになります。
    pub async fn update_v2(&mut self, message: &ComponentsV2) -> Result<()> {
        let action = self.response.update_action()?;
        if !self.uses_components_v2() {
            return Err(Error::InvalidResponseState(
                "Components V2 で作られた元メッセージが必要です",
            ));
        }
        message.validate()?;
        match action {
            UpdateAction::Initial => {
                self.http
                    .interaction(self.interaction.application_id)
                    .create_response(
                        self.interaction.id,
                        &self.interaction.token,
                        &v2_update_response(message),
                    )
                    .await?;
            }
            UpdateAction::Edit => edit_v2_response(self.http, self.interaction, message).await?,
        }
        self.response = ButtonResponse::Sent;
        Ok(())
    }

    /// 初回応答としてモーダルを開きます。
    ///
    /// # エラー
    /// 初回応答済み、無効なモーダル、または Discord が受け付けなかった場合に失敗します。
    pub async fn show_modal(&mut self, modal: &Modal) -> Result<()> {
        self.response.require_pending()?;
        let response = modal.response()?;
        self.http
            .interaction(self.interaction.application_id)
            .create_response(self.interaction.id, &self.interaction.token, &response)
            .await?;
        self.response = ButtonResponse::Sent;
        Ok(())
    }

    fn uses_components_v2(&self) -> bool {
        self.message()
            .and_then(|message| message.flags)
            .is_some_and(|flags| flags.contains(MessageFlags::IS_COMPONENTS_V2))
    }

    /// 操作を受け付けた後、全員に見える追加メッセージを送ります。
    ///
    /// # エラー
    /// 初回応答前、または Discord が受け付けなかった場合に失敗します。
    pub async fn followup(&self, content: &str) -> Result<()> {
        self.send_followup(content, false).await
    }

    /// 操作を受け付けた後、ボタンを押した本人にだけ追加メッセージを送ります。
    ///
    /// # エラー
    /// 初回応答前、または Discord が受け付けなかった場合に失敗します。
    pub async fn followup_ephemeral(&self, content: &str) -> Result<()> {
        self.send_followup(content, true).await
    }

    async fn send_followup(&self, content: &str, ephemeral: bool) -> Result<()> {
        if self.response == ButtonResponse::Pending {
            return Err(Error::InvalidResponseState(
                "追加メッセージを送る前にボタン操作に応答してください",
            ));
        }
        send_followup_message(self.http, self.interaction, content, ephemeral).await
    }
}

/// 文字列セレクトを一回選んだときにハンドラーへ渡す情報です。
#[derive(Debug)]
pub struct SelectContext<'a> {
    inner: ButtonContext<'a>,
    values: &'a [String],
}

impl<'a> SelectContext<'a> {
    /// セレクトメニューの ID を返します。
    pub fn custom_id(&self) -> &'a str {
        self.inner.custom_id()
    }

    /// ユーザーが選択した値を借ります。何も選ばれていなければ空です。
    pub fn values(&self) -> &'a [String] {
        self.values
    }

    /// セレクトを操作したユーザーの ID を返します。
    pub fn author_id(&self) -> Option<Id<UserMarker>> {
        self.inner.author_id()
    }

    /// 元メッセージを作るきっかけになったユーザーの ID を返します。
    /// 判定方法は [`ButtonContext::invoker_id`] と同じです。
    pub fn invoker_id(&self) -> Option<Id<UserMarker>> {
        self.inner.invoker_id()
    }

    /// 返信先の投稿が省略されていれば取得します。詳細は [`ButtonContext::resolve_invoker_id`] を参照してください。
    ///
    /// # エラー
    /// Discord から返信先の投稿を取得できない場合にエラーを返します。
    pub async fn resolve_invoker_id(&self) -> Result<Option<Id<UserMarker>>> {
        self.inner.resolve_invoker_id().await
    }

    /// セレクトが操作されたサーバーの ID を返します。DM では `None` です。
    pub fn guild_id(&self) -> Option<Id<GuildMarker>> {
        self.inner.guild_id()
    }

    /// セレクトが付いた Discord メッセージを借ります。
    pub fn message(&self) -> Option<&'a Message> {
        self.inner.message()
    }

    /// Gateway の往復時間と、セレクト操作を処理するまでの時間を返します。
    pub fn timing(&self) -> Timing {
        self.inner.timing()
    }

    /// Akari が使用している Twilight の REST クライアントを借ります。
    pub fn http(&self) -> &'a Client {
        self.inner.http()
    }

    /// セレクトを操作した本人にだけ表示する初回応答を送ります。
    ///
    /// # エラー
    /// 応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn reply_ephemeral(&mut self, content: &str) -> Result<()> {
        self.inner.reply_ephemeral(content).await
    }

    /// セレクトが付いたメッセージの本文、Embed、コンポーネントを置き換えます。
    ///
    /// # エラー
    /// 更新できない応答状態、無効なメッセージ、または Discord が受け付けなかった場合に失敗します。
    pub async fn update_message(&mut self, message: &UiMessage) -> Result<()> {
        self.inner.update_message(message).await
    }

    /// 元の Components V2 メッセージのコンポーネントを置き換えます。
    ///
    /// # エラー
    /// 更新できない応答状態、元メッセージが Components V2 以外、無効なコンポーネント、または通信の失敗でエラーになります。
    pub async fn update_v2(&mut self, message: &ComponentsV2) -> Result<()> {
        self.inner.update_v2(message).await
    }

    /// セレクト操作を先に受け付け、元のメッセージを後から変更できるようにします。
    ///
    /// # エラー
    /// 初回応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn defer_update(&mut self) -> Result<()> {
        self.inner.defer_update().await
    }

    /// 初回応答としてモーダルを開きます。
    ///
    /// # エラー
    /// 初回応答済み、無効なモーダル、または Discord が受け付けなかった場合に失敗します。
    pub async fn show_modal(&mut self, modal: &Modal) -> Result<()> {
        self.inner.show_modal(modal).await
    }
}

/// ユーザー・ロール・チャンネルを選択したときにハンドラーへ渡す情報です。
#[derive(Debug)]
pub struct EntitySelectContext<'a, M> {
    inner: ButtonContext<'a>,
    values: Vec<Id<M>>,
    resolved: Option<&'a InteractionDataResolved>,
}

/// ユーザーセレクトが選択されたときの情報です。
pub type UserSelectContext<'a> = EntitySelectContext<'a, UserMarker>;

/// ロールセレクトが選択されたときの情報です。
pub type RoleSelectContext<'a> = EntitySelectContext<'a, RoleMarker>;

/// チャンネルセレクトが選択されたときの情報です。
pub type ChannelSelectContext<'a> = EntitySelectContext<'a, ChannelMarker>;

/// メンショナブルセレクトで選択できるユーザーまたはロールです。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
#[non_exhaustive]
pub enum Mentionable {
    /// 選択されたユーザーの ID です。
    User(Id<UserMarker>),
    /// 選択されたロールの ID です。
    Role(Id<RoleMarker>),
}

/// メンショナブルセレクトを操作したときにハンドラーへ渡す情報です。
#[derive(Debug)]
pub struct MentionableSelectContext<'a> {
    inner: ButtonContext<'a>,
    values: Vec<Mentionable>,
    resolved: Option<&'a InteractionDataResolved>,
}

macro_rules! typed_select_methods {
    ($lifetime:lifetime) => {
        /// セレクトメニューの ID を返します。
        pub fn custom_id(&self) -> &$lifetime str {
            self.inner.custom_id()
        }

        /// Discord が解決済みの情報を送った場合に、その情報を借ります。
        pub fn resolved(&self) -> Option<&$lifetime InteractionDataResolved> {
            self.resolved
        }

        /// セレクトを操作したユーザーの ID を返します。
        pub fn author_id(&self) -> Option<Id<UserMarker>> {
            self.inner.author_id()
        }

        /// 元メッセージを作るきっかけになったユーザーの ID を返します。
        /// 判定方法は [`ButtonContext::invoker_id`] と同じです。
        pub fn invoker_id(&self) -> Option<Id<UserMarker>> {
            self.inner.invoker_id()
        }

        /// 返信先の投稿が省略されていれば取得します。詳細は [`ButtonContext::resolve_invoker_id`] を参照してください。
        ///
        /// # エラー
        /// Discord から返信先の投稿を取得できない場合にエラーを返します。
        pub async fn resolve_invoker_id(&self) -> Result<Option<Id<UserMarker>>> {
            self.inner.resolve_invoker_id().await
        }

        /// セレクトが操作されたサーバーの ID を返します。DM では `None` です。
        pub fn guild_id(&self) -> Option<Id<GuildMarker>> {
            self.inner.guild_id()
        }

        /// セレクトが付いた Discord メッセージを借ります。
        pub fn message(&self) -> Option<&$lifetime Message> {
            self.inner.message()
        }

        /// Gateway の往復時間と、セレクト操作を処理するまでの時間を返します。
        pub fn timing(&self) -> Timing {
            self.inner.timing()
        }

        /// Akari が使用している Twilight の REST クライアントを借ります。
        pub fn http(&self) -> &$lifetime Client {
            self.inner.http()
        }

        /// セレクトを操作した本人にだけ表示する初回応答を送ります。
        ///
        /// # エラー
        /// 応答済み、または Discord が受け付けなかった場合に失敗します。
        pub async fn reply_ephemeral(&mut self, content: &str) -> Result<()> {
            self.inner.reply_ephemeral(content).await
        }

        /// セレクトが付いたメッセージの本文、Embed、コンポーネントを置き換えます。
        ///
        /// # エラー
        /// 更新できない応答状態、無効なメッセージ、または Discord が受け付けなかった場合に失敗します。
        pub async fn update_message(&mut self, message: &UiMessage) -> Result<()> {
            self.inner.update_message(message).await
        }

        /// 元の Components V2 メッセージのコンポーネントを置き換えます。
        ///
        /// # エラー
        /// 更新できない応答状態、元メッセージが Components V2 以外、無効なコンポーネント、または通信の失敗でエラーになります。
        pub async fn update_v2(&mut self, message: &ComponentsV2) -> Result<()> {
            self.inner.update_v2(message).await
        }

        /// セレクト操作を先に受け付け、元のメッセージを後から変更できるようにします。
        ///
        /// # エラー
        /// 初回応答済み、または Discord が受け付けなかった場合に失敗します。
        pub async fn defer_update(&mut self) -> Result<()> {
            self.inner.defer_update().await
        }

        /// 初回応答としてモーダルを開きます。
        ///
        /// # エラー
        /// 初回応答済み、無効なモーダル、または Discord が受け付けなかった場合に失敗します。
        pub async fn show_modal(&mut self, modal: &Modal) -> Result<()> {
            self.inner.show_modal(modal).await
        }
    };
}

impl<'a, M> EntitySelectContext<'a, M> {
    /// 選択された項目の Discord ID を借ります。選択項目がない場合は空です。
    pub fn values(&self) -> &[Id<M>] {
        &self.values
    }

    typed_select_methods!('a);
}

impl<'a> MentionableSelectContext<'a> {
    /// 選択されたユーザーとロールを Discord から届いた順序で借ります。
    pub fn values(&self) -> &[Mentionable] {
        &self.values
    }

    typed_select_methods!('a);
}

/// モーダルを一回送信したときにハンドラーへ渡す情報です。
pub struct ModalContext<'a> {
    http: &'a Client,
    interaction: &'a Interaction,
    data: &'a ModalInteractionData,
    timing: Timing,
    response: ResponseState,
}

impl std::fmt::Debug for ModalContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModalContext")
            .field("interaction_id", &self.interaction.id)
            .field("timing", &self.timing)
            .field("custom_id", &self.data.custom_id)
            .field("response", &self.response)
            .finish_non_exhaustive()
    }
}

impl<'a> ModalContext<'a> {
    /// モーダルの ID を返します。
    pub fn custom_id(&self) -> &'a str {
        &self.data.custom_id
    }

    /// 指定した入力欄に送信されたテキストを借ります。該当の入力欄がない場合は `None` です。
    pub fn text_input(&self, custom_id: &str) -> Option<&'a str> {
        self.data
            .components
            .iter()
            .find_map(|input| submitted_text(input, custom_id))
    }

    /// モーダルを送信したユーザーの ID を返します。
    pub fn author_id(&self) -> Option<Id<UserMarker>> {
        self.interaction.author_id()
    }

    /// モーダルが送信されたサーバーの ID を返します。DM では `None` です。
    pub fn guild_id(&self) -> Option<Id<GuildMarker>> {
        self.interaction.guild_id
    }

    /// Gateway の往復時間と、モーダル送信を処理するまでの時間を返します。
    pub fn timing(&self) -> Timing {
        self.timing
    }

    /// Akari が使用している Twilight の REST クライアントを借ります。
    pub fn http(&self) -> &'a Client {
        self.http
    }

    /// チャンネルに初回応答を送るか、遅延応答を元の公開範囲のまま完成させます。
    ///
    /// # エラー
    /// 応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn reply(&mut self, content: &str) -> Result<()> {
        self.response
            .reply(self.http, self.interaction, content, false)
            .await
    }

    /// モーダルを送信した本人にだけ応答するか、非公開の遅延応答を完成させます。
    ///
    /// # エラー
    /// 応答済み、公開の遅延応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn reply_ephemeral(&mut self, content: &str) -> Result<()> {
        self.response
            .reply(self.http, self.interaction, content, true)
            .await
    }

    /// チャンネルに処理中の表示を出します。作業が終わったら [`Self::reply`] で完成させます。
    ///
    /// # エラー
    /// 初回応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn defer(&mut self) -> Result<()> {
        self.response
            .defer(self.http, self.interaction, false)
            .await
    }

    /// 送信した本人にだけ処理中の表示を出します。
    ///
    /// # エラー
    /// 初回応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn defer_ephemeral(&mut self) -> Result<()> {
        self.response.defer(self.http, self.interaction, true).await
    }

    /// 初回メッセージを完成させた後、チャンネルに追加メッセージを送ります。
    ///
    /// # エラー
    /// 初回メッセージが未完成、または Discord が受け付けなかった場合に失敗します。
    pub async fn followup(&self, content: &str) -> Result<()> {
        self.response
            .followup(self.http, self.interaction, content, false)
            .await
    }

    /// 初回メッセージを完成させた後、送信した本人にだけ追加メッセージを送ります。
    ///
    /// # エラー
    /// 初回メッセージが未完成、または Discord が受け付けなかった場合に失敗します。
    pub async fn followup_ephemeral(&self, content: &str) -> Result<()> {
        self.response
            .followup(self.http, self.interaction, content, true)
            .await
    }
}

fn submitted_text<'a>(
    component: &'a ModalInteractionComponent,
    custom_id: &str,
) -> Option<&'a str> {
    match component {
        ModalInteractionComponent::ActionRow(row) => row
            .components
            .iter()
            .find_map(|child| submitted_text(child, custom_id)),
        ModalInteractionComponent::Label(label) => submitted_text(&label.component, custom_id),
        ModalInteractionComponent::TextInput(input) if input.custom_id == custom_id => {
            Some(&input.value)
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ButtonResponse {
    Pending,
    DeferredUpdate,
    Sent,
}

impl ButtonResponse {
    fn require_pending(self) -> Result<()> {
        if self == Self::Pending {
            Ok(())
        } else {
            Err(Error::InvalidResponseState(
                "この操作にはすでに初回応答を送りました",
            ))
        }
    }

    fn update_action(self) -> Result<UpdateAction> {
        match self {
            Self::Pending => Ok(UpdateAction::Initial),
            Self::DeferredUpdate => Ok(UpdateAction::Edit),
            Self::Sent => Err(Error::InvalidResponseState(
                "この操作では元のメッセージを更新できません",
            )),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum UpdateAction {
    Initial,
    Edit,
}

fn update_response(message: &UiMessage) -> InteractionResponse {
    InteractionResponse {
        kind: InteractionResponseType::UpdateMessage,
        data: Some(InteractionResponseData {
            allowed_mentions: Some(AllowedMentions::default()),
            content: Some(message.content.clone().unwrap_or_default()),
            embeds: Some(message.embeds.clone()),
            components: Some(message.components.clone()),
            ..Default::default()
        }),
    }
}

macro_rules! handler_for {
    ($handler:ident, $context:ident) => {
        trait $handler: Send + Sync {
            fn call<'a, 'ctx>(
                &'a self,
                ctx: &'a mut $context<'ctx>,
            ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>>;
        }

        impl<F> $handler for F
        where
            F: AsyncFn(&mut $context<'_>) -> Result<()> + Send + Sync,
        {
            fn call<'a, 'ctx>(
                &'a self,
                ctx: &'a mut $context<'ctx>,
            ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
                Box::pin(self(ctx))
            }
        }
    };
}

handler_for!(ButtonHandler, ButtonContext);
handler_for!(SelectHandler, SelectContext);
handler_for!(ModalHandler, ModalContext);
handler_for!(MentionableSelectHandler, MentionableSelectContext);

trait EntitySelectHandler<M>: Send + Sync {
    fn call<'a, 'ctx>(
        &'a self,
        ctx: &'a mut EntitySelectContext<'ctx, M>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>>;
}

impl<F, M> EntitySelectHandler<M> for F
where
    F: AsyncFn(&mut EntitySelectContext<'_, M>) -> Result<()> + Send + Sync,
{
    fn call<'a, 'ctx>(
        &'a self,
        ctx: &'a mut EntitySelectContext<'ctx, M>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        Box::pin(self(ctx))
    }
}

#[cfg(test)]
mod tests;
