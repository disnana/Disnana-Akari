use super::owner::{message_invoker, message_owner};
use super::{
    ButtonContext, ButtonResponse, ComponentType, Id, Interaction, InteractionData,
    InteractionHandlers, InteractionResponseType, InteractionType, Message,
    MessageComponentInteractionData, ModalContext, ModalInteractionComponent, ModalInteractionData,
    ResponseState, Result, SelectContext, UiMessage, UpdateAction, modal_fallback, submitted_text,
    update_response,
};
use super::{
    ChannelMarker, ChannelSelectContext, RoleMarker, RoleSelectContext, UserMarker,
    UserSelectContext, parse_entity_ids,
};
use super::{InteractionDataResolved, Mentionable, MentionableSelectContext, parse_mentionables};
use crate::Messages;
use crate::ui::{Button, Embed};
use twilight_model::{
    application::interaction::{
        InteractionMetadata,
        modal::{ModalInteractionActionRow, ModalInteractionLabel, ModalInteractionTextInput},
    },
    channel::message::{MessageReference, MessageReferenceType, MessageType},
    guild::{Permissions, Role, RoleColors, RoleFlags},
    oauth::ApplicationIntegrationMap,
    user::User,
    util::Timestamp,
};

async fn click(_: &mut ButtonContext<'_>) -> Result<()> {
    Ok(())
}

async fn select(_: &mut SelectContext<'_>) -> Result<()> {
    Ok(())
}

async fn submit(_: &mut ModalContext<'_>) -> Result<()> {
    Ok(())
}

#[allow(deprecated)]
fn interaction(kind: InteractionType, data: InteractionData) -> Interaction {
    Interaction {
        app_permissions: None,
        application_id: Id::new(10),
        attachment_size_limit: 0,
        authorizing_integration_owners: ApplicationIntegrationMap {
            guild: None,
            user: None,
        },
        channel: None,
        channel_id: None,
        context: None,
        data: Some(data),
        entitlements: vec![],
        guild: None,
        guild_id: None,
        guild_locale: None,
        id: Id::new(1),
        kind,
        locale: None,
        member: None,
        message: None,
        token: String::new(),
        user: None,
    }
}

fn component_payload(component_type: ComponentType, id: &str) -> InteractionData {
    InteractionData::MessageComponent(Box::new(MessageComponentInteractionData {
        component_type,
        custom_id: id.into(),
        resolved: None,
        values: vec![],
    }))
}

#[test]
fn routes_only_matching_kinds_and_exact_ids() -> Result<()> {
    let mut handlers = InteractionHandlers::default();
    handlers.register_select("shared", select)?;
    handlers.register_modal("shared", submit)?;
    assert!(handlers.has_handlers());
    assert!(handlers.register_select("shared", select).is_err());
    assert!(handlers.register_modal("shared", submit).is_err());
    assert!(handlers.register_select("", select).is_err());
    assert!(handlers.register_modal(&"a".repeat(101), submit).is_err());
    let menu = interaction(
        InteractionType::MessageComponent,
        component_payload(ComponentType::TextSelectMenu, "shared"),
    );
    assert!(handlers.accepts(&menu));
    let menu = interaction(
        InteractionType::MessageComponent,
        component_payload(ComponentType::TextSelectMenu, "Shared"),
    );
    assert!(!handlers.accepts(&menu));
    let menu = interaction(
        InteractionType::MessageComponent,
        component_payload(ComponentType::UserSelectMenu, "shared"),
    );
    assert!(!handlers.accepts(&menu));
    let button = interaction(
        InteractionType::MessageComponent,
        component_payload(ComponentType::Button, "shared"),
    );
    assert!(!handlers.accepts(&button));
    handlers.register_button("shared", click)?;
    assert!(handlers.accepts(&button));
    let modal = interaction(
        InteractionType::ModalSubmit,
        InteractionData::ModalSubmit(Box::new(ModalInteractionData {
            components: vec![],
            custom_id: "shared".into(),
            resolved: None,
        })),
    );
    assert!(handlers.accepts(&modal));
    let malformed = interaction(
        InteractionType::ModalSubmit,
        component_payload(ComponentType::TextSelectMenu, "shared"),
    );
    assert!(!handlers.accepts(&malformed));
    Ok(())
}

#[test]
fn entity_selects_only_accept_the_registered_type_and_id() -> Result<()> {
    async fn user_selected(ctx: &mut UserSelectContext<'_>) -> Result<()> {
        let _: &[Id<UserMarker>] = ctx.values();
        Ok(())
    }
    async fn role_selected(ctx: &mut RoleSelectContext<'_>) -> Result<()> {
        let _: &[Id<RoleMarker>] = ctx.values();
        Ok(())
    }
    async fn channel_selected(ctx: &mut ChannelSelectContext<'_>) -> Result<()> {
        let _: &[Id<ChannelMarker>] = ctx.values();
        Ok(())
    }
    let mut handlers = InteractionHandlers::default();
    handlers.register_user_select("user", user_selected)?;
    handlers.register_role_select("role", role_selected)?;
    handlers.register_channel_select("channel", channel_selected)?;
    assert!(handlers.has_handlers());
    assert!(
        handlers
            .register_user_select("user", user_selected)
            .is_err()
    );
    assert!(
        handlers
            .register_role_select("role", role_selected)
            .is_err()
    );
    assert!(
        handlers
            .register_channel_select("channel", channel_selected)
            .is_err()
    );
    for (kind, id, expected) in [
        (ComponentType::UserSelectMenu, "user", true),
        (ComponentType::UserSelectMenu, "role", false),
        (ComponentType::RoleSelectMenu, "role", true),
        (ComponentType::RoleSelectMenu, "Role", false),
        (ComponentType::ChannelSelectMenu, "channel", true),
        (ComponentType::ChannelSelectMenu, "user", false),
        (ComponentType::TextSelectMenu, "channel", false),
        (ComponentType::MentionableSelectMenu, "user", false),
    ] {
        let incoming = interaction(
            InteractionType::MessageComponent,
            component_payload(kind, id),
        );
        assert_eq!(handlers.accepts(&incoming), expected);
    }
    Ok(())
}

#[test]
fn accepts_canonical_unique_snowflakes_and_rejects_entire_invalid_lists() -> Result<()> {
    let values = vec!["123".into(), u64::MAX.to_string()];
    assert_eq!(
        parse_entity_ids::<UserMarker>(&values)?,
        vec![Id::new(123), Id::new(u64::MAX)]
    );
    assert!(parse_entity_ids::<RoleMarker>(&[])?.is_empty());
    for invalid in [
        "",
        "0",
        "01",
        "-1",
        "+1",
        " 1",
        "１",
        "18446744073709551616",
        "123",
    ] {
        let malformed = vec!["123".into(), invalid.into()];
        assert!(parse_entity_ids::<ChannelMarker>(&malformed).is_err());
    }
    let too_many = (1..=26).map(|id| id.to_string()).collect::<Vec<_>>();
    assert!(parse_entity_ids::<UserMarker>(&too_many).is_err());
    Ok(())
}

#[test]
fn mentionable_routes_match_the_kind_and_custom_id() -> Result<()> {
    async fn selected(ctx: &mut MentionableSelectContext<'_>) -> Result<()> {
        let _: &[Mentionable] = ctx.values();
        Ok(())
    }
    let mut handlers = InteractionHandlers::default();
    handlers.register_mentionable_select("target", selected)?;
    assert!(handlers.has_handlers());
    assert!(
        handlers
            .register_mentionable_select("target", selected)
            .is_err()
    );
    assert!(handlers.register_mentionable_select("", selected).is_err());
    for (kind, id, expected) in [
        (ComponentType::MentionableSelectMenu, "target", true),
        (ComponentType::MentionableSelectMenu, "Target", false),
        (ComponentType::UserSelectMenu, "target", false),
        (ComponentType::RoleSelectMenu, "target", false),
    ] {
        let incoming = interaction(
            InteractionType::MessageComponent,
            component_payload(kind, id),
        );
        assert_eq!(handlers.accepts(&incoming), expected);
    }
    Ok(())
}

#[test]
fn mentionable_values_preserve_order_and_require_exactly_one_matching_entity() -> Result<()> {
    let mut resolved = InteractionDataResolved {
        attachments: Default::default(),
        channels: Default::default(),
        members: Default::default(),
        messages: Default::default(),
        roles: [(Id::new(5), role(5))].into(),
        users: [(Id::new(9), user(9, false))].into(),
    };
    let valid = ["9".into(), "5".into()];
    assert_eq!(
        parse_mentionables(&valid, Some(&resolved))?,
        [Mentionable::User(Id::new(9)), Mentionable::Role(Id::new(5))]
    );
    assert!(parse_mentionables(&[], None)?.is_empty());
    assert!(parse_mentionables(&valid, None).is_err());
    for invalid in ["2", "0", "05", "+5", "18446744073709551616", "9"] {
        assert!(parse_mentionables(&["9".into(), invalid.into()], Some(&resolved)).is_err());
    }
    let too_many = (1..=26).map(|value| value.to_string()).collect::<Vec<_>>();
    assert!(parse_mentionables(&too_many, Some(&resolved)).is_err());

    resolved.roles.insert(Id::new(9), role(9));
    assert!(parse_mentionables(&valid, Some(&resolved)).is_err());
    resolved.roles.remove(&Id::new(9));
    resolved.users.insert(Id::new(9), user(8, false));
    assert!(parse_mentionables(&valid, Some(&resolved)).is_err());
    Ok(())
}

#[allow(deprecated)]
fn role(id: u64) -> Role {
    Role {
        color: 0,
        colors: RoleColors {
            primary_color: 0,
            secondary_color: None,
            tertiary_color: None,
        },
        hoist: false,
        icon: None,
        id: Id::new(id),
        managed: false,
        mentionable: true,
        name: "fixture".into(),
        permissions: Permissions::empty(),
        position: 0,
        flags: RoleFlags::empty(),
        tags: None,
        unicode_emoji: None,
    }
}

#[test]
fn finds_text_inputs_in_label_and_legacy_row() {
    let field = |value: &str| {
        ModalInteractionComponent::TextInput(ModalInteractionTextInput {
            custom_id: "description".into(),
            id: 1,
            value: value.into(),
        })
    };
    let label = ModalInteractionComponent::Label(ModalInteractionLabel {
        component: Box::new(field("new")),
        id: 2,
    });
    let legacy = ModalInteractionComponent::ActionRow(ModalInteractionActionRow {
        components: vec![field("legacy")],
        id: 2,
    });
    assert_eq!(submitted_text(&label, "description"), Some("new"));
    assert_eq!(submitted_text(&legacy, "description"), Some("legacy"));
    assert_eq!(submitted_text(&label, "Description"), None);
}

#[test]
fn invoker_comes_from_supported_message_metadata() {
    let mut metadata = InteractionMetadata {
        authorizing_integration_owners: ApplicationIntegrationMap {
            guild: None,
            user: None,
        },
        id: Id::new(1),
        interacted_message_id: None,
        kind: InteractionType::ApplicationCommand,
        original_response_message_id: None,
        target_message_id: None,
        target_user: None,
        triggering_interaction_metadata: None,
        user: user(42, false),
    };
    assert_eq!(message_invoker(&metadata), Some(Id::new(42)));
    metadata.kind = InteractionType::Ping;
    assert_eq!(message_invoker(&metadata), None);
}

#[test]
fn modal_fallback_respects_outcome_and_visibility() {
    let messages = Messages::default();
    let accepted = messages.modal_accepted.as_deref().unwrap();
    let failed = messages.modal_failed.as_deref().unwrap();
    assert_eq!(
        modal_fallback(ResponseState::Pending, true, &messages),
        Some((accepted, true))
    );
    assert_eq!(
        modal_fallback(ResponseState::Pending, false, &messages),
        Some((failed, true))
    );
    for ephemeral in [false, true] {
        let deferred = ResponseState::Deferred { ephemeral };
        assert_eq!(
            modal_fallback(deferred, true, &messages),
            Some((accepted, false))
        );
        assert_eq!(
            modal_fallback(deferred, false, &messages),
            Some((failed, false))
        );
    }
    assert_eq!(
        modal_fallback(ResponseState::Replied, true, &messages),
        None
    );
    assert_eq!(
        modal_fallback(ResponseState::Replied, false, &messages),
        None
    );
}

#[test]
fn modal_fallback_sends_nothing_when_the_message_is_disabled() {
    let mut messages = Messages {
        modal_accepted: None,
        modal_failed: None,
        ..Messages::default()
    };
    for succeeded in [true, false] {
        assert_eq!(
            modal_fallback(ResponseState::Pending, succeeded, &messages),
            None
        );
    }
    messages.modal_accepted = Some("受け取りました".to_owned());
    assert_eq!(
        modal_fallback(ResponseState::Pending, true, &messages),
        Some(("受け取りました", true))
    );
    assert_eq!(
        modal_fallback(ResponseState::Pending, false, &messages),
        None
    );
}

#[test]
fn invoker_from_reply_requires_a_matching_embedded_human_message() {
    let command = message(50, user(42, false));
    let mut response = message(60, user(99, true));
    response.kind = MessageType::Reply;
    response.reference = Some(MessageReference {
        kind: MessageReferenceType::Default,
        channel_id: Some(command.channel_id),
        guild_id: command.guild_id,
        message_id: Some(command.id),
        fail_if_not_exists: None,
    });
    assert_eq!(message_owner(&response), None);

    response.referenced_message = Some(Box::new(command));
    assert_eq!(message_owner(&response), Some(Id::new(42)));

    let reference = response.reference.as_mut().unwrap();
    reference.kind = MessageReferenceType::Forward;
    assert_eq!(message_owner(&response), None);

    let reference = response.reference.as_mut().unwrap();
    reference.kind = MessageReferenceType::Default;
    reference.message_id = Some(Id::new(90));
    assert_eq!(message_owner(&response), None);

    response.reference.as_mut().unwrap().message_id = Some(Id::new(50));
    response.referenced_message.as_mut().unwrap().author.bot = true;
    assert_eq!(message_owner(&response), None);
}

fn user(id: u64, bot: bool) -> User {
    User {
        accent_color: None,
        avatar: None,
        avatar_decoration: None,
        avatar_decoration_data: None,
        banner: None,
        bot,
        discriminator: 0,
        email: None,
        flags: None,
        global_name: None,
        id: Id::new(id),
        locale: None,
        mfa_enabled: None,
        name: "fixture".into(),
        premium_type: None,
        primary_guild: None,
        public_flags: None,
        system: None,
        verified: None,
    }
}

#[allow(deprecated)]
fn message(id: u64, author: User) -> Message {
    Message {
        activity: None,
        application: None,
        application_id: None,
        attachments: vec![],
        author,
        call: None,
        channel_id: Id::new(3),
        components: vec![],
        content: String::new(),
        edited_timestamp: None,
        embeds: vec![],
        flags: None,
        guild_id: Some(Id::new(4)),
        id: Id::new(id),
        interaction: None,
        interaction_metadata: None,
        kind: MessageType::Regular,
        member: None,
        mention_channels: vec![],
        mention_everyone: false,
        mention_roles: vec![],
        mentions: vec![],
        message_snapshots: vec![],
        pinned: false,
        poll: None,
        reactions: vec![],
        reference: None,
        referenced_message: None,
        role_subscription_data: None,
        sticker_items: vec![],
        timestamp: Timestamp::from_micros(1_580_608_922_020_000).unwrap(),
        thread: None,
        tts: false,
        webhook_id: None,
    }
}

#[test]
fn rejects_duplicate_and_invalid_button_ids() -> Result<()> {
    let mut buttons = InteractionHandlers::default();
    assert!(!buttons.has_handlers());
    assert!(buttons.register_button("", click).is_err());
    assert!(buttons.register_button(&"a".repeat(101), click).is_err());
    buttons.register_button("akari:ui:ping", click)?;
    assert!(buttons.has_handlers());
    assert!(buttons.register_button("akari:ui:ping", click).is_err());
    Ok(())
}

#[test]
fn selects_an_update_path_only_before_reply_or_after_deferring() -> Result<()> {
    assert_eq!(
        ButtonResponse::Pending.update_action()?,
        UpdateAction::Initial
    );
    assert_eq!(
        ButtonResponse::DeferredUpdate.update_action()?,
        UpdateAction::Edit
    );
    assert!(ButtonResponse::DeferredUpdate.require_pending().is_err());
    assert!(ButtonResponse::Sent.require_pending().is_err());
    assert!(ButtonResponse::Sent.update_action().is_err());
    Ok(())
}

#[test]
fn update_payload_replaces_all_three_fields_and_suppresses_mentions() {
    let message = UiMessage::new().embed(Embed::new("新しいタイトル"));
    let response = update_response(&message);
    assert_eq!(response.kind, InteractionResponseType::UpdateMessage);
    let data = response.data.unwrap();
    assert_eq!(data.content.as_deref(), Some(""));
    assert_eq!(data.embeds.unwrap().len(), 1);
    assert!(data.components.unwrap().is_empty());
    let mentions = data.allowed_mentions.unwrap();
    assert!(mentions.parse.is_empty());
    assert!(!mentions.replied_user);

    let buttons_only = UiMessage::new().buttons(vec![Button::primary("refresh", "更新")]);
    let data = update_response(&buttons_only).data.unwrap();
    assert!(data.embeds.unwrap().is_empty());
    assert_eq!(data.components.unwrap().len(), 1);
}
