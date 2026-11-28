use super::*;
use crate::VoiceStateTransition;
use twilight_model::gateway::payload::incoming::VoiceStateUpdate;

#[tokio::test]
async fn voice_transitions_are_seeded_and_keep_the_raw_events() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (updates, mut transitions) = mpsc::channel(4);
    let (raw, mut raw_states) = mpsc::channel(4);
    let mut events = Events::new();
    events.on_voice_state_transition(async move |event: &VoiceStateTransition| {
        updates
            .send((
                event.before().and_then(|before| before.channel_id()),
                event.after().channel_id(),
            ))
            .await
    });
    events.on_gateway(async move |event: &VoiceStateUpdate| raw.send(event.channel_id).await);
    let intents = Intents::GUILDS | Intents::GUILD_VOICE_STATES;
    let (mut bot, shutdown) = Bot::new(FIXTURE_TOKEN)
        .unwrap()
        .with_intents(intents)
        .with_events(events)
        .with_shutdown();
    bot.gateway_test_port = Some(address.port());
    let server = async {
        let mut socket = accept(&listener).await;
        let identify = receive_opcode(&mut socket, 2).await;
        assert_eq!(identify["d"]["intents"], intents.bits());
        send_ready(
            &mut socket,
            &format!("ws://{address}"),
            "voice-transitions",
            [0, 1],
        )
        .await;
        let mut guild = guild_payload(4);
        guild["voice_states"] = json!([state(Some("9"))]);
        send_dispatch(&mut socket, 2, "GUILD_CREATE", guild).await;
        send_dispatch(&mut socket, 3, "VOICE_STATE_UPDATE", state(Some("10"))).await;
        assert_eq!(raw_states.recv().await, Some(Some(Id::new(10))));
        assert_eq!(
            transitions.recv().await,
            Some((Some(Id::new(9)), Some(Id::new(10))))
        );
        send_dispatch(&mut socket, 4, "VOICE_STATE_UPDATE", state(None)).await;
        assert_eq!(raw_states.recv().await, Some(None));
        assert_eq!(transitions.recv().await, Some((Some(Id::new(10)), None)));
        shutdown.request();
        expect_close(&mut socket, 1000).await;
    };
    let (result, ()) = timeout(Duration::from_secs(10), async {
        tokio::join!(bot.start(), server)
    })
    .await
    .unwrap();
    assert!(result.is_ok(), "{result:?}");
}

#[tokio::test]
async fn voice_transition_intents_are_checked_before_connecting() {
    for (provided, missing) in [
        (Intents::GUILDS, Intents::GUILD_VOICE_STATES),
        (Intents::GUILD_VOICE_STATES, Intents::GUILDS),
    ] {
        let mut events = Events::new();
        events.on_voice_state_transition(async |_: &VoiceStateTransition| Ok::<(), Error>(()));
        let bot = Bot::new(FIXTURE_TOKEN)
            .unwrap()
            .with_intents(provided)
            .with_events(events);
        let result = timeout(Duration::from_secs(1), bot.start()).await.unwrap();
        assert!(matches!(result, Err(Error::MissingEventIntents { any_of }) if any_of == missing));
    }
}

fn state(channel: Option<&str>) -> Value {
    json!({
        "guild_id":"4", "user_id":"6", "channel_id":channel, "session_id":"voice-state-test",
        "deaf":false, "mute":false, "self_deaf":false, "self_mute":false, "self_video":false, "suppress":false
    })
}

#[tokio::test]
async fn raw_voice_wait_subscribes_and_observes_leave_without_event_handler() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (ready, mut ready_rx) = mpsc::channel(1);
    let mut events = Events::new();
    events.on_ready(async move |_: &Ready| ready.send(()).await);
    let (bot, waiters) = Bot::new(FIXTURE_TOKEN)
        .unwrap()
        .with_intents(Intents::GUILD_VOICE_STATES)
        .with_events(events)
        .with_waiters(crate::WaitLimits::default())
        .unwrap();
    let (mut bot, shutdown) = bot.with_shutdown();
    bot.gateway_test_port = Some(address.port());
    let server = async {
        let mut socket = accept(&listener).await;
        let identify = receive_opcode(&mut socket, 2).await;
        assert_eq!(identify["d"]["intents"], Intents::GUILD_VOICE_STATES.bits());
        send_ready(
            &mut socket,
            &format!("ws://{address}"),
            "voice-waits",
            [0, 1],
        )
        .await;
        ready_rx.recv().await.unwrap();

        let leave = waiters
            .wait_for_voice_state_where(Id::new(4), Id::new(6), Duration::from_secs(5), |state| {
                state.channel_id.is_none()
            })
            .unwrap();
        let mut unrelated = state(None);
        unrelated["guild_id"] = json!("5");
        send_dispatch(&mut socket, 2, "VOICE_STATE_UPDATE", unrelated).await;
        send_dispatch(&mut socket, 3, "VOICE_STATE_UPDATE", state(Some("10"))).await;
        send_dispatch(&mut socket, 4, "VOICE_STATE_UPDATE", state(None)).await;
        let update = leave.await.unwrap();
        assert_eq!(update.guild_id, Some(Id::new(4)));
        assert!(update.channel_id.is_none());

        shutdown.request();
        expect_close(&mut socket, 1000).await;
    };
    let (result, ()) = timeout(Duration::from_secs(10), async {
        tokio::join!(bot.start(), server)
    })
    .await
    .unwrap();
    assert!(result.is_ok(), "{result:?}");
}
