use std::{future::Future, pin::Pin};

use twilight_http::Client;
use twilight_model::{
    application::{
        interaction::{
            Interaction, InteractionData, InteractionDataResolved, InteractionType,
            message_component::MessageComponentInteractionData,
            modal::{ModalInteractionComponent, ModalInteractionData},
        },
        monetization::Entitlement,
    },
    channel::{
        Attachment, Message,
        message::{AllowedMentions, MessageFlags, component::ComponentType},
    },
    http::interaction::{InteractionResponse, InteractionResponseData, InteractionResponseType},
    id::{
        Id,
        marker::{
            AttachmentMarker, ChannelMarker, GenericMarker, GuildMarker, RoleMarker, UserMarker,
        },
    },
};

mod followup;
mod modal;
mod original;
#[cfg(test)]
mod original_tests;
mod owner;
mod routes;
pub(crate) use modal::{RadioSubmission, capture_modal_radios};
use routes::Routes;

use crate::{
    Error, ErrorSource, HandlerKind, Messages, ResponseStateError, Result, SelectionError, Timing,
    hooks::Hooks,
    slash::{
        InitialResponder, ResponseState, callback_unavailable, edit_ui_response, edit_v2_response,
        send_followup_message,
    },
    ui::{ComponentsV2, DynamicIdRef, Modal, UiMessage},
};

#[derive(Default)]
pub(crate) struct InteractionHandlers {
    buttons: Routes<dyn ButtonHandler>,
    selects: Routes<dyn SelectHandler>,
    user_selects: Routes<dyn EntitySelectHandler<UserMarker>>,
    role_selects: Routes<dyn EntitySelectHandler<RoleMarker>>,
    channel_selects: Routes<dyn EntitySelectHandler<ChannelMarker>>,
    mentionable_selects: Routes<dyn MentionableSelectHandler>,
    modals: Routes<dyn ModalHandler>,
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
    pub(crate) fn has_modals(&self) -> bool {
        !self.modals.is_empty()
    }

    pub(crate) fn register_button(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut ButtonContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        self.buttons
            .register_exact(HandlerKind::Button, custom_id, Box::new(handler))
    }

    pub(crate) fn register_select(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut SelectContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        self.selects
            .register_exact(HandlerKind::Select, custom_id, Box::new(handler))
    }

    pub(crate) fn register_user_select(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut UserSelectContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        self.user_selects
            .register_exact(HandlerKind::UserSelect, custom_id, Box::new(handler))
    }

    pub(crate) fn register_role_select(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut RoleSelectContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        self.role_selects
            .register_exact(HandlerKind::RoleSelect, custom_id, Box::new(handler))
    }

    pub(crate) fn register_channel_select(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut ChannelSelectContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        self.channel_selects.register_exact(
            HandlerKind::ChannelSelect,
            custom_id,
            Box::new(handler),
        )
    }

    pub(crate) fn register_mentionable_select(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut MentionableSelectContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        self.mentionable_selects.register_exact(
            HandlerKind::MentionableSelect,
            custom_id,
            Box::new(handler),
        )
    }

    pub(crate) fn register_modal(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut ModalContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        self.modals
            .register_exact(HandlerKind::Modal, custom_id, Box::new(handler))
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
            Some(UiInput::Button(id)) => self.buttons.resolve(id).is_some(),
            Some(UiInput::Select(data)) => self.selects.resolve(&data.custom_id).is_some(),
            Some(UiInput::UserSelect(data)) => self.user_selects.resolve(&data.custom_id).is_some(),
            Some(UiInput::RoleSelect(data)) => self.role_selects.resolve(&data.custom_id).is_some(),
            Some(UiInput::ChannelSelect(data)) => {
                self.channel_selects.resolve(&data.custom_id).is_some()
            }
            Some(UiInput::MentionableSelect(data)) => {
                self.mentionable_selects.resolve(&data.custom_id).is_some()
            }
            Some(UiInput::Modal(data)) => self.modals.resolve(&data.custom_id).is_some(),
            None => false,
        }
    }

    pub(crate) async fn dispatch(
        &self,
        http: &Client,
        interaction: &Interaction,
        timing: Timing,
        hooks: &Hooks,
        radio: &[RadioSubmission],
    ) {
        let initial = InitialResponder {
            http,
            interaction,
            timing,
            info: &hooks.info,
            permissions: &hooks.permissions,
        };
        match ui_input(interaction) {
            Some(UiInput::Button(id)) => {
                let Some((handler, dynamic_id)) = self.buttons.resolve(id) else {
                    return;
                };
                let mut ctx = ButtonContext::new(initial, id, dynamic_id);
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
                let Some((handler, dynamic_id)) = self.selects.resolve(&data.custom_id) else {
                    return;
                };
                let mut ctx = SelectContext {
                    inner: ButtonContext::new(initial, &data.custom_id, dynamic_id),
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
                dispatch_entity(&self.user_selects, initial, data, hooks).await;
            }
            Some(UiInput::RoleSelect(data)) => {
                dispatch_entity(&self.role_selects, initial, data, hooks).await;
            }
            Some(UiInput::ChannelSelect(data)) => {
                dispatch_entity(&self.channel_selects, initial, data, hooks).await;
            }
            Some(UiInput::MentionableSelect(data)) => {
                let Some((handler, dynamic_id)) = self.mentionable_selects.resolve(&data.custom_id)
                else {
                    return;
                };
                let mut inner = ButtonContext::new(initial, &data.custom_id, dynamic_id);
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
                let Some((handler, dynamic_id)) = self.modals.resolve(&data.custom_id) else {
                    return;
                };
                let mut ctx = ModalContext {
                    initial,
                    data,
                    dynamic_id,
                    response: ComponentResponse::Pending,
                    radio,
                };
                let result = handler.call(&mut ctx).await;
                finish_modal(&mut ctx, result, hooks).await;
            }
            None => {}
        }
    }
}

async fn dispatch_entity<M>(
    handlers: &Routes<dyn EntitySelectHandler<M>>,
    initial: InitialResponder<'_>,
    data: &MessageComponentInteractionData,
    hooks: &Hooks,
) {
    let Some((handler, dynamic_id)) = handlers.resolve(&data.custom_id) else {
        return;
    };
    let mut inner = ButtonContext::new(initial, &data.custom_id, dynamic_id);
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
        &Error::from(SelectionError::Unreadable),
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
    collect_selection(values, |value| {
        if value.starts_with('0') || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(SelectionError::NotAnId.into());
        }
        resolve(value.parse().map_err(|_| SelectionError::NotAnId)?)
    })
}

fn collect_selection<T: Eq, V>(values: &[V], resolve: impl Fn(&V) -> Result<T>) -> Result<Vec<T>> {
    if values.len() > 25 {
        return Err(SelectionError::TooMany.into());
    }
    let mut selections = Vec::with_capacity(values.len());
    for value in values {
        let selection = resolve(value)?;
        if selections.contains(&selection) {
            return Err(SelectionError::Duplicate.into());
        }
        selections.push(selection);
    }
    Ok(selections)
}

fn parse_mentionables(
    values: &[String],
    resolved: Option<&InteractionDataResolved>,
) -> Result<Vec<Mentionable>> {
    parse_select_values(values, |id| resolve_mentionable(id, resolved))
}

pub(crate) fn resolve_mentionable(
    id: Id<GenericMarker>,
    resolved: Option<&InteractionDataResolved>,
) -> Result<Mentionable> {
    let resolved = resolved.ok_or(SelectionError::MissingResolved)?;
    let user_id = id.cast();
    let role_id = id.cast();
    match (resolved.users.get(&user_id), resolved.roles.get(&role_id)) {
        (Some(user), None) if user.id == user_id => Ok(Mentionable::User(user_id)),
        (None, Some(role)) if role.id == role_id => Ok(Mentionable::Role(role_id)),
        _ => Err(SelectionError::AmbiguousMention.into()),
    }
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
        ResponseState::Replied
        | ResponseState::RepliedV2
        | ResponseState::OriginalDeleted { .. }
        | ResponseState::ModalOpened => None,
    }
}

async fn finish_modal(ctx: &mut ModalContext<'_>, result: Result<()>, hooks: &Hooks) {
    let name = Some(ctx.custom_id());
    if let Err(error) = &result {
        hooks.reporter.report(ErrorSource::Modal, name, error);
        if ctx.response == ComponentResponse::Pending && callback_unavailable(error) {
            return;
        }
    }
    let fallback = if ctx.response == ComponentResponse::DeferredUpdate {
        if result.is_ok() {
            return;
        }
        let Some(content) = hooks.messages.modal_failed.as_deref() else {
            return;
        };
        ctx.followup_ephemeral(content).await
    } else {
        let Ok(state) = ctx.response.channel_response() else {
            return;
        };
        let Some((content, ephemeral)) = modal_fallback(state, result.is_ok(), &hooks.messages)
        else {
            return;
        };
        ctx.respond(content, ephemeral).await
    };
    if let Err(error) = fallback {
        hooks.reporter.report(ErrorSource::Response, name, &error);
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
            if ctx.response == ComponentResponse::Pending && callback_unavailable(&error) {
                return;
            }
            let Some(text) = failure else {
                return;
            };
            let fallback = match ctx.response {
                ComponentResponse::Pending => ctx.reply_ephemeral(text).await,
                ComponentResponse::DeferredUpdate => ctx.followup_ephemeral(text).await,
                ComponentResponse::Sent | ComponentResponse::OwnResponse(_) => Ok(()),
            };
            if let Err(error) = fallback {
                hooks.reporter.report(ErrorSource::Response, name, &error);
            }
        }
        Ok(()) if ctx.response == ComponentResponse::Pending => {
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
    initial: InitialResponder<'a>,
    custom_id: &'a str,
    dynamic_id: Option<DynamicIdRef<'a>>,
    response: ComponentResponse,
}

impl std::fmt::Debug for ButtonContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ButtonContext")
            .field("interaction_id", &self.initial.interaction.id)
            .field("timing", &self.initial.timing)
            .field("custom_id", &self.custom_id)
            .field("response", &self.response)
            .finish_non_exhaustive()
    }
}

impl<'a> ButtonContext<'a> {
    fn new(
        initial: InitialResponder<'a>,
        custom_id: &'a str,
        dynamic_id: Option<DynamicIdRef<'a>>,
    ) -> Self {
        Self {
            initial,
            custom_id,
            dynamic_id,
            response: ComponentResponse::Pending,
        }
    }

    /// Akari が使用している Twilight の REST クライアントを借ります。
    pub fn http(&self) -> &'a Client {
        self.initial.http
    }

    /// ボタン作成時に設定した ID を返します。
    pub fn custom_id(&self) -> &'a str {
        self.custom_id
    }

    /// このボタンを押した Interaction の ID です。
    pub fn interaction_id(&self) -> Id<twilight_model::id::marker::InteractionMarker> {
        self.initial.interaction.id
    }

    /// 動的な名前空間で登録したハンドラーに、ID の名前空間と値を返します。
    pub fn dynamic_id(&self) -> Option<DynamicIdRef<'a>> {
        self.dynamic_id
    }

    /// ボタンを押したユーザーの ID を返します。
    pub fn author_id(&self) -> Option<Id<UserMarker>> {
        self.initial.interaction.author_id()
    }

    /// Discord が今回のボタン操作の Interaction に付けた購入権利を借ります。
    /// 対象 SKU、利用者またはサーバー、消費や有効期間の扱いはアプリ側で確認します。
    pub fn entitlements(&self) -> &'a [Entitlement] {
        &self.initial.interaction.entitlements
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
        owner::resolve_owner(self.initial.http, message).await
    }

    /// ボタンが押されたサーバーの ID を返します。DM では `None` です。
    pub fn guild_id(&self) -> Option<Id<GuildMarker>> {
        self.initial.interaction.guild_id
    }

    /// ボタンが付いた Discord メッセージを借ります。
    pub fn message(&self) -> Option<&'a Message> {
        self.initial.interaction.message.as_ref()
    }

    /// Gateway の往復時間と、ボタン操作を処理するまでの時間を返します。
    pub fn timing(&self) -> Timing {
        self.initial.timing
    }

    /// ボタンを押した本人にだけ表示する初回応答を送ります。
    ///
    /// # エラー
    ///
    /// 応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn reply_ephemeral(&mut self, content: &str) -> Result<()> {
        self.response.require_pending()?;
        self.initial.message(content, true).await?;
        self.response = ComponentResponse::OwnResponse(ResponseState::Replied);
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
        self.response.defer_update(self.initial).await
    }

    /// ボタンが付いたメッセージの本文、Embed、コンポーネントを置き換えます。
    /// 省略した本文や Embed、コンポーネントは取り除かれます。`defer_update` 後にも一度呼べます。
    ///
    /// # エラー
    /// 更新できない応答状態、無効なメッセージ、または Discord が受け付けない場合に失敗します。
    pub async fn update_message(&mut self, message: &UiMessage) -> Result<()> {
        self.response.update_message(self.initial, message).await
    }

    /// 元の Components V2 メッセージのコンポーネントを置き換え、指定した添付を追加します。
    /// 既存の添付ファイルは保持します。
    /// `defer_update` 後にも一度呼べます。
    /// 遅延更新で添付を追加する場合は、更新前に Discord から添付一覧を取得します。
    ///
    /// # エラー
    /// 更新できない応答状態、元メッセージが Components V2 以外、無効なコンポーネント、または通信の失敗でエラーになります。
    pub async fn update_v2(&mut self, message: &ComponentsV2) -> Result<()> {
        self.response.update_v2(self.initial, message).await
    }

    /// 初回応答としてモーダルを開きます。
    ///
    /// # エラー
    /// 初回応答済み、無効なモーダル、または Discord が受け付けなかった場合に失敗します。
    pub async fn show_modal(&mut self, modal: &Modal) -> Result<()> {
        self.response.require_pending()?;
        self.initial.modal(modal).await?;
        self.response = ComponentResponse::Sent;
        Ok(())
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
        self.response
            .send_followup(self.initial, content, ephemeral)
            .await
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

    /// 動的な名前空間で登録したハンドラーに、ID の名前空間と値を返します。
    pub fn dynamic_id(&self) -> Option<DynamicIdRef<'a>> {
        self.inner.dynamic_id()
    }

    /// ユーザーが選択した値を借ります。何も選ばれていなければ空です。
    pub fn values(&self) -> &'a [String] {
        self.values
    }

    /// セレクトを操作したユーザーの ID を返します。
    pub fn author_id(&self) -> Option<Id<UserMarker>> {
        self.inner.author_id()
    }

    /// Discord が今回のセレクト操作の Interaction に付けた購入権利を借ります。
    /// 確認方法は [`ButtonContext::entitlements`] を参照してください。
    pub fn entitlements(&self) -> &'a [Entitlement] {
        self.inner.entitlements()
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

/// セレクトメニューやコマンドの引数で指定できるユーザーまたはロールです。
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

        /// 動的な名前空間で登録したハンドラーに、ID の名前空間と値を返します。
        pub fn dynamic_id(&self) -> Option<DynamicIdRef<$lifetime>> {
            self.inner.dynamic_id()
        }

        /// Discord が解決済みの情報を送った場合に、その情報を借ります。
        pub fn resolved(&self) -> Option<&$lifetime InteractionDataResolved> {
            self.resolved
        }

        /// セレクトを操作したユーザーの ID を返します。
        pub fn author_id(&self) -> Option<Id<UserMarker>> {
            self.inner.author_id()
        }

        /// Discord が今回のセレクト操作の Interaction に付けた購入権利を借ります。
        /// 確認方法は [`ButtonContext::entitlements`] を参照してください。
        pub fn entitlements(&self) -> &$lifetime [Entitlement] {
            self.inner.entitlements()
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
    initial: InitialResponder<'a>,
    data: &'a ModalInteractionData,
    dynamic_id: Option<DynamicIdRef<'a>>,
    response: ComponentResponse,
    radio: &'a [RadioSubmission],
}

impl std::fmt::Debug for ModalContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModalContext")
            .field("interaction_id", &self.initial.interaction.id)
            .field("timing", &self.initial.timing)
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

    /// 動的な名前空間で登録したハンドラーに、ID の名前空間と値を返します。
    pub fn dynamic_id(&self) -> Option<DynamicIdRef<'a>> {
        self.dynamic_id
    }

    /// 指定した入力欄に送信されたテキストを借ります。該当の入力欄がない場合は `None` です。
    pub fn text_input(&self, custom_id: &str) -> Option<&'a str> {
        self.data
            .components
            .iter()
            .find_map(|input| submitted_text(input, custom_id))
    }

    /// 指定した添付欄のファイル ID を Discord から届いた順に借ります。
    ///
    /// 添付欄がない場合は `None`、任意入力の欄に添付がない場合は空のスライスです。
    pub fn file_upload(&self, custom_id: &str) -> Option<&'a [Id<AttachmentMarker>]> {
        self.data.components.iter().find_map(|input| {
            submitted_field(input, |field| match field {
                ModalInteractionComponent::FileUpload(upload) if upload.custom_id == custom_id => {
                    Some(upload.values.as_slice())
                }
                _ => None,
            })
        })
    }

    /// モーダル送信時に Discord が解決した添付の情報を借ります。
    ///
    /// 解決データに ID がなければ `None` です。ファイルのダウンロードは行いません。
    pub fn attachment(&self, id: Id<AttachmentMarker>) -> Option<&'a Attachment> {
        self.data
            .resolved
            .as_ref()?
            .attachments
            .get(&id)
            .filter(|attachment| attachment.id == id)
    }

    /// 指定したチェックボックスの状態を返します。オフの場合は `Some(false)` です。
    ///
    /// 指定した ID のチェックボックスがない場合は `None` です。
    pub fn checkbox(&self, custom_id: &str) -> Option<bool> {
        self.data.components.iter().find_map(|input| {
            submitted_field(input, |field| match field {
                ModalInteractionComponent::Checkbox(checkbox)
                    if checkbox.custom_id == custom_id =>
                {
                    Some(checkbox.value)
                }
                _ => None,
            })
        })
    }

    /// モーダルを送信したユーザーの ID を返します。
    pub fn author_id(&self) -> Option<Id<UserMarker>> {
        self.initial.interaction.author_id()
    }

    /// Discord が今回のモーダル送信の Interaction に付けた購入権利を借ります。
    /// 対象 SKU、利用者またはサーバー、消費や有効期間の扱いはアプリ側で確認します。
    pub fn entitlements(&self) -> &'a [Entitlement] {
        &self.initial.interaction.entitlements
    }

    /// モーダルが送信されたサーバーの ID を返します。DM では `None` です。
    pub fn guild_id(&self) -> Option<Id<GuildMarker>> {
        self.initial.interaction.guild_id
    }

    /// Gateway の往復時間と、モーダル送信を処理するまでの時間を返します。
    pub fn timing(&self) -> Timing {
        self.initial.timing
    }

    /// Akari が使用している Twilight の REST クライアントを借ります。
    pub fn http(&self) -> &'a Client {
        self.initial.http
    }

    /// チャンネルに初回応答を送るか、遅延応答を元の公開範囲のまま完成させます。
    ///
    /// # エラー
    /// 応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn reply(&mut self, content: &str) -> Result<()> {
        self.respond(content, false).await
    }

    /// モーダルを送信した本人にだけ応答するか、非公開の遅延応答を完成させます。
    ///
    /// # エラー
    /// 応答済み、公開の遅延応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn reply_ephemeral(&mut self, content: &str) -> Result<()> {
        self.respond(content, true).await
    }

    /// チャンネルに処理中の表示を出します。作業が終わったら [`Self::reply`] で完成させます。
    ///
    /// # エラー
    /// 初回応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn defer(&mut self) -> Result<()> {
        self.defer_response(false).await
    }

    /// 送信した本人にだけ処理中の表示を出します。
    ///
    /// # エラー
    /// 初回応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn defer_ephemeral(&mut self) -> Result<()> {
        self.defer_response(true).await
    }

    /// 初回メッセージを完成させるか、元のメッセージの更新を受け付けた後に、チャンネルへ追加メッセージを送ります。
    ///
    /// # エラー
    /// 初回メッセージが未完成、または Discord が受け付けなかった場合に失敗します。
    pub async fn followup(&self, content: &str) -> Result<()> {
        self.send_followup(content, false).await
    }

    /// 初回メッセージを完成させるか、元のメッセージの更新を受け付けた後に、本人にだけ追加メッセージを送ります。
    ///
    /// # エラー
    /// 初回メッセージが未完成、または Discord が受け付けなかった場合に失敗します。
    pub async fn followup_ephemeral(&self, content: &str) -> Result<()> {
        self.send_followup(content, true).await
    }

    /// フォームの送信に公開の投票で応答します。公開の `defer` 後にも使えます。
    ///
    /// # エラー
    /// 非公開の遅延応答、元メッセージの更新を予約した場合、応答済み、
    /// または Discord が受け付けない場合に失敗します。
    pub async fn reply_poll(&mut self, poll: &crate::PollDraft<'_>) -> Result<()> {
        let mut state = self.response.channel_response()?;
        state.reply_poll(self.initial, poll).await?;
        self.response = ComponentResponse::OwnResponse(state);
        Ok(())
    }

    async fn respond(&mut self, content: &str, ephemeral: bool) -> Result<()> {
        let mut state = self.response.channel_response()?;
        state.reply(self.initial, content, ephemeral).await?;
        self.response = ComponentResponse::OwnResponse(state);
        Ok(())
    }

    async fn defer_response(&mut self, ephemeral: bool) -> Result<()> {
        let mut state = self.response.channel_response()?;
        state.defer(self.initial, ephemeral).await?;
        self.response = ComponentResponse::OwnResponse(state);
        Ok(())
    }

    async fn send_followup(&self, content: &str, ephemeral: bool) -> Result<()> {
        self.response
            .send_followup(self.initial, content, ephemeral)
            .await
    }
}

fn submitted_text<'a>(
    component: &'a ModalInteractionComponent,
    custom_id: &str,
) -> Option<&'a str> {
    submitted_field(component, |field| match field {
        ModalInteractionComponent::TextInput(input) if input.custom_id == custom_id => {
            Some(input.value.as_str())
        }
        _ => None,
    })
}

fn submitted_field<'a, T>(
    component: &'a ModalInteractionComponent,
    get: impl Fn(&'a ModalInteractionComponent) -> Option<T> + Copy,
) -> Option<T> {
    match component {
        ModalInteractionComponent::ActionRow(row) => row
            .components
            .iter()
            .find_map(|child| submitted_field(child, get)),
        ModalInteractionComponent::Label(label) => submitted_field(&label.component, get),
        field => get(field),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComponentResponse {
    Pending,
    DeferredUpdate,
    Sent,
    OwnResponse(ResponseState),
}

impl ComponentResponse {
    fn channel_response(self) -> Result<ResponseState> {
        match self {
            Self::Pending => Ok(ResponseState::Pending),
            Self::OwnResponse(state) => Ok(state),
            Self::DeferredUpdate | Self::Sent => Err(ResponseStateError::AlreadyResponded.into()),
        }
    }

    fn require_followup(self) -> Result<()> {
        match self {
            Self::Pending => Err(ResponseStateError::FollowupBeforeResponse.into()),
            Self::DeferredUpdate | Self::Sent => Ok(()),
            Self::OwnResponse(state) => state.require_followup(),
        }
    }

    async fn send_followup(
        self,
        initial: InitialResponder<'_>,
        content: &str,
        ephemeral: bool,
    ) -> Result<()> {
        self.require_followup()?;
        send_followup_message(initial.http, initial.interaction, content, ephemeral).await
    }

    fn require_pending(self) -> Result<()> {
        if self == Self::Pending {
            Ok(())
        } else {
            Err(ResponseStateError::AlreadyResponded.into())
        }
    }

    fn update_action(self) -> Result<UpdateAction> {
        match self {
            Self::Pending => Ok(UpdateAction::Initial),
            Self::DeferredUpdate => Ok(UpdateAction::Edit),
            Self::Sent | Self::OwnResponse(_) => Err(ResponseStateError::CannotUpdate.into()),
        }
    }

    async fn defer_update(&mut self, initial: InitialResponder<'_>) -> Result<()> {
        self.require_pending()?;
        initial
            .send(&InteractionResponse {
                kind: InteractionResponseType::DeferredUpdateMessage,
                data: None,
            })
            .await?;
        *self = Self::DeferredUpdate;
        Ok(())
    }

    async fn update_message(
        &mut self,
        initial: InitialResponder<'_>,
        message: &UiMessage,
    ) -> Result<()> {
        let action = self.update_action()?;
        if uses_components_v2(initial.interaction.message.as_ref()) {
            return Err(ResponseStateError::UseUpdateV2.into());
        }
        message.validate()?;
        match action {
            UpdateAction::Initial => initial.send(&update_response(message)).await?,
            UpdateAction::Edit => {
                edit_ui_response(initial.http, initial.interaction, message).await?
            }
        }
        *self = Self::Sent;
        Ok(())
    }

    async fn update_v2(
        &mut self,
        initial: InitialResponder<'_>,
        message: &ComponentsV2,
    ) -> Result<()> {
        let action = self.update_action()?;
        let source = initial.interaction.message.as_ref();
        if !uses_components_v2(source) {
            return Err(ResponseStateError::NeedsV2Message.into());
        }
        message.validate()?;
        match action {
            UpdateAction::Initial => initial.v2_update(message).await?,
            UpdateAction::Edit => {
                let current;
                let retained = if message.needs_existing_attachments() {
                    message.validate_attachment_sizes(initial.interaction.attachment_size_limit)?;
                    current = initial.fetch_original_response().await?;
                    Some(current.attachments.as_slice())
                } else {
                    source.map(|source| source.attachments.as_slice())
                };
                edit_v2_response(initial.http, initial.interaction, message, retained).await?
            }
        }
        *self = Self::Sent;
        Ok(())
    }
}

fn uses_components_v2(message: Option<&Message>) -> bool {
    message
        .and_then(|message| message.flags)
        .is_some_and(|flags| flags.contains(MessageFlags::IS_COMPONENTS_V2))
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
