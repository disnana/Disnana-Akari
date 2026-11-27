use std::{future::poll_fn, pin::Pin};

use futures_util::Stream;
use serde::{Deserialize, Deserializer};
use twilight_gateway::{
    Event, EventTypeFlags, Message, Shard, ShardState,
    error::{ReceiveMessageError, ReceiveMessageErrorType},
    parse,
};
use twilight_model::{
    application::interaction::InteractionType,
    channel::{
        Attachment, ChannelMention,
        message::{Component, Embed, Mention, MessageCall, MessageFlags},
    },
    gateway::{
        OpCode,
        event::GatewayEventDeserializer,
        payload::incoming::{InteractionCreate, MemberUpdate},
    },
    guild::Member,
    id::{
        Id,
        marker::{ChannelMarker, GuildMarker, MessageMarker, RoleMarker, UserMarker},
    },
    poll::Poll,
    user::AvatarDecorationData,
    util::Timestamp,
};

use crate::{
    Error, ReactionRemoveEmoji, Result,
    interactions::{RadioSubmission, capture_modal_radios},
};

const CUSTOM_DISPATCHES: EventTypeFlags =
    EventTypeFlags::MESSAGE_UPDATE.union(EventTypeFlags::REACTION_REMOVE_EMOJI);

/// Discord が送った、メッセージの部分更新です。
///
/// 編集前の値は含みません。Discord が送信を省略した項目や `null` の項目は `None` です。
/// 本文が空文字で届いた場合は `Some` に空文字が入り、`None` と区別できます。
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[non_exhaustive]
#[serde(from = "MessageUpdatePayload")]
pub struct MessageUpdate {
    /// 更新されたメッセージの ID。
    pub id: Id<MessageMarker>,
    /// メッセージがあるチャンネルの ID。
    pub channel_id: Id<ChannelMarker>,
    /// メッセージがサーバー内にある場合のサーバー ID。
    pub guild_id: Option<Id<GuildMarker>>,
    /// Discord が送った場合の投稿者 ID。
    pub author_id: Option<Id<UserMarker>>,
    /// 更新ペイロードに含まれる本文。本文を受信できるかは Discord の Intent にも依存します。
    pub content: Option<String>,
    /// Discord が送った場合の編集時刻。
    pub edited_timestamp: Option<Timestamp>,
    /// 更新ペイロードに含まれる添付ファイル。
    pub attachments: Option<Vec<Attachment>>,
    /// 更新ペイロードに含まれる埋め込み。
    pub embeds: Option<Vec<Embed>>,
    /// 更新ペイロードに含まれるコンポーネント。
    pub components: Option<Vec<Component>>,
    /// 更新ペイロードに含まれるフラグ。
    pub flags: Option<MessageFlags>,
    timestamp_provided: bool,
    flags_provided: bool,
    additional: Option<Box<AdditionalMessageFields>>,
}

#[derive(Deserialize)]
struct MessageUpdatePayload {
    id: Id<MessageMarker>,
    channel_id: Id<ChannelMarker>,
    guild_id: Option<Id<GuildMarker>>,
    #[serde(rename = "author", default, deserialize_with = "author_id")]
    author_id: Option<Id<UserMarker>>,
    content: Option<String>,
    #[serde(default, deserialize_with = "nullable_field")]
    edited_timestamp: Option<Option<Timestamp>>,
    attachments: Option<Vec<Attachment>>,
    embeds: Option<Vec<Embed>>,
    components: Option<Vec<Component>>,
    #[serde(default, deserialize_with = "nullable_field")]
    flags: Option<Option<MessageFlags>>,
    #[serde(default, deserialize_with = "nullable_field")]
    call: Option<Option<MessageCall>>,
    #[serde(default, deserialize_with = "nullable_field")]
    poll: Option<Option<Poll>>,
    pinned: Option<bool>,
    tts: Option<bool>,
    mention_everyone: Option<bool>,
    mention_channels: Option<Vec<ChannelMention>>,
    mention_roles: Option<Vec<Id<RoleMarker>>>,
    mentions: Option<Vec<Mention>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct AdditionalMessageFields {
    call: Option<Option<MessageCall>>,
    poll: Option<Option<Poll>>,
    pinned: Option<bool>,
    tts: Option<bool>,
    mention_everyone: Option<bool>,
    mention_channels: Option<Vec<ChannelMention>>,
    mention_roles: Option<Vec<Id<RoleMarker>>>,
    mentions: Option<Vec<Mention>>,
}

impl From<MessageUpdatePayload> for MessageUpdate {
    fn from(payload: MessageUpdatePayload) -> Self {
        let additional = AdditionalMessageFields {
            call: payload.call,
            poll: payload.poll,
            pinned: payload.pinned,
            tts: payload.tts,
            mention_everyone: payload.mention_everyone,
            mention_channels: payload.mention_channels,
            mention_roles: payload.mention_roles,
            mentions: payload.mentions,
        };
        Self {
            id: payload.id,
            channel_id: payload.channel_id,
            guild_id: payload.guild_id,
            author_id: payload.author_id,
            content: payload.content,
            edited_timestamp: payload.edited_timestamp.flatten(),
            attachments: payload.attachments,
            embeds: payload.embeds,
            components: payload.components,
            flags: payload.flags.flatten(),
            timestamp_provided: payload.edited_timestamp.is_some(),
            flags_provided: payload.flags.is_some(),
            additional: (additional != AdditionalMessageFields::default())
                .then(|| Box::new(additional)),
        }
    }
}

impl MessageUpdate {
    pub(crate) fn apply_to(&self, message: &mut twilight_model::channel::Message) {
        if let Some(content) = &self.content {
            message.content.clone_from(content);
        }
        if let Some(attachments) = &self.attachments {
            message.attachments.clone_from(attachments);
        }
        if let Some(embeds) = &self.embeds {
            message.embeds.clone_from(embeds);
        }
        if let Some(components) = &self.components {
            message.components.clone_from(components);
        }
        if self.timestamp_provided {
            message.edited_timestamp = self.edited_timestamp;
        }
        if self.flags_provided {
            message.flags = self.flags;
        }
        if let Some(additional) = &self.additional {
            additional.apply_to(message);
        }
    }
}

impl AdditionalMessageFields {
    fn apply_to(&self, message: &mut twilight_model::channel::Message) {
        if let Some(call) = &self.call {
            message.call.clone_from(call);
        }
        if let Some(poll) = &self.poll {
            message.poll.clone_from(poll);
        }
        if let Some(pinned) = self.pinned {
            message.pinned = pinned;
        }
        if let Some(tts) = self.tts {
            message.tts = tts;
        }
        if let Some(everyone) = self.mention_everyone {
            message.mention_everyone = everyone;
        }
        if let Some(channels) = &self.mention_channels {
            message.mention_channels.clone_from(channels);
        }
        if let Some(roles) = &self.mention_roles {
            message.mention_roles.clone_from(roles);
        }
        if let Some(mentions) = &self.mentions {
            message.mentions.clone_from(mentions);
        }
    }
}

fn nullable_field<'de, D, T>(deserializer: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

fn author_id<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Id<UserMarker>>, D::Error> {
    #[derive(Deserialize)]
    struct Author {
        id: Id<UserMarker>,
    }
    Ok(Option::<Author>::deserialize(deserializer)?.map(|author| author.id))
}

pub(crate) enum IncomingEvent {
    Standard(Event),
    MessageUpdate(MessageUpdate),
    ReactionRemoveEmoji(ReactionRemoveEmoji),
    ModalWithRadio(Box<InteractionCreate>, Vec<RadioSubmission>),
    MemberUpdate(Box<MemberUpdate>, Option<MemberUpdateFields>),
    VoiceNotification(crate::events::VoiceNotification),
}

#[derive(Clone, Copy, Default)]
pub(crate) struct GatewayDecodeOptions {
    pub(crate) capture_modal_radios: bool,
    pub(crate) capture_member_fields: bool,
    pub(crate) voice_notifications: crate::events::VoiceNotificationTypes,
}

#[derive(Deserialize)]
pub(crate) struct MemberUpdateFields {
    #[serde(default, deserialize_with = "provided")]
    avatar: bool,
    #[serde(default, deserialize_with = "nullable_field")]
    avatar_decoration_data: Option<Option<AvatarDecorationData>>,
    #[serde(default, deserialize_with = "provided")]
    communication_disabled_until: bool,
    #[serde(default, deserialize_with = "provided")]
    joined_at: bool,
    #[serde(default, deserialize_with = "provided")]
    nick: bool,
    #[serde(default, deserialize_with = "provided")]
    pending: bool,
    #[serde(default, deserialize_with = "provided")]
    premium_since: bool,
}

fn provided<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<bool, D::Error> {
    serde::de::IgnoredAny::deserialize(deserializer).map(|_| true)
}

impl MemberUpdateFields {
    pub(crate) fn apply(self, event: &MemberUpdate, cached: &mut Member) {
        if self.avatar {
            cached.avatar = event.avatar;
        }
        if let Some(avatar_decoration_data) = self.avatar_decoration_data {
            cached.avatar_decoration_data = avatar_decoration_data;
        }
        if self.communication_disabled_until {
            cached.communication_disabled_until = event.communication_disabled_until;
        }
        if self.joined_at {
            cached.joined_at = event.joined_at;
        }
        if self.nick {
            cached.nick.clone_from(&event.nick);
        }
        if self.pending {
            cached.pending = event.pending;
        }
        if self.premium_since {
            cached.premium_since = event.premium_since;
        }
        if let Some(flags) = event.flags {
            cached.flags = flags;
        }
        if let Some(deaf) = event.deaf {
            cached.deaf = deaf;
        }
        if let Some(mute) = event.mute {
            cached.mute = mute;
        }
        cached.roles.clone_from(&event.roles);
        cached.user.clone_from(&event.user);
    }
}

pub(crate) async fn next_event(
    shard: &mut Shard,
    types: EventTypeFlags,
    options: GatewayDecodeOptions,
    observe_state: impl Fn(ShardState, ShardState),
) -> Option<Result<IncomingEvent>> {
    // Twilight はフレームやエラーを返さず接続状態を変えることがあります。
    let next_message = |shard: &mut Shard, cx: &mut std::task::Context<'_>| {
        let previous = shard.state();
        let message = Pin::new(&mut *shard).poll_next(cx);
        let current = shard.state();
        if previous != current {
            observe_state(previous, current);
        }
        message
    };

    while let Some(message) = poll_fn(|cx| next_message(shard, cx)).await {
        match message {
            Ok(Message::Close(frame)) => {
                return Some(Ok(IncomingEvent::Standard(Event::GatewayClose(frame))));
            }
            Ok(Message::Text(json)) => {
                if let Some(event) = decode_text_with_options(json, types, options) {
                    return Some(event);
                }
            }
            Err(error) => return Some(Err(error.into())),
        }
    }
    None
}

pub(crate) fn lost_gateway_event(error: &Error, relevant: EventTypeFlags) -> bool {
    let Error::GatewayReceive(error) = error else {
        return false;
    };
    let ReceiveMessageErrorType::Deserializing { event } = error.kind() else {
        return false;
    };
    GatewayEventDeserializer::from_json(event).is_none_or(|header| {
        OpCode::from(header.op())
            .and_then(|op| EventTypeFlags::try_from((op, header.event_type())).ok())
            .is_some_and(|kind| relevant.intersects(kind))
    })
}

#[cfg(test)]
pub(crate) fn decode_text(
    json: String,
    types: EventTypeFlags,
    capture_radios: bool,
) -> Option<Result<IncomingEvent>> {
    decode_text_with_options(
        json,
        types,
        GatewayDecodeOptions {
            capture_modal_radios: capture_radios,
            ..GatewayDecodeOptions::default()
        },
    )
}

pub(crate) fn decode_text_with_options(
    json: String,
    types: EventTypeFlags,
    options: GatewayDecodeOptions,
) -> Option<Result<IncomingEvent>> {
    let capture_members =
        options.capture_member_fields && types.contains(EventTypeFlags::MEMBER_UPDATE);
    if (types.intersects(CUSTOM_DISPATCHES)
        || options.capture_modal_radios
        || capture_members
        || !options.voice_notifications.is_empty())
        && let Some(header) = GatewayEventDeserializer::from_json(&json)
        && header.op() == OpCode::Dispatch as u8
    {
        #[derive(Deserialize)]
        struct Dispatch<T> {
            d: T,
        }

        match header.event_type() {
            // Twilight 0.18 は、部分更新でも完全な Message を要求する。
            Some("MESSAGE_UPDATE") if types.contains(EventTypeFlags::MESSAGE_UPDATE) => {
                return Some(
                    serde_json::from_str::<Dispatch<MessageUpdate>>(&json)
                        .map(|payload| IncomingEvent::MessageUpdate(payload.d))
                        .map_err(Error::MessageUpdateDecode),
                );
            }
            // Twilight 0.18 は guild_id を要求するが、Discord の DM では省略される。
            Some("MESSAGE_REACTION_REMOVE_EMOJI")
                if types.contains(EventTypeFlags::REACTION_REMOVE_EMOJI) =>
            {
                return Some(
                    serde_json::from_str::<Dispatch<ReactionRemoveEmoji>>(&json)
                        .map(|payload| IncomingEvent::ReactionRemoveEmoji(payload.d))
                        .map_err(Error::ReactionRemoveEmojiDecode),
                );
            }
            Some("INTERACTION_CREATE")
                if options.capture_modal_radios
                    && types.contains(EventTypeFlags::INTERACTION_CREATE) =>
            {
                return decode_interaction(json, types);
            }
            Some("GUILD_MEMBER_UPDATE") if capture_members => {
                let fields = serde_json::from_str::<Dispatch<MemberUpdateFields>>(&json)
                    .ok()
                    .map(|payload| payload.d);
                return parse(json, types).transpose().map(|event| {
                    event
                        .map(|event| match event.into() {
                            Event::MemberUpdate(member) => {
                                IncomingEvent::MemberUpdate(member, fields)
                            }
                            event => IncomingEvent::Standard(event),
                        })
                        .map_err(Error::from)
                });
            }
            Some(name) if !options.voice_notifications.is_empty() => {
                if let Some(event) = options.voice_notifications.decode(name, &json) {
                    return Some(event.map(IncomingEvent::VoiceNotification));
                }
            }
            _ => {}
        }
    }
    match parse(json, types) {
        Ok(event) => event.map(|event| Ok(IncomingEvent::Standard(event.into()))),
        Err(error) => Some(
            decode_redacted_automod(&error, types)
                .map(IncomingEvent::Standard)
                .ok_or_else(|| Error::from(error)),
        ),
    }
}

fn decode_redacted_automod(error: &ReceiveMessageError, types: EventTypeFlags) -> Option<Event> {
    if !types.contains(EventTypeFlags::AUTO_MODERATION_ACTION_EXECUTION) {
        return None;
    }
    let ReceiveMessageErrorType::Deserializing { event } = error.kind() else {
        return None;
    };
    let header = GatewayEventDeserializer::from_json(event)?;
    if header.op() != OpCode::Dispatch as u8
        || header.event_type() != Some("AUTO_MODERATION_ACTION_EXECUTION")
    {
        return None;
    }

    #[derive(Deserialize)]
    struct Dispatch {
        #[serde(rename = "op")]
        _opcode: u8,
        #[serde(rename = "s")]
        _sequence: u64,
        #[serde(rename = "t")]
        _event_type: String,
        d: serde_json::Map<String, serde_json::Value>,
    }
    let mut body = serde_json::from_str::<Dispatch>(event).ok()?.d;
    if body.contains_key("content") {
        return None;
    }
    // Discord は本文を省略できるが、Twilight 0.18 の型には文字列が必要。
    body.insert("content".into(), serde_json::Value::String(String::new()));
    serde_json::from_value(serde_json::Value::Object(body))
        .ok()
        .map(Event::AutoModerationActionExecution)
}

fn decode_interaction(json: String, types: EventTypeFlags) -> Option<Result<IncomingEvent>> {
    let radios = capture_modal_radios(&json);
    let event = parse(json, types).transpose()?;
    Some(event.map_err(Error::from).and_then(|event| {
        let event = match event.into() {
            Event::InteractionCreate(interaction)
                if interaction.0.kind == InteractionType::ModalSubmit =>
            {
                let radios = radios?;
                if radios.is_empty() {
                    Event::InteractionCreate(interaction)
                } else {
                    return Ok(IncomingEvent::ModalWithRadio(interaction, radios));
                }
            }
            event => event,
        };
        Ok(IncomingEvent::Standard(event))
    }))
}

#[cfg(test)]
mod tests {
    use std::{
        future::Ready,
        sync::{Arc, Mutex},
    };

    use super::*;
    use crate::{
        Bot, Context, Events, MessageDelete, MessageDeleteBulk, ReactionAdd, ReactionRemove,
        ReactionRemoveAll, check_lifecycle_intents, dispatch_events, forward_lifecycle_event,
        hooks::Hooks, permissions::PermissionResolver,
    };
    use tokio::sync::mpsc;
    use twilight_gateway::Intents;
    use twilight_http::Client;
    use twilight_model::channel::message::EmojiReactionType;

    fn decode_text(json: String, types: EventTypeFlags) -> Option<Result<IncomingEvent>> {
        super::decode_text(json, types, false)
    }

    const EMBED_EDIT: &str = r#"{"op":0,"s":1,"t":"MESSAGE_UPDATE","d":{"id":"1","channel_id":"2","embeds":[{"type":"rich","description":"preview"}]}}"#;
    const EMPTY_EDIT: &str = r#"{"op":0,"s":2,"t":"MESSAGE_UPDATE","d":{"id":"1","channel_id":"2","guild_id":"3","author":{"id":"4"},"content":"","attachments":[]}}"#;
    const DELETE: &str = r#"{"op":0,"s":3,"t":"MESSAGE_DELETE","d":{"id":"1","channel_id":"2"}}"#;
    const BULK_DELETE: &str = r#"{"op":0,"s":4,"t":"MESSAGE_DELETE_BULK","d":{"ids":["1","5"],"channel_id":"2","guild_id":"3"}}"#;
    const INVALID_EDIT: &str = r#"{"op":0,"s":5,"t":"MESSAGE_UPDATE","d":{"channel_id":"2"}}"#;
    const REACTION_ADD: &str = r#"{"op":0,"s":6,"t":"MESSAGE_REACTION_ADD","d":{"channel_id":"2","message_id":"1","guild_id":"3","user_id":"4","message_author_id":"5","emoji":{"id":null,"name":"👍"},"burst":false,"type":0}}"#;
    const REACTION_REMOVE: &str = r#"{"op":0,"s":7,"t":"MESSAGE_REACTION_REMOVE","d":{"channel_id":"2","message_id":"1","user_id":"4","emoji":{"id":"9","name":null},"burst":true,"type":1}}"#;
    const REACTION_CLEAR: &str = r#"{"op":0,"s":8,"t":"MESSAGE_REACTION_REMOVE_ALL","d":{"channel_id":"2","message_id":"1","guild_id":"3"}}"#;
    const DM_REACTION_CLEAR_EMOJI: &str = r#"{"op":0,"s":9,"t":"MESSAGE_REACTION_REMOVE_EMOJI","d":{"channel_id":"2","message_id":"1","emoji":{"id":"9","name":null}}}"#;
    const INVALID_REACTION_CLEAR_EMOJI: &str = r#"{"op":0,"s":10,"t":"MESSAGE_REACTION_REMOVE_EMOJI","d":{"channel_id":"2","message_id":"1"}}"#;

    fn edit(payload: &str) -> MessageUpdate {
        let IncomingEvent::MessageUpdate(update) =
            decode_text(payload.to_owned(), EventTypeFlags::MESSAGE_UPDATE)
                .unwrap()
                .unwrap()
        else {
            panic!("expected a message update");
        };
        update
    }

    #[test]
    fn decodes_partial_edits_without_a_full_message() {
        let update = edit(EMBED_EDIT);
        assert_eq!(update.id.get(), 1);
        assert!(update.content.is_none());
        assert!(update.author_id.is_none());
        assert_eq!(
            update.embeds.as_ref().unwrap()[0].description.as_deref(),
            Some("preview")
        );

        let update = edit(EMPTY_EDIT);
        assert_eq!(update.content.as_deref(), Some(""));
        assert_eq!(update.author_id, Some(Id::new(4)));
        assert_eq!(update.guild_id, Some(Id::new(3)));
        assert_eq!(update.attachments.as_deref(), Some([].as_slice()));
    }

    #[test]
    fn skips_unsubscribed_edits_and_reports_malformed_subscribed_edits() {
        assert!(decode_text(INVALID_EDIT.to_owned(), EventTypeFlags::MESSAGE_DELETE).is_none());
        assert!(matches!(
            decode_text(INVALID_EDIT.to_owned(), EventTypeFlags::MESSAGE_UPDATE),
            Some(Err(Error::MessageUpdateDecode(_)))
        ));
    }

    #[tokio::test]
    async fn dispatches_gateway_updates_and_deletions_in_receive_order() {
        let observed = Mutex::new(Vec::new());
        let hooks = Hooks::default();
        let http = Client::new("test".into());
        let permissions = PermissionResolver::new(None);
        let mut events = Events::new();
        assert!(events.message_lifecycle_types().is_empty());
        let observed = Arc::new(observed);
        let edits = observed.clone();
        let deletes = observed.clone();
        let bulk_deletes = observed.clone();
        events.on_raw_message_edit(async move |update: &MessageUpdate| -> Result<()> {
            assert_eq!(update.author_id, None);
            edits.lock().unwrap().push(("edit", update.id.get()));
            Ok(())
        });
        assert_eq!(
            events.message_lifecycle_types(),
            EventTypeFlags::MESSAGE_UPDATE
        );
        events.on_raw_message_delete(async move |delete: &MessageDelete| -> Result<()> {
            assert_eq!(delete.guild_id, None);
            deletes.lock().unwrap().push(("delete", delete.id.get()));
            Ok(())
        });
        events.on_raw_bulk_message_delete(async move |delete: &MessageDeleteBulk| -> Result<()> {
            assert_eq!(delete.guild_id, Some(Id::new(3)));
            bulk_deletes
                .lock()
                .unwrap()
                .extend(delete.ids.iter().map(|id| ("bulk", id.get())));
            Ok(())
        });

        let flags = events.message_lifecycle_types();
        assert!(check_lifecycle_intents(Intents::GUILD_MESSAGES, flags).is_ok());
        assert!(check_lifecycle_intents(Intents::DIRECT_MESSAGES, flags).is_err());
        assert!(
            check_lifecycle_intents(
                Intents::DIRECT_MESSAGES,
                flags - EventTypeFlags::MESSAGE_DELETE_BULK
            )
            .is_ok()
        );
        assert!(check_lifecycle_intents(Intents::MESSAGE_CONTENT, flags).is_err());

        let (tx, rx) = mpsc::channel(3);
        for payload in [EMBED_EDIT, DELETE, BULK_DELETE] {
            let event = decode_text(payload.to_owned(), flags).unwrap();
            assert!(forward_lifecycle_event(event, &tx, &hooks.reporter, None).is_none());
        }
        drop(tx);
        dispatch_events(
            rx,
            None::<fn(Context<'_>) -> Ready<Result<()>>>,
            &events,
            &http,
            &permissions,
            &hooks,
        )
        .await;
        assert_eq!(
            *observed.lock().unwrap(),
            [("edit", 1), ("delete", 1), ("bulk", 1), ("bulk", 5)]
        );
    }

    #[tokio::test]
    async fn dispatches_raw_reactions_for_guilds_and_dms_in_receive_order() {
        let mut events = Events::new();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let added = Arc::clone(&observed);
        let removed = Arc::clone(&observed);
        let cleared = Arc::clone(&observed);
        let cleared_emoji = Arc::clone(&observed);
        assert!(events.reaction_types().is_empty());

        events.on_raw_reaction_add(async move |event: &ReactionAdd| -> Result<()> {
            assert_eq!(event.guild_id, Some(Id::new(3)));
            assert_eq!(event.user_id, Id::new(4));
            assert_eq!(event.message_author_id, Some(Id::new(5)));
            assert!(matches!(&event.emoji, EmojiReactionType::Unicode { name } if name == "👍"));
            added.lock().unwrap().push("add");
            Ok(())
        });
        assert_eq!(events.reaction_types(), EventTypeFlags::REACTION_ADD);
        events.on_raw_reaction_remove(async move |event: &ReactionRemove| -> Result<()> {
            assert_eq!(event.guild_id, None);
            assert!(event.burst);
            assert!(matches!(event.emoji, EmojiReactionType::Custom { id, name: None, .. } if id == Id::new(9)));
            removed.lock().unwrap().push("remove");
            Ok(())
        });
        events.on_raw_reaction_clear(async move |event: &ReactionRemoveAll| -> Result<()> {
            assert_eq!(event.guild_id, Some(Id::new(3)));
            assert_eq!(event.message_id, Id::new(1));
            cleared.lock().unwrap().push("clear");
            Ok(())
        });
        events.on_raw_reaction_clear_emoji(async move |event: &ReactionRemoveEmoji| -> Result<()> {
            assert_eq!(event.guild_id, None);
            assert_eq!(event.message_id, Id::new(1));
            assert!(matches!(event.emoji, EmojiReactionType::Custom { id, name: None, .. } if id == Id::new(9)));
            cleared_emoji.lock().unwrap().push("clear emoji");
            Ok(())
        });

        let hooks = Hooks::default();
        let (tx, rx) = mpsc::channel(4);
        for payload in [
            REACTION_ADD,
            REACTION_REMOVE,
            REACTION_CLEAR,
            DM_REACTION_CLEAR_EMOJI,
        ] {
            let event = decode_text(payload.to_owned(), events.reaction_types()).unwrap();
            assert!(forward_lifecycle_event(event, &tx, &hooks.reporter, None).is_none());
        }
        drop(tx);
        dispatch_events(
            rx,
            None::<fn(Context<'_>) -> Ready<Result<()>>>,
            &events,
            &Client::new("test".into()),
            &PermissionResolver::new(None),
            &hooks,
        )
        .await;
        assert_eq!(
            *observed.lock().unwrap(),
            ["add", "remove", "clear", "clear emoji"]
        );
    }

    #[test]
    fn skips_unsubscribed_reaction_events_and_reports_invalid_clear_emoji() {
        for (payload, other_flag) in [
            (REACTION_ADD, EventTypeFlags::REACTION_REMOVE),
            (REACTION_REMOVE, EventTypeFlags::REACTION_ADD),
            (REACTION_CLEAR, EventTypeFlags::REACTION_REMOVE_EMOJI),
            (DM_REACTION_CLEAR_EMOJI, EventTypeFlags::MESSAGE_UPDATE),
        ] {
            assert!(decode_text(payload.to_owned(), other_flag).is_none());
        }
        assert!(matches!(
            decode_text(
                INVALID_REACTION_CLEAR_EMOJI.to_owned(),
                EventTypeFlags::REACTION_REMOVE_EMOJI
            ),
            Some(Err(Error::ReactionRemoveEmojiDecode(_)))
        ));
    }

    #[tokio::test]
    async fn missing_reaction_intents_fail_before_connecting() {
        use crate::check_reaction_intents;

        let flags = EventTypeFlags::REACTION_ADD;
        assert!(check_reaction_intents(Intents::GUILD_MESSAGE_REACTIONS, flags).is_ok());
        assert!(check_reaction_intents(Intents::DIRECT_MESSAGE_REACTIONS, flags).is_ok());

        let mut events = Events::new();
        events.on_raw_reaction_add(async |_: &ReactionAdd| -> Result<()> { Ok(()) });
        let result = Bot::new("test")
            .unwrap()
            .add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)
            .with_events(events)
            .start()
            .await;
        assert!(matches!(result, Err(Error::MissingEventIntents { any_of })
            if any_of == Intents::GUILD_MESSAGE_REACTIONS | Intents::DIRECT_MESSAGE_REACTIONS));
    }
}
