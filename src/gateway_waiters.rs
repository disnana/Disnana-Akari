use std::sync::Arc;

use twilight_gateway::{Event, EventTypeFlags, Intents};
use twilight_model::channel::Message;

use crate::{EventWaiters, Result, message_events::IncomingEvent};

/// Gateway の受信条件と待機ハンドルの稼働期間を結び付ける。
pub(crate) struct WaiterRuntime {
    handle: Option<EventWaiters>,
    messages: bool,
    reactions: bool,
}

impl WaiterRuntime {
    pub(crate) fn new(handle: Option<EventWaiters>, intents: Intents) -> Self {
        Self {
            messages: handle.is_some()
                && intents.intersects(Intents::GUILD_MESSAGES | Intents::DIRECT_MESSAGES),
            reactions: handle.is_some()
                && intents.intersects(
                    Intents::GUILD_MESSAGE_REACTIONS | Intents::DIRECT_MESSAGE_REACTIONS,
                ),
            handle,
        }
    }

    pub(crate) fn event_types(&self) -> EventTypeFlags {
        let mut events = EventTypeFlags::empty();
        if self.messages {
            events |= EventTypeFlags::MESSAGE_CREATE;
        }
        if self.reactions {
            events |= EventTypeFlags::REACTION_ADD;
        }
        events
    }

    pub(crate) fn activate(&self) {
        if let Some(handle) = &self.handle {
            handle.activate(self.messages, self.reactions);
        }
    }

    pub(crate) fn message_cutoff(&self, message: &Message) -> Option<u64> {
        self.handle
            .as_ref()?
            .message_cutoff(message.channel_id, message.author.id)
    }

    pub(crate) fn deliver_message(&self, message: Arc<Message>, cutoff: u64) {
        if let Some(handle) = &self.handle {
            handle.deliver_message(message, cutoff);
        }
    }

    pub(crate) fn observe_reaction(&self, incoming: &Result<IncomingEvent>) {
        if let Some(handle) = &self.handle
            && let Ok(IncomingEvent::Standard(Event::ReactionAdd(reaction))) = incoming
            && let Some(cutoff) = handle.reaction_cutoff(reaction.channel_id, reaction.user_id)
        {
            handle.deliver_reaction(Arc::new(reaction.as_ref().clone()), cutoff);
        }
    }

    pub(crate) fn close(&self) {
        if let Some(handle) = &self.handle {
            handle.close();
        }
    }
}

impl Drop for WaiterRuntime {
    fn drop(&mut self) {
        self.close();
    }
}
