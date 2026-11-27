use std::sync::{Arc, Mutex};

use super::{Events, GatewayPayload, Intents, Result, incoming, tests::dispatch_payloads};
use crate::{Error, EventTypeFlags, handler_event_types};

const REDACTED_AUTOMOD: &str = r#"{"op":0,"s":2,"t":"AUTO_MODERATION_ACTION_EXECUTION","d":{"action":{"type":1},"guild_id":"7","rule_id":"102","rule_trigger_type":1,"user_id":"9","matched_keyword":null}}"#;

fn assert_subscription<P: GatewayPayload>(intent: Intents, flag: EventTypeFlags) {
    let mut events = Events::new();
    events.on_gateway(async |_: &P| -> Result<()> { Ok(()) });
    assert_eq!(handler_event_types(&events, intent).unwrap(), flag);
    if !intent.is_empty() {
        let unrelated = Intents::all() - intent;
        assert!(matches!(
            handler_event_types(&events, unrelated),
            Err(Error::MissingEventIntents { any_of }) if any_of == intent
        ));
    }
}

#[test]
fn notification_subscriptions_require_their_own_intent_and_flag() {
    macro_rules! subscriptions {
        ($($intent:expr => [$($payload:ident => $flag:ident),+ $(,)?]),+ $(,)?) => {
            $($(assert_subscription::<incoming::$payload>($intent, EventTypeFlags::$flag);)+)+
        };
    }

    subscriptions! {
        Intents::GUILD_MODERATION => [
            BanAdd => BAN_ADD, BanRemove => BAN_REMOVE,
            GuildAuditLogEntryCreate => GUILD_AUDIT_LOG_ENTRY_CREATE,
        ],
        Intents::AUTO_MODERATION_CONFIGURATION => [
            AutoModerationRuleCreate => AUTO_MODERATION_RULE_CREATE,
            AutoModerationRuleUpdate => AUTO_MODERATION_RULE_UPDATE,
            AutoModerationRuleDelete => AUTO_MODERATION_RULE_DELETE,
        ],
        Intents::AUTO_MODERATION_EXECUTION => [
            AutoModerationActionExecution => AUTO_MODERATION_ACTION_EXECUTION,
        ],
        Intents::GUILD_EMOJIS_AND_STICKERS => [
            GuildEmojisUpdate => GUILD_EMOJIS_UPDATE, GuildStickersUpdate => GUILD_STICKERS_UPDATE,
        ],
        Intents::GUILD_INTEGRATIONS => [
            GuildIntegrationsUpdate => GUILD_INTEGRATIONS_UPDATE,
            IntegrationCreate => INTEGRATION_CREATE, IntegrationUpdate => INTEGRATION_UPDATE,
            IntegrationDelete => INTEGRATION_DELETE,
        ],
        Intents::GUILD_WEBHOOKS => [WebhooksUpdate => WEBHOOKS_UPDATE],
        Intents::GUILD_INVITES => [InviteCreate => INVITE_CREATE, InviteDelete => INVITE_DELETE],
        Intents::GUILDS => [
            StageInstanceCreate => STAGE_INSTANCE_CREATE, StageInstanceUpdate => STAGE_INSTANCE_UPDATE,
            StageInstanceDelete => STAGE_INSTANCE_DELETE,
        ],
        Intents::GUILD_SCHEDULED_EVENTS => [
            GuildScheduledEventCreate => GUILD_SCHEDULED_EVENT_CREATE,
            GuildScheduledEventUpdate => GUILD_SCHEDULED_EVENT_UPDATE,
            GuildScheduledEventDelete => GUILD_SCHEDULED_EVENT_DELETE,
            GuildScheduledEventUserAdd => GUILD_SCHEDULED_EVENT_USER_ADD,
            GuildScheduledEventUserRemove => GUILD_SCHEDULED_EVENT_USER_REMOVE,
        ],
        Intents::empty() => [
            UserUpdate => USER_UPDATE, CommandPermissionsUpdate => COMMAND_PERMISSIONS_UPDATE,
            EntitlementCreate => ENTITLEMENT_CREATE, EntitlementUpdate => ENTITLEMENT_UPDATE,
            EntitlementDelete => ENTITLEMENT_DELETE, SubscriptionCreate => SUBSCRIPTION_CREATE,
            SubscriptionUpdate => SUBSCRIPTION_UPDATE, SubscriptionDelete => SUBSCRIPTION_DELETE,
        ],
    }
}

#[test]
fn redacted_automod_keeps_dispatch_validation() {
    use crate::message_events::decode_text;

    let flags = EventTypeFlags::AUTO_MODERATION_ACTION_EXECUTION;
    assert!(decode_text(REDACTED_AUTOMOD.to_owned(), EventTypeFlags::empty(), false).is_none());
    let missing_sequence = REDACTED_AUTOMOD.replace(r#""s":2,"#, "");
    assert!(matches!(
        decode_text(missing_sequence, flags, false),
        Some(Err(_))
    ));
    let invalid_content = REDACTED_AUTOMOD.replace(r#""d":{"#, r#""d":{"content":1,"#);
    assert!(matches!(
        decode_text(invalid_content, flags, false),
        Some(Err(_))
    ));
}

#[tokio::test]
async fn dispatches_admin_and_scheduled_notifications_with_typed_ids() {
    let mut events = Events::new();
    let received = Arc::new(Mutex::new(Vec::new()));
    let audit = Arc::clone(&received);
    let action = Arc::clone(&received);
    let stage = Arc::clone(&received);
    let scheduled = Arc::clone(&received);
    events.on_gateway(
        async move |event: &incoming::GuildAuditLogEntryCreate| -> Result<()> {
            assert_eq!(event.guild_id.map(|id| id.get()), Some(7));
            assert!(event.target_id.is_none());
            audit.lock().unwrap().push(event.id.get());
            Ok(())
        },
    );
    events.on_gateway(
        async move |event: &incoming::AutoModerationActionExecution| -> Result<()> {
            assert!(event.content.is_empty());
            assert!(event.message_id.is_none());
            assert!(event.matched_content.is_none());
            action.lock().unwrap().push(event.rule_id.get());
            Ok(())
        },
    );
    events.on_gateway(
        async move |event: &incoming::StageInstanceCreate| -> Result<()> {
            assert_eq!(event.channel_id.get(), 8);
            stage.lock().unwrap().push(event.id.get());
            Ok(())
        },
    );
    events.on_gateway(
        async move |event: &incoming::GuildScheduledEventUserAdd| -> Result<()> {
            assert_eq!(event.user_id.get(), 9);
            scheduled
                .lock()
                .unwrap()
                .push(event.guild_scheduled_event_id.get());
            Ok(())
        },
    );
    dispatch_payloads(
        &events,
        Intents::GUILDS | Intents::GUILD_MODERATION | Intents::AUTO_MODERATION_EXECUTION | Intents::GUILD_SCHEDULED_EVENTS,
        &[
            r#"{"op":0,"s":1,"t":"GUILD_AUDIT_LOG_ENTRY_CREATE","d":{"action_type":1,"guild_id":"7","id":"101","target_id":null,"user_id":"9"}}"#,
            REDACTED_AUTOMOD,
            r#"{"op":0,"s":3,"t":"STAGE_INSTANCE_CREATE","d":{"guild_id":"7","channel_id":"8","id":"103","privacy_level":2,"topic":"Akari"}}"#,
            r#"{"op":0,"s":4,"t":"GUILD_SCHEDULED_EVENT_USER_ADD","d":{"guild_id":"7","guild_scheduled_event_id":"104","user_id":"9"}}"#,
        ],
    )
    .await;
    assert_eq!(*received.lock().unwrap(), [101, 102, 103, 104]);
}

#[tokio::test]
async fn dispatches_application_notifications_without_gateway_intents() {
    let mut events = Events::new();
    let received = Arc::new(Mutex::new(Vec::new()));
    let command = Arc::clone(&received);
    let entitlement = Arc::clone(&received);
    let subscription = Arc::clone(&received);
    events.on_gateway(
        async move |event: &incoming::CommandPermissionsUpdate| -> Result<()> {
            assert!(event.permissions.is_empty());
            command.lock().unwrap().push(event.id.get());
            Ok(())
        },
    );
    events.on_gateway(
        async move |event: &incoming::EntitlementCreate| -> Result<()> {
            assert_eq!(event.guild_id.map(|id| id.get()), Some(7));
            assert!(event.user_id.is_none());
            entitlement.lock().unwrap().push(event.id.get());
            Ok(())
        },
    );
    events.on_gateway(
        async move |event: &incoming::SubscriptionUpdate| -> Result<()> {
            assert_eq!(event.user_id.get(), 9);
            assert_eq!(event.entitlement_ids.first().map(|id| id.get()), Some(102));
            subscription.lock().unwrap().push(event.id.get());
            Ok(())
        },
    );
    dispatch_payloads(
        &events,
        Intents::empty(),
        &[
            r#"{"op":0,"s":1,"t":"APPLICATION_COMMAND_PERMISSIONS_UPDATE","d":{"application_id":"2","guild_id":"7","id":"101","permissions":[]}}"#,
            r#"{"op":0,"s":2,"t":"ENTITLEMENT_CREATE","d":{"application_id":"2","deleted":false,"guild_id":"7","id":"102","type":8,"sku_id":"3"}}"#,
            r#"{"op":0,"s":3,"t":"SUBSCRIPTION_UPDATE","d":{"id":"103","user_id":"9","sku_ids":["3"],"entitlement_ids":["102"],"current_period_start":"2026-01-01T00:00:00+00:00","current_period_end":"2026-02-01T00:00:00+00:00","status":0}}"#,
        ],
    )
    .await;
    assert_eq!(*received.lock().unwrap(), [101, 102, 103]);
}
