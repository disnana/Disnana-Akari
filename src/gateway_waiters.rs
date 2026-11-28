use std::sync::Arc;

use twilight_gateway::{Event, EventTypeFlags, Intents};
use twilight_model::channel::Message;

use crate::{EventWaiters, Result, message_events::IncomingEvent};

/// Gateway の受信条件と待機ハンドルの稼働期間を結び付ける。
pub(crate) struct WaiterRuntime {
    handle: Option<EventWaiters>,
    messages: bool,
    reactions: bool,
    voice_states: bool,
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
            voice_states: handle.is_some() && intents.contains(Intents::GUILD_VOICE_STATES),
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
        if self.voice_states {
            events |= EventTypeFlags::VOICE_STATE_UPDATE;
        }
        events
    }

    pub(crate) fn activate(&self) {
        if let Some(handle) = &self.handle {
            handle.activate(self.messages, self.reactions, self.voice_states);
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

    pub(crate) fn observe(&self, incoming: &Result<IncomingEvent>) {
        let Some(handle) = &self.handle else {
            return;
        };
        match incoming {
            Ok(IncomingEvent::Standard(Event::ReactionAdd(reaction))) => {
                if let Some(cutoff) = handle.reaction_cutoff(reaction.channel_id, reaction.user_id)
                {
                    handle.deliver_reaction(Arc::new(reaction.as_ref().clone()), cutoff);
                }
            }
            Ok(IncomingEvent::Standard(Event::VoiceStateUpdate(update))) => {
                if let Some(guild) = update.guild_id
                    && let Some(cutoff) = handle.voice_state_cutoff(guild, update.user_id)
                {
                    handle.deliver_voice_state(Arc::new(update.0.clone()), cutoff);
                }
            }
            _ => {}
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
