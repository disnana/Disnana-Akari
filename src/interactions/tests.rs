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

#[allow(
    deprecated,
    reason = "テスト用の値を組み立てるために、非推奨のフィールドを埋める。"
)]
pub(super) fn interaction(kind: InteractionType, data: InteractionData) -> Interaction {
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

#[tokio::test]
async fn button_context_debug_omits_credentials() {
    let bot_token = "dummy-bot-credential";
    let interaction_token = "dummy-interaction-credential";
    let client = twilight_http::Client::new(bot_token.to_owned());
    let mut event = interaction(
        InteractionType::MessageComponent,
        component_payload(ComponentType::Button, "akari:test"),
    );
    event.token = interaction_token.to_owned();
    let (_, timing) = crate::timing::Pending::new((), None).begin();
    let info = crate::hooks::InfoReporter::default();
    let initial = super::InitialResponder {
        http: &client,
        interaction: &event,
        timing,
        info: &info,
    };
    let ctx = ButtonContext::new(initial, "akari:test", None);
    let debug = format!("{ctx:?}");
    assert!(debug.contains("akari:test"));
    assert!(!debug.contains(bot_token));
    assert!(!debug.contains(interaction_token));
}

#[tokio::test]
async fn initial_response_reports_the_failed_attempt_and_successful_retry() -> Result<()> {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };
    use tokio::{
        io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
        net::TcpListener,
        time::timeout,
    };

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(crate::Error::handler)?;
    let client = twilight_http::Client::builder()
        .proxy(
            listener
                .local_addr()
                .map_err(crate::Error::handler)?
                .to_string(),
            true,
        )
        .build();
    let mut event = interaction(
        InteractionType::MessageComponent,
        component_payload(ComponentType::Button, "akari:test"),
    );
    event.token = "dummy-interaction-credential".to_owned();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let info = crate::hooks::InfoReporter::new(move |event| sink.lock().unwrap().push(*event));
    let (_, timing) = crate::timing::Pending::new((), None).begin();
    let initial = super::InitialResponder {
        http: &client,
        interaction: &event,
        timing,
        info: &info,
    };
    let mut ctx = ButtonContext::new(initial, "akari:test", None);

    let request = async {
        assert!(ctx.defer_update().await.is_err());
        assert!(matches!(
            seen.lock().unwrap().as_slice(),
            [crate::InfoEvent::InteractionResponseFailed {
                interaction_id,
                command_id: None,
                http_status: Some(400),
                discord_code: Some(50035),
                ..
            }] if *interaction_id == event.id
        ));
        ctx.defer_update().await
    };
    let server = async {
        let body = r#"{"code":50035,"message":"mock rejection"}"#;
        let failure = format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        for response in [
            failure.as_str(),
            "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n",
        ] {
            let (stream, _) = listener.accept().await?;
            let mut stream = BufReader::new(stream);
            let mut header = String::new();
            let mut content_length = 0;
            loop {
                header.clear();
                if stream.read_line(&mut header).await? == 0 || header == "\r\n" {
                    break;
                }
                if let Some((name, value)) = header.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    content_length = value.trim().parse().unwrap();
                }
            }
            stream.read_exact(&mut vec![0; content_length]).await?;
            stream.get_mut().write_all(response.as_bytes()).await?;
        }
        Ok::<_, std::io::Error>(())
    };
    let (request, server) = timeout(Duration::from_secs(5), async {
        tokio::join!(request, server)
    })
    .await
    .map_err(crate::Error::handler)?;
    server.map_err(crate::Error::handler)?;
    request?;
    assert_eq!(ctx.response, ButtonResponse::DeferredUpdate);
    assert!(matches!(
        seen.lock().unwrap().as_slice(),
        [crate::InfoEvent::InteractionResponseFailed { .. }, crate::InfoEvent::InteractionResponded {
            interaction_type: InteractionType::MessageComponent,
            response_type: InteractionResponseType::DeferredUpdateMessage,
            elapsed,
            rest_elapsed,
        }] if elapsed >= rest_elapsed
    ));
    Ok(())
}

pub(super) fn component_payload(component_type: ComponentType, id: &str) -> InteractionData {
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

#[tokio::test]
async fn dynamic_handlers_dispatch_with_borrowed_values_and_static_priority() -> Result<()> {
    use std::sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    };

    let seen = Arc::new(AtomicU8::new(0));
    let mut events = crate::Events::new();
    let state = seen.clone();
    events.on_button("ticket:fixed", async move |ctx: &mut ButtonContext<'_>| {
        assert!(ctx.dynamic_id().is_none());
        ctx.response = ButtonResponse::Sent;
        state.fetch_or(1, Ordering::Relaxed);
        Ok::<_, crate::Error>(())
    })?;
    let state = seen.clone();
    events.on_dynamic_button("ticket", async move |ctx: &mut ButtonContext<'_>| {
        assert_eq!(ctx.custom_id(), "ticket:button");
        assert_eq!(
            ctx.dynamic_id().map(crate::ui::DynamicIdRef::value),
            Some("button")
        );
        ctx.response = ButtonResponse::Sent;
        state.fetch_or(2, Ordering::Relaxed);
        Ok::<_, crate::Error>(())
    })?;
    let state = seen.clone();
    events.on_dynamic_user_select("ticket", async move |ctx: &mut UserSelectContext<'_>| {
        assert_eq!(
            ctx.dynamic_id().map(crate::ui::DynamicIdRef::value),
            Some("who")
        );
        assert_eq!(ctx.values(), &[Id::new(15)]);
        ctx.inner.response = ButtonResponse::Sent;
        state.fetch_or(4, Ordering::Relaxed);
        Ok::<_, crate::Error>(())
    })?;
    let state = seen.clone();
    events.on_dynamic_modal("ticket", async move |ctx: &mut ModalContext<'_>| {
        assert_eq!(
            ctx.dynamic_id().map(crate::ui::DynamicIdRef::value),
            Some("form")
        );
        ctx.response = ResponseState::Replied;
        state.fetch_or(8, Ordering::Relaxed);
        Ok::<_, crate::Error>(())
    })?;
    let handlers = &events.interaction_handlers;
    assert!(handlers.has_modals());
    let string_select = interaction(
        InteractionType::MessageComponent,
        component_payload(ComponentType::TextSelectMenu, "ticket:who"),
    );
    assert!(!handlers.accepts(&string_select));
    let select = InteractionData::MessageComponent(Box::new(MessageComponentInteractionData {
        component_type: ComponentType::UserSelectMenu,
        custom_id: "ticket:who".into(),
        resolved: None,
        values: vec!["15".into()],
    }));
    let modal = InteractionData::ModalSubmit(Box::new(ModalInteractionData {
        components: vec![],
        custom_id: "ticket:form".into(),
        resolved: None,
    }));
    let http = twilight_http::Client::new("dummy".into());
    let hooks = crate::hooks::Hooks::default();
    for (kind, data) in [
        (
            InteractionType::MessageComponent,
            component_payload(ComponentType::Button, "ticket:fixed"),
        ),
        (
            InteractionType::MessageComponent,
            component_payload(ComponentType::Button, "ticket:button"),
        ),
        (InteractionType::MessageComponent, select),
        (InteractionType::ModalSubmit, modal),
    ] {
        let event = interaction(kind, data);
        assert!(handlers.accepts(&event));
        let (_, timing) = crate::timing::Pending::new((), None).begin();
        handlers.dispatch(&http, &event, timing, &hooks, &[]).await;
    }
    assert_eq!(seen.load(Ordering::Relaxed), 15);
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

#[allow(
    deprecated,
    reason = "テスト用の値を組み立てるために、非推奨のフィールドを埋める。"
)]
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

#[tokio::test]
async fn dispatches_modal_file_upload_and_checkbox_submissions() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let data: ModalInteractionData = serde_json::from_value(serde_json::json!({
        "custom_id": "feedback",
        "components": [
            {"type": 18, "id": 1, "component": {
                "type": 19, "id": 2, "custom_id": "evidence", "values": ["12", "11", "13"]
            }},
            {"type": 18, "id": 3, "component": {
                "type": 19, "id": 4, "custom_id": "optional", "values": []
            }},
            {"type": 18, "id": 5, "component": {
                "type": 23, "id": 6, "custom_id": "updates", "value": true
            }},
            {"type": 18, "id": 7, "component": {
                "type": 23, "id": 8, "custom_id": "share", "value": false
            }}
        ],
        "resolved": {
            "attachments": {
                "11": {
                    "id": "11", "filename": "preview.png", "content_type": "image/png",
                    "ephemeral": true, "size": 20, "url": "https://example.test/preview.png",
                    "proxy_url": "https://example.test/proxy/preview.png"
                },
                "13": {
                    "id": "14", "filename": "mismatched.png", "size": 20,
                    "url": "https://example.test/mismatched.png",
                    "proxy_url": "https://example.test/proxy/mismatched.png"
                }
            }
        }
    }))
    .unwrap();
    let event = interaction(
        InteractionType::ModalSubmit,
        InteractionData::ModalSubmit(Box::new(data)),
    );
    let called = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&called);
    let mut handlers = InteractionHandlers::default();
    handlers
        .register_modal("feedback", async move |ctx: &mut ModalContext<'_>| {
            assert_eq!(
                ctx.file_upload("evidence"),
                Some([Id::new(12), Id::new(11), Id::new(13)].as_slice())
            );
            assert_eq!(ctx.file_upload("optional"), Some([].as_slice()));
            assert!(ctx.file_upload("Evidence").is_none());
            assert!(ctx.text_input("evidence").is_none());
            assert_eq!(ctx.checkbox("updates"), Some(true));
            assert_eq!(ctx.checkbox("share"), Some(false));
            assert_eq!(ctx.checkbox("Share"), None);
            assert_eq!(ctx.checkbox("evidence"), None);
            assert!(ctx.attachment(Id::new(12)).is_none());
            assert!(ctx.attachment(Id::new(13)).is_none());
            assert!(ctx.attachment(Id::new(14)).is_none());
            let attachment = ctx.attachment(Id::new(11)).unwrap();
            assert_eq!(attachment.filename, "preview.png");
            assert!(attachment.ephemeral);
            observed.store(true, Ordering::Relaxed);
            Ok(())
        })
        .unwrap();
    let hooks = crate::hooks::Hooks {
        messages: Messages {
            modal_accepted: None,
            modal_failed: None,
            ..Default::default()
        },
        ..Default::default()
    };
    let (_, timing) = crate::timing::Pending::new((), None).begin();
    assert!(handlers.accepts(&event));
    handlers
        .dispatch(
            &twilight_http::Client::builder().build(),
            &event,
            timing,
            &hooks,
            &[],
        )
        .await;
    assert!(called.load(Ordering::Relaxed));
}

#[tokio::test]
async fn dispatches_modal_select_values_with_typed_ids_and_resolved_mentions() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let mut data: ModalInteractionData = serde_json::from_value(serde_json::json!({
        "custom_id": "survey",
        "components": [
            {"type": 18, "id": 1, "component": {
                "type": 3, "id": 2, "custom_id": "color", "values": []
            }},
            {"type": 18, "id": 3, "component": {
                "type": 5, "id": 4, "custom_id": "user", "values": ["42"]
            }},
            {"type": 18, "id": 5, "component": {
                "type": 6, "id": 6, "custom_id": "role", "values": ["44"]
            }},
            {"type": 18, "id": 7, "component": {
                "type": 8, "id": 8, "custom_id": "channel", "values": ["45"]
            }},
            {"type": 18, "id": 9, "component": {
                "type": 7, "id": 10, "custom_id": "mention", "values": ["44", "42"]
            }}
        ]
    }))
    .unwrap();
    let mut resolved: InteractionDataResolved =
        serde_json::from_value(serde_json::json!({})).unwrap();
    resolved.users.insert(Id::new(42), user(42, false));
    resolved.roles.insert(Id::new(44), role(44));
    data.resolved = Some(resolved);
    let event = interaction(
        InteractionType::ModalSubmit,
        InteractionData::ModalSubmit(Box::new(data)),
    );
    let called = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&called);
    let mut handlers = InteractionHandlers::default();
    handlers
        .register_modal("survey", async move |ctx: &mut ModalContext<'_>| {
            assert_eq!(ctx.string_select("color"), Some([].as_slice()));
            assert_eq!(ctx.string_select("user"), None);
            assert_eq!(ctx.user_select("user"), Some([Id::new(42)].as_slice()));
            assert_eq!(ctx.role_select("role"), Some([Id::new(44)].as_slice()));
            assert_eq!(
                ctx.channel_select("channel"),
                Some([Id::new(45)].as_slice())
            );
            assert_eq!(ctx.mentionable_select("color")?, None);
            assert_eq!(
                ctx.mentionable_select("mention")?.as_deref(),
                Some(
                    [
                        Mentionable::Role(Id::new(44)),
                        Mentionable::User(Id::new(42))
                    ]
                    .as_slice()
                )
            );
            observed.store(true, Ordering::Relaxed);
            Ok(())
        })
        .unwrap();
    let hooks = crate::hooks::Hooks {
        messages: Messages {
            modal_accepted: None,
            modal_failed: None,
            ..Default::default()
        },
        ..Default::default()
    };
    let (_, timing) = crate::timing::Pending::new((), None).begin();
    assert!(handlers.accepts(&event));
    handlers
        .dispatch(
            &twilight_http::Client::builder().build(),
            &event,
            timing,
            &hooks,
            &[],
        )
        .await;
    assert!(called.load(Ordering::Relaxed));
}

#[tokio::test]
async fn dispatches_modal_checkbox_group_values_and_distinguishes_empty_fields() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let data: ModalInteractionData = serde_json::from_value(serde_json::json!({
        "custom_id": "survey",
        "components": [
            {"type": 18, "id": 1, "component": {
                "type": 22, "id": 2, "custom_id": "colors", "values": ["green", "blue"]
            }},
            {"type": 18, "id": 3, "component": {
                "type": 22, "id": 4, "custom_id": "unchecked", "values": []
            }}
        ]
    }))
    .unwrap();
    let event = interaction(
        InteractionType::ModalSubmit,
        InteractionData::ModalSubmit(Box::new(data)),
    );
    let called = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&called);
    let mut handlers = InteractionHandlers::default();
    handlers
        .register_modal("survey", async move |ctx: &mut ModalContext<'_>| {
            let values = ctx.checkbox_group("colors").unwrap();
            assert_eq!(values, ["green", "blue"]);
            assert_eq!(ctx.checkbox_group("unchecked"), Some([].as_slice()));
            assert!(ctx.checkbox_group("Colors").is_none());
            assert!(ctx.string_select("colors").is_none());
            observed.store(true, Ordering::Relaxed);
            Ok(())
        })
        .unwrap();
    let hooks = crate::hooks::Hooks {
        messages: Messages {
            modal_accepted: None,
            modal_failed: None,
            ..Default::default()
        },
        ..Default::default()
    };
    let (_, timing) = crate::timing::Pending::new((), None).begin();
    assert!(handlers.accepts(&event));
    handlers
        .dispatch(
            &twilight_http::Client::builder().build(),
            &event,
            timing,
            &hooks,
            &[],
        )
        .await;
    assert!(called.load(Ordering::Relaxed));
}

#[tokio::test]
async fn modal_original_response_uses_the_private_submit_reply_not_the_source_message() -> Result<()>
{
    use crate::{Error, ResponseStateError, slash::tests::exchange_response, ui::ComponentsV2};
    use serde_json::{Value, json};
    use std::time::Duration;
    use tokio::{net::TcpListener, time::timeout};
    use twilight_model::channel::message::MessageFlags;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(Error::handler)?;
    let http = twilight_http::Client::builder()
        .proxy(
            listener.local_addr().map_err(Error::handler)?.to_string(),
            true,
        )
        .build();
    let mut event = interaction(
        InteractionType::ModalSubmit,
        InteractionData::ModalSubmit(Box::new(ModalInteractionData {
            components: Vec::new(),
            custom_id: "survey".into(),
            resolved: None,
        })),
    );
    event.token = "modal-submit-token".into();
    event.message = Some(message(99, user(4, true)));
    let mut handlers = InteractionHandlers::default();
    handlers.register_modal("survey", async |ctx: &mut ModalContext<'_>| -> Result<()> {
        let ui = UiMessage::new().content("従来形式");
        let v2 = ComponentsV2::new().text("<@123>");
        assert!(matches!(
            ctx.original_response().await,
            Err(Error::ResponseState(
                ResponseStateError::OriginalResponseUnavailable
            ))
        ));
        for result in [
            ctx.edit_original_response("本文").await,
            ctx.edit_original_ui(&ui).await,
            ctx.edit_original_v2(&v2).await,
            ctx.delete_original_response().await,
        ] {
            assert!(matches!(
                result,
                Err(Error::ResponseState(
                    ResponseStateError::OriginalResponseUnavailable
                ))
            ));
        }
        ctx.defer_ephemeral().await?;
        ctx.edit_original_v2(&v2).await?;
        assert!(matches!(
            ctx.edit_original_ui(&ui).await,
            Err(Error::ResponseState(
                ResponseStateError::OriginalResponseFormatMismatch
            ))
        ));
        let response = ctx.original_response().await?;
        assert_eq!(response.id, Id::new(7));
        assert!(response.flags.unwrap().contains(MessageFlags::EPHEMERAL));
        ctx.delete_original_response().await?;
        assert!(matches!(
            ctx.delete_original_response().await,
            Err(Error::ResponseState(
                ResponseStateError::OriginalResponseUnavailable
            ))
        ));
        ctx.followup_ephemeral("deleted").await
    })?;
    let hooks = crate::hooks::Hooks::default();
    let (_, timing) = crate::timing::Pending::new((), None).begin();
    let server = async {
        let callback: Value = serde_json::from_str(
            &exchange_response(
                &listener,
                "POST",
                Some("/interactions/1/modal-submit-token/callback"),
                None,
            )
            .await?,
        )
        .unwrap();
        assert_eq!(callback["type"], 5);
        assert_eq!(callback["data"]["flags"], 64);
        let path = Some("/webhooks/10/modal-submit-token/messages/@original");
        let patch: Value =
            serde_json::from_str(&exchange_response(&listener, "PATCH", path, None).await?)
                .unwrap();
        assert_eq!(patch["flags"], 32_768);
        assert_eq!(patch["components"][0]["type"], 10);
        assert_eq!(patch["components"][0]["content"], "<@123>");
        assert_eq!(patch["allowed_mentions"]["parse"], json!([]));
        assert!(patch.get("attachments").is_none());
        assert!(patch.get("content").is_none());
        let mut reply = message(7, user(4, true));
        reply.flags = Some(MessageFlags::EPHEMERAL | MessageFlags::IS_COMPONENTS_V2);
        let reply = serde_json::to_string(&reply).unwrap();
        exchange_response(&listener, "GET", path, Some(&reply)).await?;
        exchange_response(&listener, "DELETE", path, None).await?;
        let followup: Value = serde_json::from_str(
            &exchange_response(
                &listener,
                "POST",
                Some("/webhooks/10/modal-submit-token"),
                None,
            )
            .await?,
        )
        .unwrap();
        assert_eq!(followup["flags"], 64);
        assert_eq!(followup["content"], "deleted");
        Ok::<_, std::io::Error>(())
    };
    let (_, server) = timeout(Duration::from_secs(5), async {
        tokio::join!(
            handlers.dispatch(&http, &event, timing, &hooks, &[]),
            server
        )
    })
    .await
    .map_err(Error::handler)?;
    server.map_err(Error::handler)
}

fn radio_submit_event(components: serde_json::Value) -> String {
    let event = interaction(
        InteractionType::ModalSubmit,
        InteractionData::ModalSubmit(Box::new(ModalInteractionData {
            components: Vec::new(),
            custom_id: "survey".into(),
            resolved: None,
        })),
    );
    let mut payload = serde_json::to_value(event).unwrap();
    *payload.get_mut("data").unwrap() = serde_json::json!({
        "custom_id": "survey",
        "components": components,
    });
    serde_json::json!({"op": 0, "s": 1, "t": "INTERACTION_CREATE", "d": payload}).to_string()
}

#[tokio::test]
async fn dispatches_radio_group_values_from_the_gateway_without_replacing_other_fields() {
    use crate::message_events::{IncomingEvent, decode_text};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use twilight_gateway::{Event, EventTypeFlags};

    let payload = radio_submit_event(serde_json::json!([
        {"type": 18, "id": 1, "component": {
            "type": 21, "id": 2, "custom_id": "color", "value": "blue"
        }},
        {"type": 18, "id": 3, "component": {
            "type": 21, "id": 4, "custom_id": "optional", "value": null
        }},
        {"type": 18, "id": 5, "component": {
            "type": 4, "id": 6, "custom_id": "text", "value": "test"
        }},
        {"type": 18, "id": 7, "component": {
            "type": 23, "id": 8, "custom_id": "checkbox", "value": true
        }}
    ]));
    let flags = EventTypeFlags::INTERACTION_CREATE;
    assert!(matches!(
        decode_text(payload.clone(), flags, false),
        Some(Ok(IncomingEvent::Standard(Event::InteractionCreate(_))))
    ));
    let Some(Ok(IncomingEvent::ModalWithRadio(event, radios))) = decode_text(payload, flags, true)
    else {
        panic!("expected the typed Gateway interaction with radio values");
    };
    let event = event.0;
    let called = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&called);
    let mut handlers = InteractionHandlers::default();
    handlers
        .register_modal("survey", async move |ctx: &mut ModalContext<'_>| {
            assert_eq!(ctx.radio_group("color"), Some(Some("blue")));
            assert_eq!(ctx.radio_group("optional"), Some(None));
            assert_eq!(ctx.radio_group("missing"), None);
            assert_eq!(ctx.radio_group("text"), None);
            assert_eq!(ctx.text_input("text"), Some("test"));
            assert_eq!(ctx.text_input("color"), None);
            assert_eq!(ctx.checkbox("checkbox"), Some(true));
            observed.store(true, Ordering::Relaxed);
            Ok(())
        })
        .unwrap();
    let hooks = crate::hooks::Hooks {
        messages: Messages {
            modal_accepted: None,
            modal_failed: None,
            ..Default::default()
        },
        ..Default::default()
    };
    let (_, timing) = crate::timing::Pending::new((), None).begin();
    assert!(handlers.accepts(&event));
    handlers
        .dispatch(
            &twilight_http::Client::builder().build(),
            &event,
            timing,
            &hooks,
            &radios,
        )
        .await;
    assert!(called.load(Ordering::Relaxed));
}

#[test]
fn rejects_invalid_radio_metadata_and_preserves_standard_interaction_validation() {
    use crate::{Error, SelectionError, message_events::decode_text};
    use twilight_gateway::EventTypeFlags;

    let flags = EventTypeFlags::INTERACTION_CREATE;
    let malformed = radio_submit_event(serde_json::json!([{"type": 18, "id": 1, "component": {
        "type": 21, "id": 2, "custom_id": "color"
    }}]));
    assert!(matches!(
        decode_text(malformed, flags, true),
        Some(Err(Error::Selection(SelectionError::Unreadable)))
    ));
    let repeated = radio_submit_event(serde_json::json!([
        {"type": 18, "id": 1, "component": {"type": 21, "id": 2, "custom_id": "same", "value": "a"}},
        {"type": 18, "id": 3, "component": {"type": 4, "id": 4, "custom_id": "same", "value": "b"}}
    ]));
    assert!(matches!(
        decode_text(repeated, flags, true),
        Some(Err(Error::Selection(SelectionError::Duplicate)))
    ));
    let invalid_other_field = radio_submit_event(serde_json::json!([
        {"type": 18, "id": 1, "component": {"type": 21, "id": 2, "custom_id": "radio", "value": "a"}},
        {"type": 18, "id": 3, "component": {"type": 4, "id": 4, "custom_id": "text"}}
    ]));
    assert!(matches!(
        decode_text(invalid_other_field, flags, true),
        Some(Err(Error::GatewayReceive(_)))
    ));
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

pub(super) fn user(id: u64, bot: bool) -> User {
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

#[allow(
    deprecated,
    reason = "テスト用の値を組み立てるために、非推奨のフィールドを埋める。"
)]
pub(super) fn message(id: u64, author: User) -> Message {
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
