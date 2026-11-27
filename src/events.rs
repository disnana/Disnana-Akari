use std::{future::Future, pin::Pin, sync::Arc};

use twilight_gateway::{Event, EventTypeFlags, Intents};
use twilight_model::channel::Message;

pub use twilight_model::gateway::payload::incoming::{MessageDelete, MessageDeleteBulk, Ready};

mod activity;
mod cached_members;
mod cached_messages;
mod dynamic_interactions;
mod gateway;
mod reactions;
pub(crate) use activity::ActivityEvent;
use activity::ActivityHandlers;
pub use activity::{MessagePollVoteAdd, MessagePollVoteRemove, TypingStart};
use cached_members::CachedMemberHandlers;
use cached_messages::CachedMessageHandlers;
use gateway::GatewayHandlers;
pub use gateway::GatewayPayload;
pub(crate) use reactions::ReactionEvent;
use reactions::ReactionHandlers;
pub use reactions::{ReactionAdd, ReactionRemove, ReactionRemoveAll, ReactionRemoveEmoji};

use crate::{
    ButtonContext, CachedMessageEdit, ChannelSelectContext, ErrorSource, MentionableSelectContext,
    MessageUpdate, ModalContext, Result, RoleSelectContext, SelectContext, UserSelectContext,
    error::adapt_handler, hooks::Reporter, interactions::InteractionHandlers,
};

/// Gateway イベントごとに非同期ハンドラーを登録します。
#[derive(Default)]
pub struct Events {
    ready: Vec<Box<dyn Handler<Ready>>>,
    message: Vec<Box<dyn Handler<Message>>>,
    message_update: Vec<Box<dyn Handler<MessageUpdate>>>,
    message_delete: Vec<Box<dyn Handler<MessageDelete>>>,
    message_delete_bulk: Vec<Box<dyn Handler<MessageDeleteBulk>>>,
    cached_messages: CachedMessageHandlers,
    cached_members: CachedMemberHandlers,
    reactions: ReactionHandlers,
    activity: ActivityHandlers,
    gateway: GatewayHandlers,
    pub(crate) interaction_handlers: InteractionHandlers,
}

impl std::fmt::Debug for Events {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Events")
            .field("ready_handlers", &self.ready.len())
            .field("message_handlers", &self.message.len())
            .field("message_update_handlers", &self.message_update.len())
            .field("message_delete_handlers", &self.message_delete.len())
            .field(
                "message_delete_bulk_handlers",
                &self.message_delete_bulk.len(),
            )
            .field("reaction_handlers", &self.reactions)
            .field("activity_handlers", &self.activity)
            .field("gateway_handlers", &self.gateway)
            .field("cached_message_types", &self.cached_message_types())
            .field("interaction_handlers", &self.interaction_handlers)
            .finish()
    }
}

impl Events {
    /// 空のイベントハンドラー一覧を作ります。
    pub fn new() -> Self {
        Self::default()
    }

    /// 新しい Gateway セッションが準備できたときに呼ばれる関数を追加します。
    pub fn on_ready<E>(
        &mut self,
        handler: impl AsyncFn(&Ready) -> std::result::Result<(), E> + Send + Sync + 'static,
    ) where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.ready
            .push(Box::new(adapt_handler!(handler, event: &Ready)));
    }

    /// サーバーで人が送信した新着メッセージを受け取る関数を追加します。
    pub fn on_message<E>(
        &mut self,
        handler: impl AsyncFn(&Message) -> std::result::Result<(), E> + Send + Sync + 'static,
    ) where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.message
            .push(Box::new(adapt_handler!(handler, event: &Message)));
    }

    /// Discord から届いたメッセージの部分更新を受け取る関数を追加します。
    ///
    /// 編集前の情報はなく、本文が変わらない更新でも呼ばれます。
    pub fn on_raw_message_edit<E>(
        &mut self,
        handler: impl AsyncFn(&MessageUpdate) -> std::result::Result<(), E> + Send + Sync + 'static,
    ) where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.message_update
            .push(Box::new(adapt_handler!(handler, event: &MessageUpdate)));
    }

    /// Discord から届いた単独削除の ID を受け取る関数を追加します。
    pub fn on_raw_message_delete<E>(
        &mut self,
        handler: impl AsyncFn(&MessageDelete) -> std::result::Result<(), E> + Send + Sync + 'static,
    ) where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.message_delete
            .push(Box::new(adapt_handler!(handler, event: &MessageDelete)));
    }

    /// サーバー内の一括削除の ID 一覧を受け取る関数を追加します。
    pub fn on_raw_bulk_message_delete<E>(
        &mut self,
        handler: impl AsyncFn(&MessageDeleteBulk) -> std::result::Result<(), E> + Send + Sync + 'static,
    ) where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.message_delete_bulk
            .push(Box::new(adapt_handler!(handler, event: &MessageDeleteBulk)));
    }

    /// 指定したカスタム ID のボタンを押したときに呼ぶ関数を登録します。
    ///
    /// # エラー
    ///
    /// ID の長さが 1 文字未満か 100 文字超、または登録済みの場合に失敗します。
    pub fn on_button<E>(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut ButtonContext<'_>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.interaction_handlers.register_button(
            custom_id,
            adapt_handler!(handler, ctx: &mut ButtonContext<'_>),
        )
    }

    /// 指定した ID の文字列 Select Menu が選択されたときに呼ぶ関数を登録します。
    ///
    /// # エラー
    /// ID が無効、または同じ Select Menu ID を登録済みの場合に失敗します。
    pub fn on_select<E>(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut SelectContext<'_>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.interaction_handlers.register_select(
            custom_id,
            adapt_handler!(handler, ctx: &mut SelectContext<'_>),
        )
    }

    /// 指定した ID のユーザー選択を処理する関数を登録します。
    ///
    /// # エラー
    /// ID が無効、または同じユーザー Select ID を登録済みの場合に失敗します。
    pub fn on_user_select<E>(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut UserSelectContext<'_>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.interaction_handlers.register_user_select(
            custom_id,
            adapt_handler!(handler, ctx: &mut UserSelectContext<'_>),
        )
    }

    /// 指定した ID のロール選択を処理する関数を登録します。
    ///
    /// # エラー
    /// ID が無効、または同じロール Select ID を登録済みの場合に失敗します。
    pub fn on_role_select<E>(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut RoleSelectContext<'_>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.interaction_handlers.register_role_select(
            custom_id,
            adapt_handler!(handler, ctx: &mut RoleSelectContext<'_>),
        )
    }

    /// 指定した ID のチャンネル選択を処理する関数を登録します。
    ///
    /// # エラー
    /// ID が無効、または同じチャンネル Select ID を登録済みの場合に失敗します。
    pub fn on_channel_select<E>(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut ChannelSelectContext<'_>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.interaction_handlers.register_channel_select(
            custom_id,
            adapt_handler!(handler, ctx: &mut ChannelSelectContext<'_>),
        )
    }

    /// 指定した ID のメニューでユーザーやロールを選んだときに呼ぶ関数を登録します。
    ///
    /// # エラー
    /// ID が無効、または同じ Mentionable Select ID を登録済みの場合に失敗します。
    pub fn on_mentionable_select<E>(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut MentionableSelectContext<'_>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.interaction_handlers.register_mentionable_select(
            custom_id,
            adapt_handler!(handler, ctx: &mut MentionableSelectContext<'_>),
        )
    }

    /// 指定した ID の Modal が送信されたときに呼ぶ関数を登録します。
    ///
    /// # エラー
    /// ID が無効、または同じ Modal ID を登録済みの場合に失敗します。
    pub fn on_modal<E>(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut ModalContext<'_>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.interaction_handlers.register_modal(
            custom_id,
            adapt_handler!(handler, ctx: &mut ModalContext<'_>),
        )
    }

    pub(crate) fn has_message_handlers(&self) -> bool {
        !self.message.is_empty()
    }

    pub(crate) fn message_lifecycle_types(&self) -> EventTypeFlags {
        let mut types = EventTypeFlags::empty();
        types.set(
            EventTypeFlags::MESSAGE_UPDATE,
            !self.message_update.is_empty(),
        );
        types.set(
            EventTypeFlags::MESSAGE_DELETE,
            !self.message_delete.is_empty(),
        );
        types.set(
            EventTypeFlags::MESSAGE_DELETE_BULK,
            !self.message_delete_bulk.is_empty(),
        );
        types
    }

    pub(crate) fn cached_message_types(&self) -> EventTypeFlags {
        self.cached_messages.types()
    }

    pub(crate) async fn cached_message_edit(&self, event: &CachedMessageEdit, reporter: &Reporter) {
        self.cached_messages.edit(event, reporter).await;
    }

    pub(crate) async fn cached_message_delete(&self, event: &Message, reporter: &Reporter) {
        self.cached_messages.delete(event, reporter).await;
    }

    pub(crate) async fn cached_bulk_message_delete(
        &self,
        event: &[Arc<Message>],
        reporter: &Reporter,
    ) {
        self.cached_messages.bulk_delete(event, reporter).await;
    }

    pub(crate) fn reaction_types(&self) -> EventTypeFlags {
        self.reactions.types()
    }

    pub(crate) async fn reaction(&self, event: &ReactionEvent, reporter: &Reporter) {
        self.reactions.dispatch(event, reporter).await;
    }

    pub(crate) fn activity_types(&self) -> EventTypeFlags {
        self.activity.types()
    }

    pub(crate) async fn activity(&self, event: &ActivityEvent, reporter: &Reporter) {
        self.activity.dispatch(event, reporter).await;
    }

    pub(crate) fn gateway_types(&self, intents: Intents) -> Result<EventTypeFlags> {
        self.gateway.types(intents)
    }

    pub(crate) fn gateway_event_name(&self, event: &Event) -> Option<&'static str> {
        self.gateway.event_name(event)
    }

    pub(crate) async fn gateway(&self, event: &Event, reporter: &Reporter) {
        self.gateway.dispatch(event, reporter).await;
    }

    pub(crate) async fn message_update(&self, event: &MessageUpdate, reporter: &Reporter) {
        dispatch(&self.message_update, event, "MESSAGE_UPDATE", reporter).await;
    }

    pub(crate) async fn message_delete(&self, event: &MessageDelete, reporter: &Reporter) {
        dispatch(&self.message_delete, event, "MESSAGE_DELETE", reporter).await;
    }

    pub(crate) async fn message_delete_bulk(&self, event: &MessageDeleteBulk, reporter: &Reporter) {
        dispatch(
            &self.message_delete_bulk,
            event,
            "MESSAGE_DELETE_BULK",
            reporter,
        )
        .await;
    }

    pub(crate) async fn ready(&self, event: &Ready, reporter: &Reporter) {
        dispatch(&self.ready, event, "READY", reporter).await;
    }

    pub(crate) async fn message(&self, event: &Message, reporter: &Reporter) {
        dispatch(&self.message, event, "MESSAGE_CREATE", reporter).await;
    }
}

async fn dispatch<T: ?Sized>(
    handlers: &[Box<dyn Handler<T>>],
    event: &T,
    event_name: &str,
    reporter: &Reporter,
) {
    for handler in handlers {
        if let Err(error) = handler.call(event).await {
            reporter.report(ErrorSource::Event, Some(event_name), &error);
        }
    }
}

trait Handler<T: ?Sized>: Send + Sync {
    fn call<'a>(&'a self, event: &'a T) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>>;
}

impl<T: ?Sized, F> Handler<T> for F
where
    F: AsyncFn(&T) -> Result<()> + Send + Sync,
{
    fn call<'a>(&'a self, event: &'a T) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        Box::pin(self(event))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{Handler, dispatch};
    use crate::hooks::Reporter;

    #[tokio::test]
    async fn keeps_registration_order_after_a_handler_error() {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let first = Arc::clone(&observed);
        let second = Arc::clone(&observed);
        let handlers: Vec<Box<dyn Handler<i32>>> = vec![
            Box::new(async move |value: &i32| {
                first.lock().unwrap().push(*value);
                Err(crate::Error::msg("最初のイベントハンドラーのエラー"))
            }),
            Box::new(async move |value: &i32| {
                second.lock().unwrap().push(*value + 1);
                Ok(())
            }),
        ];
        dispatch(&handlers, &10, "TEST", &Reporter::default()).await;
        assert_eq!(*observed.lock().unwrap(), vec![10, 11]);
    }
}
