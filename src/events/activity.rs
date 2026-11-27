use twilight_gateway::EventTypeFlags;

pub use twilight_model::gateway::payload::incoming::{
    MessagePollVoteAdd, MessagePollVoteRemove, TypingStart,
};

use super::{Events, Handler, Reporter, adapt_handler, dispatch};

pub(crate) enum ActivityEvent {
    PollVoteAdd(MessagePollVoteAdd),
    PollVoteRemove(MessagePollVoteRemove),
    Typing(Box<TypingStart>),
}

#[derive(Default)]
pub(super) struct ActivityHandlers {
    poll_vote_add: Vec<Box<dyn Handler<MessagePollVoteAdd>>>,
    poll_vote_remove: Vec<Box<dyn Handler<MessagePollVoteRemove>>>,
    typing: Vec<Box<dyn Handler<TypingStart>>>,
}

impl std::fmt::Debug for ActivityHandlers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActivityHandlers")
            .field("poll_vote_add", &self.poll_vote_add.len())
            .field("poll_vote_remove", &self.poll_vote_remove.len())
            .field("typing", &self.typing.len())
            .finish()
    }
}

impl Events {
    /// 投票の回答が選ばれたときに呼ぶ関数を登録します。
    ///
    /// 複数の回答が選ばれると、回答ごとに通知が届きます。
    /// サーバーでは `GUILD_MESSAGE_POLLS`、DM では `DIRECT_MESSAGE_POLLS` を指定します。
    pub fn on_raw_poll_vote_add<E>(
        &mut self,
        handler: impl AsyncFn(&MessagePollVoteAdd) -> std::result::Result<(), E> + Send + Sync + 'static,
    ) where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.activity.poll_vote_add.push(Box::new(
            adapt_handler!(handler, event: &MessagePollVoteAdd),
        ));
    }

    /// 投票の回答が取り消されたときに呼ぶ関数を登録します。
    ///
    /// 必要な Intent は [`Self::on_raw_poll_vote_add`] と同じです。
    pub fn on_raw_poll_vote_remove<E>(
        &mut self,
        handler: impl AsyncFn(&MessagePollVoteRemove) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.activity.poll_vote_remove.push(Box::new(adapt_handler!(
            handler,
            event: &MessagePollVoteRemove
        )));
    }

    /// メッセージの入力が始まったときに呼ぶ関数を登録します。
    ///
    /// 時刻は Unix 秒で届きます。サーバーでは `GUILD_MESSAGE_TYPING`、DM では
    /// `DIRECT_MESSAGE_TYPING` を指定します。
    pub fn on_raw_typing<E>(
        &mut self,
        handler: impl AsyncFn(&TypingStart) -> std::result::Result<(), E> + Send + Sync + 'static,
    ) where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.activity
            .typing
            .push(Box::new(adapt_handler!(handler, event: &TypingStart)));
    }
}

impl ActivityHandlers {
    pub(super) fn types(&self) -> EventTypeFlags {
        let mut types = EventTypeFlags::empty();
        types.set(
            EventTypeFlags::MESSAGE_POLL_VOTE_ADD,
            !self.poll_vote_add.is_empty(),
        );
        types.set(
            EventTypeFlags::MESSAGE_POLL_VOTE_REMOVE,
            !self.poll_vote_remove.is_empty(),
        );
        types.set(EventTypeFlags::TYPING_START, !self.typing.is_empty());
        types
    }

    pub(super) async fn dispatch(&self, event: &ActivityEvent, reporter: &Reporter) {
        match event {
            ActivityEvent::PollVoteAdd(event) => {
                dispatch(
                    &self.poll_vote_add,
                    event,
                    "MESSAGE_POLL_VOTE_ADD",
                    reporter,
                )
                .await;
            }
            ActivityEvent::PollVoteRemove(event) => {
                dispatch(
                    &self.poll_vote_remove,
                    event,
                    "MESSAGE_POLL_VOTE_REMOVE",
                    reporter,
                )
                .await;
            }
            ActivityEvent::Typing(event) => {
                dispatch(&self.typing, event.as_ref(), "TYPING_START", reporter).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::sync::mpsc;
    use twilight_gateway::{Intents, parse};
    use twilight_http::Client;
    use twilight_model::id::Id;

    use super::*;
    use crate::{
        Error, ErrorSource, NoMessageHandler, QueueKind, Result, dispatch_events,
        forward_lifecycle_event, handler_event_types, hooks::Hooks, message_events::IncomingEvent,
        permissions::PermissionResolver,
    };

    const VOTE_ADD: &str = r#"{"op":0,"s":1,"t":"MESSAGE_POLL_VOTE_ADD","d":{"answer_id":2,"channel_id":"9","guild_id":"8","message_id":"7","user_id":"6"}}"#;
    const VOTE_REMOVE: &str = r#"{"op":0,"s":2,"t":"MESSAGE_POLL_VOTE_REMOVE","d":{"answer_id":3,"channel_id":"9","message_id":"7","user_id":"6"}}"#;
    const TYPING: &str = r#"{"op":0,"s":3,"t":"TYPING_START","d":{"channel_id":"9","timestamp":1500000000,"user_id":"6"}}"#;

    #[test]
    fn checks_poll_and_typing_intents_independently() {
        let mut events = Events::new();
        assert!(
            handler_event_types(&events, Intents::empty())
                .unwrap()
                .is_empty()
        );
        events.on_raw_poll_vote_add(async |_: &MessagePollVoteAdd| -> Result<()> { Ok(()) });
        let poll_intents = Intents::GUILD_MESSAGE_POLLS | Intents::DIRECT_MESSAGE_POLLS;
        let typing_intents = Intents::GUILD_MESSAGE_TYPING | Intents::DIRECT_MESSAGE_TYPING;
        assert!(matches!(handler_event_types(&events, typing_intents),
            Err(Error::MissingEventIntents { any_of }) if any_of == poll_intents));
        assert_eq!(
            handler_event_types(&events, Intents::DIRECT_MESSAGE_POLLS).unwrap(),
            EventTypeFlags::MESSAGE_POLL_VOTE_ADD
        );

        events.on_raw_typing(async |_: &TypingStart| -> Result<()> { Ok(()) });
        assert!(matches!(handler_event_types(&events, poll_intents),
            Err(Error::MissingEventIntents { any_of }) if any_of == typing_intents));
        let flags = handler_event_types(
            &events,
            Intents::GUILD_MESSAGE_POLLS | Intents::DIRECT_MESSAGE_TYPING,
        )
        .unwrap();
        assert_eq!(
            flags,
            EventTypeFlags::MESSAGE_POLL_VOTE_ADD | EventTypeFlags::TYPING_START
        );
        assert!(parse(VOTE_REMOVE.to_owned(), flags).unwrap().is_none());
    }

    #[tokio::test]
    async fn dispatches_raw_activity_in_order_and_reports_queue_backpressure() {
        let mut events = Events::new();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let added = Arc::clone(&observed);
        let removed = Arc::clone(&observed);
        let typing = Arc::clone(&observed);
        events.on_raw_poll_vote_add(async move |vote: &MessagePollVoteAdd| -> Result<()> {
            assert_eq!(vote.guild_id, Some(Id::new(8)));
            assert_eq!(vote.message_id, Id::new(7));
            added
                .lock()
                .unwrap()
                .push(("add", u64::from(vote.answer_id)));
            Ok(())
        });
        events.on_raw_poll_vote_remove(async move |vote: &MessagePollVoteRemove| -> Result<()> {
            assert_eq!(vote.guild_id, None);
            assert_eq!(vote.user_id, Id::new(6));
            removed
                .lock()
                .unwrap()
                .push(("remove", u64::from(vote.answer_id)));
            Ok(())
        });
        events.on_raw_typing(async move |event: &TypingStart| -> Result<()> {
            assert_eq!(event.guild_id, None);
            assert_eq!(event.member, None);
            assert_eq!(event.channel_id, Id::new(9));
            typing.lock().unwrap().push(("typing", event.timestamp));
            Ok(())
        });

        let errors = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&errors);
        let hooks = Hooks {
            reporter: Reporter::new(move |report| {
                assert_eq!(report.source, ErrorSource::QueueFull);
                assert!(matches!(report.error, Error::QueueFull(QueueKind::Event)));
                recorded
                    .lock()
                    .unwrap()
                    .extend(report.name.map(str::to_owned));
            }),
            ..Hooks::default()
        };
        let flags = events.activity_types();
        let (tx, rx) = mpsc::channel(3);
        for payload in [VOTE_ADD, TYPING, VOTE_REMOVE, TYPING] {
            let event = parse(payload.to_owned(), flags).unwrap().unwrap();
            let incoming = IncomingEvent::Standard(event.into());
            assert!(forward_lifecycle_event(Ok(incoming), &tx, &hooks.reporter, None).is_none());
        }
        drop(tx);
        dispatch_events(
            rx,
            None::<NoMessageHandler>,
            &events,
            &Client::new("test".into()),
            &PermissionResolver::new(None),
            &hooks,
        )
        .await;
        assert_eq!(*errors.lock().unwrap(), ["TYPING_START"]);
        assert_eq!(
            *observed.lock().unwrap(),
            [("add", 2), ("typing", 1_500_000_000), ("remove", 3)]
        );
    }
}
