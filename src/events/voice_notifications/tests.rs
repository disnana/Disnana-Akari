use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::sync::mpsc;
use twilight_gateway::{EventTypeFlags, ShardId, parse};
use twilight_http::Client;

use super::*;
use crate::{
    Bot, NoMessageHandler, dispatch_events,
    hooks::Hooks,
    message_events::{GatewayDecodeOptions, IncomingEvent, decode_text_with_options},
    permissions::PermissionResolver,
    route_gateway_event,
};

const EFFECT: &str = "VOICE_CHANNEL_EFFECT_SEND";
const CREATE: &str = "GUILD_SOUNDBOARD_SOUND_CREATE";
const UPDATE: &str = "GUILD_SOUNDBOARD_SOUND_UPDATE";
const DELETE: &str = "GUILD_SOUNDBOARD_SOUND_DELETE";
const UPDATE_ALL: &str = "GUILD_SOUNDBOARD_SOUNDS_UPDATE";
const LIST: &str = "SOUNDBOARD_SOUNDS";
const STATUS: &str = "VOICE_CHANNEL_STATUS_UPDATE";
const START: &str = "VOICE_CHANNEL_START_TIME_UPDATE";
const INFO: &str = "CHANNEL_INFO";
const EXPRESSIONS: Intents = Intents::GUILD_EMOJIS_AND_STICKERS;
const VOICE: Intents = Intents::GUILD_VOICE_STATES;

fn payload(name: &str, data: Value) -> String {
    json!({ "op": 0, "s": 2, "t": name, "d": data }).to_string()
}

fn decode(name: &str, data: Value, types: VoiceNotificationTypes) -> Option<Result<IncomingEvent>> {
    decode_text_with_options(
        payload(name, data),
        EventTypeFlags::empty(),
        GatewayDecodeOptions {
            voice_notifications: types,
            ..GatewayDecodeOptions::default()
        },
    )
}

fn sound() -> Value {
    json!({
        "sound_id":"12", "guild_id":"7", "name":"bird", "volume":0.5,
        "emoji_id":null, "emoji_name":"🐦", "available":false,
        "user":{"id":"9","username":"member","discriminator":"0"}
    })
}

#[tokio::test]
async fn only_requested_events_are_decoded_and_intents_are_checked_before_connection() {
    let mut events = Events::new();
    let empty = events.voice_notification_types(Intents::empty()).unwrap();
    assert!(empty.is_empty());
    assert!(decode(EFFECT, Value::Null, empty).is_none());
    assert!(
        parse(payload(CREATE, sound()), EventTypeFlags::all())
            .unwrap()
            .is_none()
    );

    events.on_soundboard_sounds(async |_: &SoundboardSounds| -> Result<()> { Ok(()) });
    let list = events.voice_notification_types(Intents::empty()).unwrap();
    assert!(decode(UPDATE_ALL, Value::Null, list).is_none());
    assert!(matches!(
        decode(LIST, Value::Null, list),
        Some(Err(Error::GatewayNotificationDecode { event: LIST, .. }))
    ));

    events.on_guild_soundboard_sound_create(async |_: &GuildSoundboardSoundCreate| -> Result<()> {
        Ok(())
    });
    assert!(matches!(
        events.voice_notification_types(VOICE),
        Err(Error::MissingEventIntents { any_of }) if any_of == EXPRESSIONS
    ));
    let types = events.voice_notification_types(EXPRESSIONS).unwrap();
    assert!(decode(UPDATE, sound(), types).is_none());
    assert!(decode(CREATE, sound(), types).unwrap().is_ok());

    events
        .on_voice_channel_effect_send(async |_: &VoiceChannelEffectSend| -> Result<()> { Ok(()) });
    let bot = Bot::new("fixture")
        .unwrap()
        .with_intents(EXPRESSIONS)
        .with_events(events);
    assert!(matches!(
        bot.start().await,
        Err(Error::MissingEventIntents { any_of }) if any_of == VOICE
    ));
}

#[tokio::test]
async fn channel_updates_and_info_keep_nullable_fields_and_require_only_documented_intents() {
    let mut events = Events::new();
    let status = json!({"id":"8", "guild_id":"7", "status":null});
    events.on_channel_info(async |_: &ChannelInfo| -> Result<()> { Ok(()) });
    let info_only = events.voice_notification_types(Intents::empty()).unwrap();
    assert!(decode(STATUS, status.clone(), info_only).is_none());

    let observed = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&observed);
    events.on_voice_channel_status_update(
        async move |data: &VoiceChannelStatusUpdate| -> Result<()> {
            assert_eq!(data.channel_id.get(), 8);
            assert_eq!(data.guild_id.get(), 7);
            captured.lock().unwrap().push(data.status.clone());
            Ok(())
        },
    );
    events.on_voice_channel_start_time_update(
        async |_: &VoiceChannelStartTimeUpdate| -> Result<()> { Ok(()) },
    );
    assert!(matches!(
        events.voice_notification_types(VOICE),
        Err(Error::MissingEventIntents { any_of }) if any_of == Intents::GUILDS
    ));
    let types = events.voice_notification_types(Intents::GUILDS).unwrap();
    for data in [status, json!({"id":"8", "guild_id":"7", "status":"録音中"})] {
        let IncomingEvent::VoiceNotification(event) = decode(STATUS, data, types).unwrap().unwrap()
        else {
            panic!("expected a typed voice notification");
        };
        events
            .voice_notification(&event, &Reporter::default())
            .await;
    }
    assert_eq!(*observed.lock().unwrap(), [None, Some("録音中".to_owned())]);
    assert!(matches!(
        decode(STATUS, json!({"id":"8", "guild_id":"7"}), types),
        Some(Err(Error::GatewayNotificationDecode { event: STATUS, .. }))
    ));

    for start in [None, Some(1_769_904_000_i64)] {
        let data = json!({"id":"8", "guild_id":"7", "voice_start_time":start});
        let IncomingEvent::VoiceNotification(VoiceNotification::VoiceChannelStartTimeUpdate(data)) =
            decode(START, data, types).unwrap().unwrap()
        else {
            panic!("expected a start-time update");
        };
        assert_eq!(data.channel_id.get(), 8);
        assert_eq!(data.voice_start_time, start);
    }
    let data = json!({"guild_id":"7", "channels":[{"id":"8"}, {"id":"9", "status":null, "voice_start_time":1_769_904_000} ]});
    let IncomingEvent::VoiceNotification(VoiceNotification::ChannelInfo(data)) =
        decode(INFO, data, info_only).unwrap().unwrap()
    else {
        panic!("expected channel information");
    };
    assert_eq!(data.guild_id.get(), 7);
    assert_eq!(data.channels[0].channel_id.get(), 8);
    assert_eq!(data.channels[0].status, None);
    assert_eq!(data.channels[1].voice_start_time, Some(1_769_904_000));
}

#[tokio::test]
async fn voice_soundboard_and_list_payloads_reach_the_ordered_event_worker() {
    let received = Arc::new(Mutex::new(Vec::new()));
    let mut events = Events::new();
    let log = Arc::clone(&received);
    events.on_voice_channel_effect_send(
        async move |effect: &VoiceChannelEffectSend| -> Result<()> {
            assert_eq!(effect.guild_id.get(), 7);
            assert_eq!(effect.channel_id.get(), 8);
            assert_eq!(effect.user_id.get(), 9);
            assert_eq!(effect.sound_volume, Some(0.5));
            assert_eq!(
                effect.animation_type,
                Some(VoiceEffectAnimation::Unknown(23))
            );
            assert_eq!(effect.animation_id, Some(6));
            assert!(matches!(
                effect.emoji,
                Some(EmojiReactionType::Unicode { .. })
            ));
            log.lock()
                .unwrap()
                .push((EFFECT, effect.sound_id.unwrap().get()));
            Ok(())
        },
    );
    let log = Arc::clone(&received);
    events.on_guild_soundboard_sound_create(
        async move |event: &GuildSoundboardSoundCreate| -> Result<()> {
            assert_eq!(event.sound.guild_id.unwrap().get(), 7);
            assert_eq!(event.sound.user.as_ref().unwrap().id.get(), 9);
            assert!(!event.sound.available);
            assert_eq!(event.sound.emoji_name.as_deref(), Some("🐦"));
            log.lock()
                .unwrap()
                .push((CREATE, event.sound.sound_id.get()));
            Ok(())
        },
    );
    let log = Arc::clone(&received);
    events.on_guild_soundboard_sound_update(
        async move |event: &GuildSoundboardSoundUpdate| -> Result<()> {
            assert_eq!(event.sound.name, "bird");
            log.lock()
                .unwrap()
                .push((UPDATE, event.sound.sound_id.get()));
            Ok(())
        },
    );
    let log = Arc::clone(&received);
    events.on_guild_soundboard_sound_delete(
        async move |event: &GuildSoundboardSoundDelete| -> Result<()> {
            assert_eq!(event.guild_id.get(), 7);
            log.lock().unwrap().push((DELETE, event.sound_id.get()));
            Ok(())
        },
    );
    let log = Arc::clone(&received);
    events.on_guild_soundboard_sounds_update(
        async move |event: &GuildSoundboardSoundsUpdate| -> Result<()> {
            assert_eq!(event.guild_id.get(), 7);
            log.lock()
                .unwrap()
                .push((UPDATE_ALL, event.soundboard_sounds[0].sound_id.get()));
            Ok(())
        },
    );
    let log = Arc::clone(&received);
    events.on_soundboard_sounds(async move |event: &SoundboardSounds| -> Result<()> {
        assert_eq!(event.guild_id.get(), 7);
        log.lock()
            .unwrap()
            .push((LIST, event.soundboard_sounds[0].sound_id.get()));
        Ok(())
    });

    let types = events
        .voice_notification_types(VOICE | EXPRESSIONS)
        .unwrap();
    let (queue, receiver) = mpsc::channel(6);
    let hooks = Hooks::default();
    let permissions = PermissionResolver::new(None);
    let list = || json!({"guild_id":"7","soundboard_sounds":[sound()]});
    for (name, data) in [
        (
            EFFECT,
            json!({"guild_id":"7","channel_id":"8","user_id":"9","sound_id":12,
            "sound_volume":0.5,"animation_type":23,"animation_id":6,"emoji":{"id":null,"name":"🐦"}}),
        ),
        (CREATE, sound()),
        (UPDATE, sound()),
        (DELETE, json!({"guild_id":"7","sound_id":"12"})),
        (UPDATE_ALL, list()),
        (LIST, list()),
    ] {
        let event = decode(name, data, types).unwrap().unwrap();
        assert!(
            route_gateway_event(
                Ok(event),
                &events,
                &permissions,
                &queue,
                &hooks.reporter,
                ShardId::ONE,
                (None, None)
            )
            .is_none()
        );
    }
    drop(queue);
    dispatch_events(
        receiver,
        None::<NoMessageHandler>,
        &events,
        &Client::new("fixture".into()),
        &permissions,
        &hooks,
    )
    .await;
    assert_eq!(
        *received.lock().unwrap(),
        [EFFECT, CREATE, UPDATE, DELETE, UPDATE_ALL, LIST].map(|name| (name, 12))
    );
}
