use std::{
    collections::{HashMap, hash_map::Entry},
    future::Future,
    pin::Pin,
};

use twilight_http::Client;
use twilight_model::{
    application::interaction::{
        Interaction, InteractionData, InteractionDataResolved, InteractionMetadata,
        InteractionType,
        message_component::MessageComponentInteractionData,
        modal::{ModalInteractionComponent, ModalInteractionData},
    },
    channel::{
        Message,
        message::{
            AllowedMentions, MessageFlags, MessageReferenceType, MessageType,
            component::ComponentType,
        },
    },
    http::interaction::{InteractionResponse, InteractionResponseData, InteractionResponseType},
    id::{
        Id,
        marker::{ChannelMarker, GuildMarker, RoleMarker, UserMarker},
    },
};

use crate::{
    Result, Timing,
    slash::{
        ResponseState, edit_ui_response, edit_v2_response, send_followup_message,
        send_initial_message, v2_update_response,
    },
    ui::{ComponentsV2, Modal, UiMessage},
};

const BUTTON_FAILED: &str = "ボタンを処理できませんでした。";
const SELECT_FAILED: &str = "選択内容を処理できませんでした。";
const MODAL_FAILED: &str = "入力内容を処理できませんでした。";
const MODAL_ACCEPTED: &str = "入力を受け付けました。";

#[derive(Default)]
pub(crate) struct Buttons {
    handlers: HashMap<String, Box<dyn ButtonHandler>>,
    selects: HashMap<String, Box<dyn SelectHandler>>,
    user_selects: HashMap<String, Box<dyn EntitySelectHandler<UserMarker>>>,
    role_selects: HashMap<String, Box<dyn EntitySelectHandler<RoleMarker>>>,
    channel_selects: HashMap<String, Box<dyn EntitySelectHandler<ChannelMarker>>>,
    modals: HashMap<String, Box<dyn ModalHandler>>,
}

impl Buttons {
    pub(crate) fn register(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut ButtonContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        register_handler(&mut self.handlers, "ボタン", custom_id, Box::new(handler))
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

    pub(crate) fn register_modal(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut ModalContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        register_handler(&mut self.modals, "モーダル", custom_id, Box::new(handler))
    }

    pub(crate) fn has_handlers(&self) -> bool {
        !(self.handlers.is_empty()
            && self.selects.is_empty()
            && self.user_selects.is_empty()
            && self.role_selects.is_empty()
            && self.channel_selects.is_empty()
            && self.modals.is_empty())
    }

    pub(crate) fn accepts(&self, interaction: &Interaction) -> bool {
        match ui_input(interaction) {
            Some(UiInput::Button(id)) => self.handlers.contains_key(id),
            Some(UiInput::Select(data)) => self.selects.contains_key(&data.custom_id),
            Some(UiInput::UserSelect(data)) => self.user_selects.contains_key(&data.custom_id),
            Some(UiInput::RoleSelect(data)) => self.role_selects.contains_key(&data.custom_id),
            Some(UiInput::ChannelSelect(data)) => {
                self.channel_selects.contains_key(&data.custom_id)
            }
            Some(UiInput::Modal(data)) => self.modals.contains_key(&data.custom_id),
            None => false,
        }
    }

    pub(crate) async fn dispatch(&self, http: &Client, interaction: &Interaction, timing: Timing) {
        match ui_input(interaction) {
            Some(UiInput::Button(id)) => {
                let Some(handler) = self.handlers.get(id) else {
                    return;
                };
                let mut ctx = ButtonContext::new(http, interaction, id, timing);
                let result = handler.call(&mut ctx).await;
                finish_component(&mut ctx, result, "ボタン", BUTTON_FAILED).await;
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
                finish_component(&mut ctx.inner, result, "セレクト", SELECT_FAILED).await;
            }
            Some(UiInput::UserSelect(data)) => {
                dispatch_entity(&self.user_selects, http, interaction, data, timing).await;
            }
            Some(UiInput::RoleSelect(data)) => {
                dispatch_entity(&self.role_selects, http, interaction, data, timing).await;
            }
            Some(UiInput::ChannelSelect(data)) => {
                dispatch_entity(&self.channel_selects, http, interaction, data, timing).await;
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
                if let Err(error) = &result {
                    eprintln!("モーダルの処理に失敗しました ({error})");
                }
                if let Some((content, ephemeral)) = modal_fallback(ctx.response, result.is_ok())
                    && ctx
                        .response
                        .reply(http, interaction, content, ephemeral)
                        .await
                        .is_err()
                {
                    eprintln!("モーダルの自動応答に失敗しました");
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
) {
    let Some(handler) = handlers.get(&data.custom_id) else {
        return;
    };
    let mut inner = ButtonContext::new(http, interaction, &data.custom_id, timing);
    let Ok(values) = parse_entity_ids(&data.values) else {
        if inner.reply_ephemeral(SELECT_FAILED).await.is_err() {
            eprintln!("セレクトのエラー応答に失敗しました");
        }
        return;
    };
    let mut ctx = EntitySelectContext {
        inner,
        values,
        resolved: data.resolved.as_ref(),
    };
    let result = handler.call(&mut ctx).await;
    finish_component(&mut ctx.inner, result, "セレクト", SELECT_FAILED).await;
}

fn parse_entity_ids<M>(values: &[String]) -> Result<Vec<Id<M>>> {
    if values.len() > 25 {
        return Err("セレクトの選択数が上限を超えています".into());
    }
    let mut ids = Vec::with_capacity(values.len());
    for value in values {
        if value.starts_with('0') || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("セレクトの値が Discord ID ではありません".into());
        }
        let id = value.parse()?;
        if ids.contains(&id) {
            return Err("セレクトの値が重複しています".into());
        }
        ids.push(id);
    }
    Ok(ids)
}

fn modal_fallback(state: ResponseState, succeeded: bool) -> Option<(&'static str, bool)> {
    let content = if succeeded {
        MODAL_ACCEPTED
    } else {
        MODAL_FAILED
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
        return Err(format!("{label} ID は 1 文字以上 100 文字以下で指定してください").into());
    }
    match handlers.entry(custom_id.into()) {
        Entry::Vacant(entry) => {
            entry.insert(handler);
            Ok(())
        }
        Entry::Occupied(_) => Err(format!("{label} ID `{custom_id}` は登録済みです").into()),
    }
}

async fn finish_component(
    ctx: &mut ButtonContext<'_>,
    result: Result<()>,
    label: &str,
    failure: &str,
) {
    if let Err(error) = result {
        eprintln!("{label}の処理に失敗しました ({error})");
        let fallback = match ctx.response {
            ButtonResponse::Pending => ctx.reply_ephemeral(failure).await,
            ButtonResponse::DeferredUpdate => ctx.followup_ephemeral(failure).await,
            ButtonResponse::Sent => Ok(()),
        };
        if fallback.is_err() {
            eprintln!("{label}のエラー応答に失敗しました");
        }
    } else if ctx.response == ButtonResponse::Pending && ctx.acknowledge().await.is_err() {
        eprintln!("{label}の確認応答に失敗しました");
    }
}

enum UiInput<'a> {
    Button(&'a str),
    Select(&'a MessageComponentInteractionData),
    UserSelect(&'a MessageComponentInteractionData),
    RoleSelect(&'a MessageComponentInteractionData),
    ChannelSelect(&'a MessageComponentInteractionData),
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
        message_owner(self.message()?)
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
            return Err("Components V2 のメッセージには update_v2 を使ってください".into());
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
            return Err("Components V2 で作られた元メッセージが必要です".into());
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
            return Err("追加メッセージを送る前にボタン操作に応答してください".into());
        }
        send_followup_message(self.http, self.interaction, content, ephemeral).await
    }
}

fn message_invoker(metadata: &InteractionMetadata) -> Option<Id<UserMarker>> {
    match metadata.kind {
        InteractionType::ApplicationCommand
        | InteractionType::MessageComponent
        | InteractionType::ModalSubmit => Some(metadata.user.id),
        _ => None,
    }
}

fn message_owner(message: &Message) -> Option<Id<UserMarker>> {
    if let Some(metadata) = &message.interaction_metadata {
        return message_invoker(metadata);
    }

    if message.kind != MessageType::Reply || !message.author.bot || message.webhook_id.is_some() {
        return None;
    }
    let reference = message.reference.as_ref()?;
    let target = message.referenced_message.as_deref()?;
    (reference.kind == MessageReferenceType::Default
        && reference.message_id == Some(target.id)
        && reference.channel_id == Some(target.channel_id)
        && message.channel_id == target.channel_id
        && !target.author.bot
        && target.webhook_id.is_none())
    .then_some(target.author.id)
}

/// 文字列セレクトを一回選んだときにハンドラーへ渡す情報です。
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

impl<'a, M> EntitySelectContext<'a, M> {
    /// セレクトメニューの ID を返します。
    pub fn custom_id(&self) -> &'a str {
        self.inner.custom_id()
    }

    /// 選択された項目の Discord ID を借ります。選択項目がない場合は空です。
    pub fn values(&self) -> &[Id<M>] {
        &self.values
    }

    /// Discord が解決済みの情報を送った場合に、その情報を借ります。
    pub fn resolved(&self) -> Option<&'a InteractionDataResolved> {
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

/// モーダルを一回送信したときにハンドラーへ渡す情報です。
pub struct ModalContext<'a> {
    http: &'a Client,
    interaction: &'a Interaction,
    data: &'a ModalInteractionData,
    timing: Timing,
    response: ResponseState,
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

#[derive(Clone, Copy, PartialEq, Eq)]
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
            Err("この操作にはすでに初回応答を送りました".into())
        }
    }

    fn update_action(self) -> Result<UpdateAction> {
        match self {
            Self::Pending => Ok(UpdateAction::Initial),
            Self::DeferredUpdate => Ok(UpdateAction::Edit),
            Self::Sent => Err("この操作では元のメッセージを更新できません".into()),
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
mod tests {
    use super::{
        ButtonContext, ButtonResponse, Buttons, ComponentType, Id, Interaction, InteractionData,
        InteractionMetadata, InteractionResponseType, InteractionType, MODAL_ACCEPTED,
        MODAL_FAILED, Message, MessageComponentInteractionData, MessageReferenceType, MessageType,
        ModalContext, ModalInteractionComponent, ModalInteractionData, ResponseState, Result,
        SelectContext, UiMessage, UpdateAction, message_invoker, message_owner, modal_fallback,
        submitted_text, update_response,
    };
    use super::{
        ChannelMarker, ChannelSelectContext, RoleMarker, RoleSelectContext, UserMarker,
        UserSelectContext, parse_entity_ids,
    };
    use crate::ui::{Button, Embed};
    use twilight_model::{
        application::interaction::modal::{
            ModalInteractionActionRow, ModalInteractionLabel, ModalInteractionTextInput,
        },
        channel::message::MessageReference,
        oauth::ApplicationIntegrationMap,
        user::User,
        util::Timestamp,
    };

    async fn click(_: &mut ButtonContext<'_>) -> Result<()> {
        Ok(())
    }

    async fn select(_: &mut SelectContext<'_>) -> Result<()> {
        Ok(())
    }

    async fn submit(_: &mut ModalContext<'_>) -> Result<()> {
        Ok(())
    }

    #[allow(deprecated)]
    fn interaction(kind: InteractionType, data: InteractionData) -> Interaction {
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
            data: Some(data),
            entitlements: vec![],
            guild: None,
            guild_id: None,
            guild_locale: None,
            id: Id::new(1),
            kind,
            locale: None,
            member: None,
            message: None,
            token: String::new(),
            user: None,
        }
    }

    fn component_payload(component_type: ComponentType, id: &str) -> InteractionData {
        InteractionData::MessageComponent(Box::new(MessageComponentInteractionData {
            component_type,
            custom_id: id.into(),
            resolved: None,
            values: vec![],
        }))
    }

    #[test]
    fn routes_only_matching_kinds_and_exact_ids() -> Result<()> {
        let mut handlers = Buttons::default();
        handlers.register_select("shared", select)?;
        handlers.register_modal("shared", submit)?;
        assert!(handlers.has_handlers());
        assert!(handlers.register_select("shared", select).is_err());
        assert!(handlers.register_modal("shared", submit).is_err());
        assert!(handlers.register_select("", select).is_err());
        assert!(handlers.register_modal(&"a".repeat(101), submit).is_err());
        let menu = interaction(
            InteractionType::MessageComponent,
            component_payload(ComponentType::TextSelectMenu, "shared"),
        );
        assert!(handlers.accepts(&menu));
        let menu = interaction(
            InteractionType::MessageComponent,
            component_payload(ComponentType::TextSelectMenu, "Shared"),
        );
        assert!(!handlers.accepts(&menu));
        let menu = interaction(
            InteractionType::MessageComponent,
            component_payload(ComponentType::UserSelectMenu, "shared"),
        );
        assert!(!handlers.accepts(&menu));
        let button = interaction(
            InteractionType::MessageComponent,
            component_payload(ComponentType::Button, "shared"),
        );
        assert!(!handlers.accepts(&button));
        handlers.register("shared", click)?;
        assert!(handlers.accepts(&button));
        let modal = interaction(
            InteractionType::ModalSubmit,
            InteractionData::ModalSubmit(Box::new(ModalInteractionData {
                components: vec![],
                custom_id: "shared".into(),
                resolved: None,
            })),
        );
        assert!(handlers.accepts(&modal));
        let malformed = interaction(
            InteractionType::ModalSubmit,
            component_payload(ComponentType::TextSelectMenu, "shared"),
        );
        assert!(!handlers.accepts(&malformed));
        Ok(())
    }

    #[test]
    fn entity_selects_only_accept_the_registered_type_and_id() -> Result<()> {
        async fn user_selected(ctx: &mut UserSelectContext<'_>) -> Result<()> {
            let _: &[Id<UserMarker>] = ctx.values();
            Ok(())
        }
        async fn role_selected(ctx: &mut RoleSelectContext<'_>) -> Result<()> {
            let _: &[Id<RoleMarker>] = ctx.values();
            Ok(())
        }
        async fn channel_selected(ctx: &mut ChannelSelectContext<'_>) -> Result<()> {
            let _: &[Id<ChannelMarker>] = ctx.values();
            Ok(())
        }
        let mut handlers = Buttons::default();
        handlers.register_user_select("user", user_selected)?;
        handlers.register_role_select("role", role_selected)?;
        handlers.register_channel_select("channel", channel_selected)?;
        assert!(handlers.has_handlers());
        assert!(
            handlers
                .register_user_select("user", user_selected)
                .is_err()
        );
        assert!(
            handlers
                .register_role_select("role", role_selected)
                .is_err()
        );
        assert!(
            handlers
                .register_channel_select("channel", channel_selected)
                .is_err()
        );
        for (kind, id, expected) in [
            (ComponentType::UserSelectMenu, "user", true),
            (ComponentType::UserSelectMenu, "role", false),
            (ComponentType::RoleSelectMenu, "role", true),
            (ComponentType::RoleSelectMenu, "Role", false),
            (ComponentType::ChannelSelectMenu, "channel", true),
            (ComponentType::ChannelSelectMenu, "user", false),
            (ComponentType::TextSelectMenu, "channel", false),
            (ComponentType::MentionableSelectMenu, "user", false),
        ] {
            let incoming = interaction(
                InteractionType::MessageComponent,
                component_payload(kind, id),
            );
            assert_eq!(handlers.accepts(&incoming), expected);
        }
        Ok(())
    }

    #[test]
    fn accepts_canonical_unique_snowflakes_and_rejects_entire_invalid_lists() -> Result<()> {
        let values = vec!["123".into(), u64::MAX.to_string()];
        assert_eq!(
            parse_entity_ids::<UserMarker>(&values)?,
            vec![Id::new(123), Id::new(u64::MAX)]
        );
        assert!(parse_entity_ids::<RoleMarker>(&[])?.is_empty());
        for invalid in [
            "",
            "0",
            "01",
            "-1",
            "+1",
            " 1",
            "１",
            "18446744073709551616",
            "123",
        ] {
            let malformed = vec!["123".into(), invalid.into()];
            assert!(parse_entity_ids::<ChannelMarker>(&malformed).is_err());
        }
        let too_many = (1..=26).map(|id| id.to_string()).collect::<Vec<_>>();
        assert!(parse_entity_ids::<UserMarker>(&too_many).is_err());
        Ok(())
    }

    #[test]
    fn finds_text_inputs_in_label_and_legacy_row() {
        let field = |value: &str| {
            ModalInteractionComponent::TextInput(ModalInteractionTextInput {
                custom_id: "description".into(),
                id: 1,
                value: value.into(),
            })
        };
        let label = ModalInteractionComponent::Label(ModalInteractionLabel {
            component: Box::new(field("new")),
            id: 2,
        });
        let legacy = ModalInteractionComponent::ActionRow(ModalInteractionActionRow {
            components: vec![field("legacy")],
            id: 2,
        });
        assert_eq!(submitted_text(&label, "description"), Some("new"));
        assert_eq!(submitted_text(&legacy, "description"), Some("legacy"));
        assert_eq!(submitted_text(&label, "Description"), None);
    }

    #[test]
    fn invoker_comes_from_supported_message_metadata() {
        let mut metadata = InteractionMetadata {
            authorizing_integration_owners: ApplicationIntegrationMap {
                guild: None,
                user: None,
            },
            id: Id::new(1),
            interacted_message_id: None,
            kind: InteractionType::ApplicationCommand,
            original_response_message_id: None,
            target_message_id: None,
            target_user: None,
            triggering_interaction_metadata: None,
            user: user(42, false),
        };
        assert_eq!(message_invoker(&metadata), Some(Id::new(42)));
        metadata.kind = InteractionType::Ping;
        assert_eq!(message_invoker(&metadata), None);
    }

    #[test]
    fn modal_fallback_respects_outcome_and_visibility() {
        assert_eq!(
            modal_fallback(ResponseState::Pending, true),
            Some((MODAL_ACCEPTED, true))
        );
        assert_eq!(
            modal_fallback(ResponseState::Pending, false),
            Some((MODAL_FAILED, true))
        );
        for ephemeral in [false, true] {
            let deferred = ResponseState::Deferred { ephemeral };
            assert_eq!(
                modal_fallback(deferred, true),
                Some((MODAL_ACCEPTED, false))
            );
            assert_eq!(modal_fallback(deferred, false), Some((MODAL_FAILED, false)));
        }
        assert_eq!(modal_fallback(ResponseState::Replied, true), None);
        assert_eq!(modal_fallback(ResponseState::Replied, false), None);
    }

    #[test]
    fn invoker_from_reply_requires_a_matching_embedded_human_message() {
        let command = message(50, user(42, false));
        let mut response = message(60, user(99, true));
        response.kind = MessageType::Reply;
        response.reference = Some(MessageReference {
            kind: MessageReferenceType::Default,
            channel_id: Some(command.channel_id),
            guild_id: command.guild_id,
            message_id: Some(command.id),
            fail_if_not_exists: None,
        });
        assert_eq!(message_owner(&response), None);

        response.referenced_message = Some(Box::new(command));
        assert_eq!(message_owner(&response), Some(Id::new(42)));

        let reference = response.reference.as_mut().unwrap();
        reference.kind = MessageReferenceType::Forward;
        assert_eq!(message_owner(&response), None);

        let reference = response.reference.as_mut().unwrap();
        reference.kind = MessageReferenceType::Default;
        reference.message_id = Some(Id::new(90));
        assert_eq!(message_owner(&response), None);

        response.reference.as_mut().unwrap().message_id = Some(Id::new(50));
        response.referenced_message.as_mut().unwrap().author.bot = true;
        assert_eq!(message_owner(&response), None);
    }

    fn user(id: u64, bot: bool) -> User {
        User {
            accent_color: None,
            avatar: None,
            avatar_decoration: None,
            avatar_decoration_data: None,
            banner: None,
            bot,
            discriminator: 0,
            email: None,
            flags: None,
            global_name: None,
            id: Id::new(id),
            locale: None,
            mfa_enabled: None,
            name: "fixture".into(),
            premium_type: None,
            primary_guild: None,
            public_flags: None,
            system: None,
            verified: None,
        }
    }

    #[allow(deprecated)]
    fn message(id: u64, author: User) -> Message {
        Message {
            activity: None,
            application: None,
            application_id: None,
            attachments: vec![],
            author,
            call: None,
            channel_id: Id::new(3),
            components: vec![],
            content: String::new(),
            edited_timestamp: None,
            embeds: vec![],
            flags: None,
            guild_id: Some(Id::new(4)),
            id: Id::new(id),
            interaction: None,
            interaction_metadata: None,
            kind: MessageType::Regular,
            member: None,
            mention_channels: vec![],
            mention_everyone: false,
            mention_roles: vec![],
            mentions: vec![],
            message_snapshots: vec![],
            pinned: false,
            poll: None,
            reactions: vec![],
            reference: None,
            referenced_message: None,
            role_subscription_data: None,
            sticker_items: vec![],
            timestamp: Timestamp::from_micros(1_580_608_922_020_000).unwrap(),
            thread: None,
            tts: false,
            webhook_id: None,
        }
    }

    #[test]
    fn rejects_duplicate_and_invalid_button_ids() -> Result<()> {
        let mut buttons = Buttons::default();
        assert!(!buttons.has_handlers());
        assert!(buttons.register("", click).is_err());
        assert!(buttons.register(&"a".repeat(101), click).is_err());
        buttons.register("akari:ui:ping", click)?;
        assert!(buttons.has_handlers());
        assert!(buttons.register("akari:ui:ping", click).is_err());
        Ok(())
    }

    #[test]
    fn selects_an_update_path_only_before_reply_or_after_deferring() -> Result<()> {
        assert_eq!(
            ButtonResponse::Pending.update_action()?,
            UpdateAction::Initial
        );
        assert_eq!(
            ButtonResponse::DeferredUpdate.update_action()?,
            UpdateAction::Edit
        );
        assert!(ButtonResponse::DeferredUpdate.require_pending().is_err());
        assert!(ButtonResponse::Sent.require_pending().is_err());
        assert!(ButtonResponse::Sent.update_action().is_err());
        Ok(())
    }

    #[test]
    fn update_payload_replaces_all_three_fields_and_suppresses_mentions() {
        let message = UiMessage::new().embed(Embed::new("新しいタイトル"));
        let response = update_response(&message);
        assert_eq!(response.kind, InteractionResponseType::UpdateMessage);
        let data = response.data.unwrap();
        assert_eq!(data.content.as_deref(), Some(""));
        assert_eq!(data.embeds.unwrap().len(), 1);
        assert!(data.components.unwrap().is_empty());
        let mentions = data.allowed_mentions.unwrap();
        assert!(mentions.parse.is_empty());
        assert!(!mentions.replied_user);

        let buttons_only = UiMessage::new().buttons(vec![Button::primary("refresh", "更新")]);
        let data = update_response(&buttons_only).data.unwrap();
        assert!(data.embeds.unwrap().is_empty());
        assert_eq!(data.components.unwrap().len(), 1);
    }
}
