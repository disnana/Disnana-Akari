use serde::Deserialize;
use twilight_gateway::EventTypeFlags;
use twilight_model::{
    channel::message::EmojiReactionType,
    id::{
        Id,
        marker::{ChannelMarker, GuildMarker, MessageMarker},
    },
};

pub use twilight_model::gateway::payload::incoming::{
    ReactionAdd, ReactionRemove, ReactionRemoveAll,
};

use super::{Events, Handler, Reporter, adapt_handler, dispatch};

/// メッセージから指定の絵文字のリアクションをすべて除去した通知です。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
pub struct ReactionRemoveEmoji {
    /// メッセージがあるチャンネルの ID。
    pub channel_id: Id<ChannelMarker>,
    /// メッセージの ID。
    pub message_id: Id<MessageMarker>,
    /// サーバー内のメッセージの場合のサーバー ID。
    pub guild_id: Option<Id<GuildMarker>>,
    /// 除去された Unicode またはカスタム絵文字。
    pub emoji: EmojiReactionType,
}

pub(crate) enum ReactionEvent {
    Add(Box<ReactionAdd>),
    Remove(Box<ReactionRemove>),
    Clear(ReactionRemoveAll),
    ClearEmoji(ReactionRemoveEmoji),
}

#[derive(Default)]
pub(super) struct ReactionHandlers {
    add: Vec<Box<dyn Handler<ReactionAdd>>>,
    remove: Vec<Box<dyn Handler<ReactionRemove>>>,
    clear: Vec<Box<dyn Handler<ReactionRemoveAll>>>,
    clear_emoji: Vec<Box<dyn Handler<ReactionRemoveEmoji>>>,
}

impl std::fmt::Debug for ReactionHandlers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReactionHandlers")
            .field("add", &self.add.len())
            .field("remove", &self.remove.len())
            .field("clear", &self.clear.len())
            .field("clear_emoji", &self.clear_emoji.len())
            .finish()
    }
}

impl Events {
    /// メッセージにリアクションが追加されたときに呼ぶ関数を登録します。
    ///
    /// サーバーでは `GUILD_MESSAGE_REACTIONS`、DM では `DIRECT_MESSAGE_REACTIONS` を指定します。
    pub fn on_raw_reaction_add<E>(
        &mut self,
        handler: impl AsyncFn(&ReactionAdd) -> std::result::Result<(), E> + Send + Sync + 'static,
    ) where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.reactions
            .add
            .push(Box::new(adapt_handler!(handler, event: &ReactionAdd)));
    }

    /// メッセージからリアクションが 1 件外されたときに呼ぶ関数を登録します。
    ///
    /// 必要な Intent は [`Self::on_raw_reaction_add`] と同じです。
    pub fn on_raw_reaction_remove<E>(
        &mut self,
        handler: impl AsyncFn(&ReactionRemove) -> std::result::Result<(), E> + Send + Sync + 'static,
    ) where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.reactions
            .remove
            .push(Box::new(adapt_handler!(handler, event: &ReactionRemove)));
    }

    /// メッセージからすべてのリアクションが除去されたときに呼ぶ関数を登録します。
    ///
    /// 必要な Intent は [`Self::on_raw_reaction_add`] と同じです。
    pub fn on_raw_reaction_clear<E>(
        &mut self,
        handler: impl AsyncFn(&ReactionRemoveAll) -> std::result::Result<(), E> + Send + Sync + 'static,
    ) where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.reactions
            .clear
            .push(Box::new(adapt_handler!(handler, event: &ReactionRemoveAll)));
    }

    /// メッセージから指定の絵文字のリアクションがすべて除去されたときに呼ぶ関数を登録します。
    ///
    /// 必要な Intent は [`Self::on_raw_reaction_add`] と同じです。
    pub fn on_raw_reaction_clear_emoji<E>(
        &mut self,
        handler: impl AsyncFn(&ReactionRemoveEmoji) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.reactions.clear_emoji.push(Box::new(adapt_handler!(
            handler,
            event: &ReactionRemoveEmoji
        )));
    }
}

impl ReactionHandlers {
    pub(super) fn types(&self) -> EventTypeFlags {
        let mut types = EventTypeFlags::empty();
        types.set(EventTypeFlags::REACTION_ADD, !self.add.is_empty());
        types.set(EventTypeFlags::REACTION_REMOVE, !self.remove.is_empty());
        types.set(EventTypeFlags::REACTION_REMOVE_ALL, !self.clear.is_empty());
        types.set(
            EventTypeFlags::REACTION_REMOVE_EMOJI,
            !self.clear_emoji.is_empty(),
        );
        types
    }

    pub(super) async fn dispatch(&self, event: &ReactionEvent, reporter: &Reporter) {
        match event {
            ReactionEvent::Add(event) => {
                dispatch(&self.add, event.as_ref(), "MESSAGE_REACTION_ADD", reporter).await;
            }
            ReactionEvent::Remove(event) => {
                dispatch(
                    &self.remove,
                    event.as_ref(),
                    "MESSAGE_REACTION_REMOVE",
                    reporter,
                )
                .await;
            }
            ReactionEvent::Clear(event) => {
                dispatch(&self.clear, event, "MESSAGE_REACTION_REMOVE_ALL", reporter).await;
            }
            ReactionEvent::ClearEmoji(event) => {
                dispatch(
                    &self.clear_emoji,
                    event,
                    "MESSAGE_REACTION_REMOVE_EMOJI",
                    reporter,
                )
                .await;
            }
        }
    }
}
