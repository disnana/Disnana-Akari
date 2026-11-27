use std::sync::Arc;

use twilight_gateway::EventTypeFlags;
use twilight_model::channel::Message;

use crate::CachedMessageEdit;

use super::{Events, Handler, Reporter, adapt_handler, dispatch};

#[derive(Default)]
pub(super) struct CachedMessageHandlers {
    edit: Vec<Box<dyn Handler<CachedMessageEdit>>>,
    delete: Vec<Box<dyn Handler<Message>>>,
    bulk_delete: Vec<Box<dyn Handler<[Arc<Message>]>>>,
}

impl Events {
    /// 保存済みのメッセージが更新されたときに、更新前後の情報を受け取ります。
    ///
    /// [`crate::Bot::cache_messages`] でキャッシュを有効にします。
    pub fn on_message_edit<E>(
        &mut self,
        handler: impl AsyncFn(&CachedMessageEdit) -> std::result::Result<(), E> + Send + Sync + 'static,
    ) where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.cached_messages
            .edit
            .push(Box::new(adapt_handler!(handler, event: &CachedMessageEdit)));
    }

    /// 保存済みのメッセージが単独で削除されたときに、最後に保存した内容を受け取ります。
    ///
    /// [`crate::Bot::cache_messages`] でキャッシュを有効にします。
    pub fn on_message_delete<E>(
        &mut self,
        handler: impl AsyncFn(&Message) -> std::result::Result<(), E> + Send + Sync + 'static,
    ) where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.cached_messages
            .delete
            .push(Box::new(adapt_handler!(handler, event: &Message)));
    }

    /// 一括削除のうち、保存済みだったメッセージだけを受け取ります。
    ///
    /// [`crate::Bot::cache_messages`] と `GUILD_MESSAGES` Intent が必要です。
    pub fn on_bulk_message_delete<E>(
        &mut self,
        handler: impl AsyncFn(&[Arc<Message>]) -> std::result::Result<(), E> + Send + Sync + 'static,
    ) where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.cached_messages
            .bulk_delete
            .push(Box::new(adapt_handler!(handler, event: &[Arc<Message>])));
    }
}

impl CachedMessageHandlers {
    pub(super) fn types(&self) -> EventTypeFlags {
        let mut types = EventTypeFlags::empty();
        types.set(EventTypeFlags::MESSAGE_UPDATE, !self.edit.is_empty());
        types.set(EventTypeFlags::MESSAGE_DELETE, !self.delete.is_empty());
        types.set(
            EventTypeFlags::MESSAGE_DELETE_BULK,
            !self.bulk_delete.is_empty(),
        );
        types
    }

    pub(super) async fn edit(&self, event: &CachedMessageEdit, reporter: &Reporter) {
        dispatch(&self.edit, event, "MESSAGE_UPDATE", reporter).await;
    }

    pub(super) async fn delete(&self, event: &Message, reporter: &Reporter) {
        dispatch(&self.delete, event, "MESSAGE_DELETE", reporter).await;
    }

    pub(super) async fn bulk_delete(&self, event: &[Arc<Message>], reporter: &Reporter) {
        dispatch(&self.bulk_delete, event, "MESSAGE_DELETE_BULK", reporter).await;
    }
}
