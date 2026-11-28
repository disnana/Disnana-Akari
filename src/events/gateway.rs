use std::{collections::HashMap, fmt};

use twilight_gateway::{Event, EventTypeFlags, Intents};
use twilight_model::gateway::{event::EventType, payload::incoming};

use super::{Events, Handler, Reporter, Result, adapt_handler, dispatch};
use crate::Error;

mod sealed {
    use super::{Event, EventType, Intents};

    pub trait Payload: Sized + 'static {
        const KIND: EventType;
        const REQUIRED_ANY: Intents;

        fn from_event(event: &Event) -> Option<&Self>;
    }
}

/// [`Events::on_gateway`] で受け取れる Twilight の型付きペイロードです。
///
/// 対応している型はこのトレイトの実装一覧を参照してください。
pub trait GatewayPayload: sealed::Payload {}

macro_rules! payloads {
    ($($intents:expr => [$($name:ident),+ $(,)?]),+ $(,)?) => {
        $(
            $(
                impl GatewayPayload for incoming::$name {}

                impl sealed::Payload for incoming::$name {
                    const KIND: EventType = EventType::$name;
                    const REQUIRED_ANY: Intents = $intents;

                    fn from_event(event: &Event) -> Option<&Self> {
                        match event {
                            Event::$name(payload) => Some(payload),
                            _ => None,
                        }
                    }
                }
            )+
        )+
    };
}

payloads! {
    Intents::GUILDS => [
        GuildCreate, GuildUpdate, GuildDelete,
        ChannelCreate, ChannelUpdate, ChannelDelete,
        RoleCreate, RoleUpdate, RoleDelete,
        StageInstanceCreate, StageInstanceUpdate, StageInstanceDelete,
        ThreadCreate, ThreadUpdate, ThreadDelete, ThreadListSync, ThreadMemberUpdate,
    ],
    Intents::GUILDS.union(Intents::DIRECT_MESSAGES) => [ChannelPinsUpdate],
    Intents::GUILD_MEMBERS => [MemberAdd, MemberRemove],
    Intents::GUILD_MODERATION => [BanAdd, BanRemove, GuildAuditLogEntryCreate],
    Intents::GUILD_EMOJIS_AND_STICKERS => [GuildEmojisUpdate, GuildStickersUpdate],
    Intents::GUILD_INTEGRATIONS => [
        GuildIntegrationsUpdate, IntegrationCreate, IntegrationUpdate, IntegrationDelete,
    ],
    Intents::GUILD_WEBHOOKS => [WebhooksUpdate],
    Intents::GUILD_INVITES => [InviteCreate, InviteDelete],
    Intents::GUILD_SCHEDULED_EVENTS => [
        GuildScheduledEventCreate, GuildScheduledEventUpdate, GuildScheduledEventDelete,
        GuildScheduledEventUserAdd, GuildScheduledEventUserRemove,
    ],
    Intents::AUTO_MODERATION_CONFIGURATION => [
        AutoModerationRuleCreate, AutoModerationRuleUpdate, AutoModerationRuleDelete,
    ],
    Intents::AUTO_MODERATION_EXECUTION => [AutoModerationActionExecution],
    Intents::empty() => [
        MemberUpdate, UserUpdate, CommandPermissionsUpdate, VoiceServerUpdate,
        EntitlementCreate, EntitlementUpdate, EntitlementDelete,
        SubscriptionCreate, SubscriptionUpdate, SubscriptionDelete,
    ],
    Intents::GUILDS.union(Intents::GUILD_MEMBERS) => [ThreadMembersUpdate],
    Intents::GUILD_PRESENCES => [PresenceUpdate],
    Intents::GUILD_VOICE_STATES => [VoiceStateUpdate],
}

pub(super) struct GatewayHandlers {
    types: EventTypeFlags,
    by_kind: HashMap<EventType, GatewaySubscription>,
}

struct GatewaySubscription {
    required_any: Intents,
    handlers: Vec<Box<dyn Handler<Event>>>,
}

impl Default for GatewayHandlers {
    fn default() -> Self {
        Self {
            types: EventTypeFlags::empty(),
            by_kind: HashMap::new(),
        }
    }
}

impl fmt::Debug for GatewayHandlers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GatewayHandlers")
            .field("types", &self.types)
            .finish()
    }
}

impl Events {
    /// 指定した Gateway ペイロードを受け取る関数を登録します。
    ///
    /// ハンドラーには Discord が送ったイベントをそのまま渡します。
    /// `GuildCreate` は初期同期と一時障害からの復旧でも届きます。
    /// `GuildDelete` の `unavailable` が `Some(true)` の場合は一時的な利用不能です。
    ///
    /// サーバー、チャンネル、ロール、スレッドの通知には `GUILDS` を指定します。
    /// DM の `ChannelPinsUpdate` には `DIRECT_MESSAGES` を指定します。
    /// `MemberAdd`・`MemberRemove` には特権 Intent の `GUILD_MEMBERS` が必要です。
    /// `MemberUpdate`・`ThreadMembersUpdate` を他のメンバーについて受け取る場合も
    /// `GUILD_MEMBERS` を指定します。省略すると Bot 自身に関する通知だけが届きます。
    /// ステータスの通知には特権 Intent の `GUILD_PRESENCES`、音声状態には
    /// `GUILD_VOICE_STATES` を指定します。音声接続先を通知する `VoiceServerUpdate` 自体に
    /// 追加の Intent は必要ありません。含まれる音声トークンはログに出さないでください。
    ///
    /// ステージには `GUILDS`、予定イベントと参加通知には `GUILD_SCHEDULED_EVENTS` を指定します。
    /// BAN・監査ログには `GUILD_MODERATION`、絵文字・スタンプには Twilight の
    /// `GUILD_EMOJIS_AND_STICKERS`（Discord の `GUILD_EXPRESSIONS`）を指定します。
    /// 連携、Webhook、招待にはそれぞれ `GUILD_INTEGRATIONS`、`GUILD_WEBHOOKS`、
    /// `GUILD_INVITES` を指定します。Twilight の招待ペイロードはサーバー ID を持ちます。
    /// AutoMod はルール編集に `AUTO_MODERATION_CONFIGURATION`、アクション実行に
    /// `AUTO_MODERATION_EXECUTION` を使います。ルール・実行通知の配信には `MANAGE_GUILD`、
    /// 実行時の本文には `MESSAGE_CONTENT` も必要です。
    /// 監査ログには `VIEW_AUDIT_LOG`、BAN 通知には `BAN_MEMBERS` または `VIEW_AUDIT_LOG`、
    /// 招待通知にはチャンネルの `MANAGE_CHANNELS` 権限が必要です。
    /// Bot 自身の `UserUpdate`、`CommandPermissionsUpdate`、Entitlement・Subscription 通知には
    /// 追加の Intent は必要ありません。購入の特典付与には Entitlement を確認してください。
    ///
    /// ```
    /// use disnana_akari::{Events, Result, model::gateway::payload::incoming::GuildDelete};
    ///
    /// let mut events = Events::new();
    /// events.on_gateway(async |event: &GuildDelete| -> Result<()> {
    ///     let _guild_id = event.id;
    ///     let _temporarily_unavailable = event.unavailable == Some(true);
    ///     Ok(())
    /// });
    /// ```
    pub fn on_gateway<P, E>(
        &mut self,
        handler: impl AsyncFn(&P) -> std::result::Result<(), E> + Send + Sync + 'static,
    ) where
        P: GatewayPayload,
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let handler = adapt_handler!(handler, payload: &P);
        let erased = async move |event: &Event| -> Result<()> {
            if let Some(payload) = P::from_event(event) {
                handler(payload).await?;
            }
            Ok(())
        };

        self.gateway.types.insert(P::KIND.into());
        self.gateway
            .by_kind
            .entry(P::KIND)
            .or_insert_with(|| GatewaySubscription {
                required_any: P::REQUIRED_ANY,
                handlers: Vec::new(),
            })
            .handlers
            .push(Box::new(erased));
    }
}

impl GatewayHandlers {
    pub(super) fn types(&self, intents: Intents) -> Result<EventTypeFlags> {
        if let Some(any_of) = self
            .by_kind
            .values()
            .map(|subscription| subscription.required_any)
            .filter(|any_of| !any_of.is_empty() && !intents.intersects(*any_of))
            .min_by_key(|any_of| any_of.bits())
        {
            return Err(Error::MissingEventIntents { any_of });
        }
        Ok(self.types)
    }

    pub(super) fn event_name(&self, event: &Event) -> Option<&'static str> {
        if self.types.is_empty() {
            return None;
        }

        let kind = event.kind();
        if self.types.intersects(kind.into()) {
            return kind.name();
        }
        None
    }

    pub(super) async fn dispatch(&self, event: &Event, reporter: &Reporter) {
        let kind = event.kind();
        if let Some(subscription) = self.by_kind.get(&kind)
            && let Some(name) = kind.name()
        {
            dispatch(&subscription.handlers, event, name, reporter).await;
        }
    }
}

#[cfg(test)]
mod notification_tests;

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::sync::mpsc;
    use twilight_gateway::parse;
    use twilight_model::gateway::presence::Status;

    use super::*;
    use crate::{
        Client, Error, NoMessageHandler, dispatch_events, handler_event_types, hooks::Hooks,
        message_events::decode_text, permissions::PermissionResolver, route_gateway_event,
    };

    const GUILD_UNAVAILABLE: &str =
        r#"{"op":0,"s":1,"t":"GUILD_DELETE","d":{"id":"7","unavailable":true}}"#;
    const CHANNEL_UPDATE: &str =
        r#"{"op":0,"s":2,"t":"CHANNEL_UPDATE","d":{"id":"8","guild_id":"7","type":0}}"#;
    const ROLE_DELETE: &str =
        r#"{"op":0,"s":3,"t":"GUILD_ROLE_DELETE","d":{"guild_id":"7","role_id":"9"}}"#;
    const SELF_MEMBER_UPDATE: &str = r#"{"op":0,"s":4,"t":"GUILD_MEMBER_UPDATE","d":{"guild_id":"7","roles":[],"user":{"id":"6","username":"bot","discriminator":"0","bot":true}}}"#;
    const THREAD_MEMBERS_UPDATE: &str = r#"{"op":0,"s":5,"t":"THREAD_MEMBERS_UPDATE","d":{"guild_id":"7","id":"8","member_count":1,"removed_member_ids":["6"]}}"#;
    const PRESENCE_UPDATE: &str = r#"{"op":0,"s":6,"t":"PRESENCE_UPDATE","d":{"guild_id":"7","user":{"id":"6"},"status":"idle","client_status":{}}}"#;
    const VOICE_DISCONNECT: &str = r#"{"op":0,"s":7,"t":"VOICE_STATE_UPDATE","d":{"guild_id":"7","channel_id":null,"user_id":"6","session_id":"session","deaf":false,"mute":false,"self_deaf":false,"self_mute":false,"self_video":false,"suppress":false}}"#;
    const DM_PINS_UPDATE: &str = r#"{"op":0,"s":8,"t":"CHANNEL_PINS_UPDATE","d":{"channel_id":"8","last_pin_timestamp":null}}"#;

    #[test]
    fn validates_guild_intent_and_sets_only_registered_event_flags() {
        let mut events = Events::new();
        assert!(
            handler_event_types(&events, Intents::empty())
                .unwrap()
                .is_empty()
        );
        events.on_gateway(async |_: &incoming::GuildDelete| -> Result<()> { Ok(()) });
        assert!(matches!(
            handler_event_types(&events, Intents::GUILD_MEMBERS),
            Err(Error::MissingEventIntents { any_of }) if any_of == Intents::GUILDS
        ));
        let flags = handler_event_types(&events, Intents::GUILDS).unwrap();
        assert_eq!(flags, EventTypeFlags::GUILD_DELETE);
        assert!(parse(CHANNEL_UPDATE.to_owned(), flags).unwrap().is_none());
        assert!(parse(ROLE_DELETE.to_owned(), flags).unwrap().is_none());
    }

    #[test]
    fn validates_each_subscription_and_documented_intent_exceptions() {
        let mut events = Events::new();
        events.on_gateway(async |_: &incoming::MemberUpdate| -> Result<()> { Ok(()) });
        let flags = handler_event_types(&events, Intents::empty()).unwrap();
        assert_eq!(flags, EventTypeFlags::MEMBER_UPDATE);
        events.on_gateway(async |_: &incoming::ThreadMembersUpdate| -> Result<()> { Ok(()) });
        events.on_gateway(async |_: &incoming::ChannelPinsUpdate| -> Result<()> { Ok(()) });

        let expected =
            flags | EventTypeFlags::THREAD_MEMBERS_UPDATE | EventTypeFlags::CHANNEL_PINS_UPDATE;
        assert_eq!(
            handler_event_types(&events, Intents::GUILDS).unwrap(),
            expected
        );
        assert_eq!(
            handler_event_types(&events, Intents::GUILD_MEMBERS | Intents::DIRECT_MESSAGES)
                .unwrap(),
            expected
        );
        assert!(matches!(
            handler_event_types(&events, Intents::DIRECT_MESSAGES),
            Err(Error::MissingEventIntents { any_of })
                if any_of == Intents::GUILD_MEMBERS | Intents::GUILDS
        ));
        assert!(matches!(
            handler_event_types(&events, Intents::GUILD_MEMBERS),
            Err(Error::MissingEventIntents { any_of })
                if any_of == Intents::DIRECT_MESSAGES | Intents::GUILDS
        ));

        events.on_gateway(async |_: &incoming::MemberAdd| -> Result<()> { Ok(()) });
        events.on_gateway(async |_: &incoming::MemberRemove| -> Result<()> { Ok(()) });
        events.on_gateway(async |_: &incoming::PresenceUpdate| -> Result<()> { Ok(()) });
        events.on_gateway(async |_: &incoming::VoiceStateUpdate| -> Result<()> { Ok(()) });
        assert!(matches!(
            handler_event_types(&events, Intents::GUILDS),
            Err(Error::MissingEventIntents { any_of }) if any_of == Intents::GUILD_MEMBERS
        ));
        let member_intents = Intents::GUILDS | Intents::GUILD_MEMBERS;
        assert!(matches!(
            handler_event_types(&events, member_intents | Intents::GUILD_VOICE_STATES),
            Err(Error::MissingEventIntents { any_of }) if any_of == Intents::GUILD_PRESENCES
        ));
        assert!(matches!(
            handler_event_types(&events, member_intents | Intents::GUILD_PRESENCES),
            Err(Error::MissingEventIntents { any_of }) if any_of == Intents::GUILD_VOICE_STATES
        ));
        assert_eq!(
            handler_event_types(
                &events,
                member_intents | Intents::GUILD_PRESENCES | Intents::GUILD_VOICE_STATES
            )
            .unwrap(),
            expected
                | EventTypeFlags::MEMBER_ADD
                | EventTypeFlags::MEMBER_REMOVE
                | EventTypeFlags::PRESENCE_UPDATE
                | EventTypeFlags::VOICE_STATE_UPDATE
        );
    }

    #[tokio::test]
    async fn dispatches_borrowed_payloads_in_receive_and_registration_order() {
        let mut events = Events::new();
        let received = Arc::new(Mutex::new(Vec::new()));
        let guild_first = Arc::clone(&received);
        let guild_second = Arc::clone(&received);
        let channel = Arc::clone(&received);
        let role = Arc::clone(&received);
        events.on_gateway(async move |event: &incoming::GuildDelete| -> Result<()> {
            assert_eq!(event.unavailable, Some(true));
            guild_first
                .lock()
                .unwrap()
                .push(("guild first", event.id.get()));
            Ok(())
        });
        events.on_gateway(async move |event: &incoming::GuildDelete| -> Result<()> {
            guild_second
                .lock()
                .unwrap()
                .push(("guild second", event.id.get()));
            Ok(())
        });
        events.on_gateway(async move |event: &incoming::ChannelUpdate| -> Result<()> {
            channel.lock().unwrap().push(("channel", event.id.get()));
            Ok(())
        });
        events.on_gateway(async move |event: &incoming::RoleDelete| -> Result<()> {
            role.lock().unwrap().push(("role", event.role_id.get()));
            Ok(())
        });

        dispatch_payloads(
            &events,
            Intents::GUILDS,
            &[GUILD_UNAVAILABLE, CHANNEL_UPDATE, ROLE_DELETE],
        )
        .await;

        assert_eq!(
            *received.lock().unwrap(),
            [
                ("guild first", 7),
                ("guild second", 7),
                ("channel", 8),
                ("role", 9)
            ]
        );
    }

    #[tokio::test]
    async fn dispatches_self_updates_dm_pins_and_optional_voice_payload_fields() {
        let mut events = Events::new();
        let received = Arc::new(Mutex::new(Vec::new()));
        let member = Arc::clone(&received);
        let thread = Arc::clone(&received);
        let presence = Arc::clone(&received);
        let voice = Arc::clone(&received);
        let pins = Arc::clone(&received);
        events.on_gateway(async move |event: &incoming::MemberUpdate| -> Result<()> {
            assert!(event.user.bot);
            member.lock().unwrap().push(("member", event.user.id.get()));
            Ok(())
        });
        events.on_gateway(
            async move |event: &incoming::ThreadMembersUpdate| -> Result<()> {
                assert_eq!(event.removed_member_ids.len(), 1);
                thread.lock().unwrap().push(("thread", event.id.get()));
                Ok(())
            },
        );
        events.on_gateway(
            async move |event: &incoming::PresenceUpdate| -> Result<()> {
                assert_eq!(event.status, Status::Idle);
                presence
                    .lock()
                    .unwrap()
                    .push(("presence", event.user.id().get()));
                Ok(())
            },
        );
        events.on_gateway(
            async move |event: &incoming::VoiceStateUpdate| -> Result<()> {
                assert_eq!(event.channel_id, None);
                assert_eq!(event.member, None);
                voice.lock().unwrap().push(("voice", event.user_id.get()));
                Ok(())
            },
        );
        events.on_gateway(
            async move |event: &incoming::ChannelPinsUpdate| -> Result<()> {
                assert_eq!(event.guild_id, None);
                assert_eq!(event.last_pin_timestamp, None);
                pins.lock().unwrap().push(("pins", event.channel_id.get()));
                Ok(())
            },
        );
        let intents = Intents::GUILDS
            | Intents::DIRECT_MESSAGES
            | Intents::GUILD_PRESENCES
            | Intents::GUILD_VOICE_STATES;
        dispatch_payloads(
            &events,
            intents,
            &[
                SELF_MEMBER_UPDATE,
                THREAD_MEMBERS_UPDATE,
                PRESENCE_UPDATE,
                VOICE_DISCONNECT,
                DM_PINS_UPDATE,
            ],
        )
        .await;
        assert_eq!(
            *received.lock().unwrap(),
            [
                ("member", 6),
                ("thread", 8),
                ("presence", 6),
                ("voice", 6),
                ("pins", 8)
            ]
        );
    }

    pub(super) async fn dispatch_payloads(events: &Events, intents: Intents, payloads: &[&str]) {
        let resolver = PermissionResolver::new(None);
        let hooks = Hooks::default();
        let (tx, rx) = mpsc::channel(payloads.len());
        let flags = handler_event_types(events, intents).unwrap();
        for &payload in payloads {
            let event = decode_text(payload.to_owned(), flags, false).unwrap();
            assert!(event.is_ok(), "invalid Gateway fixture: {payload}");
            assert!(
                route_gateway_event(
                    event,
                    events,
                    &resolver,
                    &tx,
                    &hooks.reporter,
                    twilight_gateway::ShardId::ONE,
                    (None, None, None),
                )
                .is_none()
            );
        }
        drop(tx);
        dispatch_events(
            rx,
            None::<NoMessageHandler>,
            events,
            &Client::new("test".into()),
            &resolver,
            &hooks,
        )
        .await;
    }
}
