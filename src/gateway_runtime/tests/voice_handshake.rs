use crate::{VoiceConnection, VoiceConnectionInfo, VoiceConnectionStatus};

use super::*;

const DEADLINE: Duration = Duration::from_secs(5);

#[tokio::test]
async fn managed_voice_pairs_gateway_events_and_fences_abandoned_attempts() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let url = format!("ws://{address}");
    let (ready, mut ready_rx) = mpsc::channel(1);
    let mut events = Events::new();
    events.on_ready(async move |_: &Ready| ready.send(()).await);
    let (bot, shutdown) = Bot::new(FIXTURE_TOKEN)
        .unwrap()
        .add_intents(Intents::GUILD_VOICE_STATES)
        .with_events(events)
        .with_shutdown();
    let (mut bot, voice) = bot.with_voice_gateway();
    bot.gateway_test_port = Some(address.port());

    let mock = async {
        let mut socket = timeout(DEADLINE, accept(&listener))
            .await
            .expect("Gateway connected");
        timeout(DEADLINE, receive_opcode(&mut socket, 2))
            .await
            .expect("Gateway identified");
        send_ready(&mut socket, &url, "managed-voice-session", [0, 1]).await;
        timeout(DEADLINE, ready_rx.recv())
            .await
            .expect("READY routed")
            .unwrap();
        let mut connection = timeout(DEADLINE, join_with_server_first(&voice, &mut socket))
            .await
            .expect("initial join confirmed");
        timeout(DEADLINE, reallocate(&mut socket, &mut connection))
            .await
            .expect("allocation renewed");
        let mut moved = timeout(DEADLINE, move_with_state_first(&voice, &mut socket))
            .await
            .expect("channel move confirmed");
        assert!(matches!(
            connection.changed().await,
            Ok(VoiceConnectionStatus::Disconnected)
        ));
        confirm_leave(&voice, &mut socket).await;
        assert!(matches!(
            moved.changed().await,
            Ok(VoiceConnectionStatus::Disconnected)
        ));
        timeout_and_cancel(&voice, &mut socket).await;
        shutdown.request();
        expect_close(&mut socket, 1000).await;
    };
    timeout(Duration::from_secs(30), async {
        tokio::try_join!(bot.start(), async {
            mock.await;
            Ok(())
        })
    })
    .await
    .unwrap()
    .unwrap();
}

fn start_join(
    voice: &VoiceGateway,
    guild: u64,
    channel: u64,
) -> tokio::task::JoinHandle<Result<VoiceConnection, VoiceGatewayError>> {
    let voice = voice.clone();
    tokio::spawn(async move { voice.join(Id::new(guild), Id::new(channel), DEADLINE).await })
}

fn assigned(status: VoiceConnectionStatus) -> std::sync::Arc<VoiceConnectionInfo> {
    let VoiceConnectionStatus::Ready(info) = status else {
        panic!("expected voice allocation");
    };
    info
}

async fn join_with_server_first(
    voice: &VoiceGateway,
    socket: &mut WebSocketStream<TcpStream>,
) -> VoiceConnection {
    let mut join = start_join(voice, 7, 8);
    let command = receive_opcode(socket, 4).await;
    assert_eq!(command["d"]["guild_id"], "7");
    assert_eq!(command["d"]["channel_id"], "8");
    send_server(socket, 2, "17", Some("unrelated.host"), "unrelated").await;
    send_state(socket, 3, "7", "3", Some("8"), "unrelated-user").await;
    send_state(socket, 4, "7", "2", Some("9"), "unrelated-channel").await;
    send_server(
        socket,
        5,
        "7",
        Some("fixture.local:2048"),
        "voice-fixture-1",
    )
    .await;
    assert!(timeout(Duration::from_millis(20), &mut join).await.is_err());
    send_state(socket, 6, "7", "2", Some("8"), "voice-session-1").await;
    let connection = join.await.unwrap().unwrap();
    let info = assigned(connection.status());
    assert_eq!(info.user_id.get(), 2);
    assert_eq!(info.channel_id.get(), 8);
    assert_eq!(info.endpoint, "fixture.local:2048");
    assert_eq!(info.token(), "voice-fixture-1");
    assert_eq!(info.session_id(), "voice-session-1");
    let debug = format!("{info:?} {connection:?}");
    assert!(!debug.contains("voice-fixture-1"));
    assert!(!debug.contains("voice-session-1"));
    let current = voice.join(Id::new(7), Id::new(8), DEADLINE).await.unwrap();
    assert_eq!(assigned(current.status()).channel_id.get(), 8);
    assert_eq!(
        voice.set_state(Id::new(7), VoiceStateChange::leave()).await,
        Err(VoiceGatewayError::Managed)
    );
    connection
}

async fn reallocate(socket: &mut WebSocketStream<TcpStream>, connection: &mut VoiceConnection) {
    send_server(socket, 7, "7", None, "discarded").await;
    assert!(matches!(
        connection.changed().await,
        Ok(VoiceConnectionStatus::AwaitingServer)
    ));
    send_server(
        socket,
        8,
        "7",
        Some("fixture.local:2048"),
        "voice-fixture-2",
    )
    .await;
    let info = assigned(connection.changed().await.unwrap());
    assert_eq!(info.token(), "voice-fixture-2");
    assert_eq!(info.session_id(), "voice-session-1");
}

async fn move_with_state_first(
    voice: &VoiceGateway,
    socket: &mut WebSocketStream<TcpStream>,
) -> VoiceConnection {
    let mut join = start_join(voice, 7, 9);
    assert_eq!(receive_opcode(socket, 4).await["d"]["channel_id"], "9");
    send_state(socket, 9, "7", "2", Some("9"), "voice-session-2").await;
    send_server(
        socket,
        10,
        "7",
        Some("fixture.local:2048"),
        "voice-fixture-2",
    )
    .await;
    assert!(timeout(Duration::from_millis(20), &mut join).await.is_err());
    send_server(
        socket,
        11,
        "7",
        Some("fixture.local:2048"),
        "voice-fixture-3",
    )
    .await;
    let connection = join.await.unwrap().unwrap();
    let info = assigned(connection.status());
    assert_eq!(info.token(), "voice-fixture-3");
    assert_eq!(info.session_id(), "voice-session-2");
    connection
}

async fn confirm_leave(voice: &VoiceGateway, socket: &mut WebSocketStream<TcpStream>) {
    let leave = voice.clone();
    let mut leaving = tokio::spawn(async move { leave.leave(Id::new(7), DEADLINE).await });
    assert!(receive_opcode(socket, 4).await["d"]["channel_id"].is_null());
    send_state(socket, 12, "7", "3", None, "unrelated-user").await;
    assert!(
        timeout(Duration::from_millis(20), &mut leaving)
            .await
            .is_err()
    );
    send_state(socket, 13, "7", "2", None, "voice-session-2").await;
    leaving.await.unwrap().unwrap();
}

async fn timeout_and_cancel(voice: &VoiceGateway, socket: &mut WebSocketStream<TcpStream>) {
    let timed_out = start_join(voice, 18, 9);
    assert_eq!(receive_opcode(socket, 4).await["d"]["guild_id"], "18");
    assert!(matches!(
        voice.join(Id::new(18), Id::new(9), DEADLINE).await,
        Err(VoiceGatewayError::Managed)
    ));
    tokio::time::pause();
    tokio::time::advance(DEADLINE).await;
    tokio::time::resume();
    assert!(matches!(
        timed_out.await.unwrap(),
        Err(VoiceGatewayError::TimedOut)
    ));
    send_state(socket, 14, "18", "2", Some("9"), "late-session").await;
    send_server(socket, 15, "18", Some("late.local"), "late-test-token").await;
    assert!(matches!(
        voice.join(Id::new(18), Id::new(9), DEADLINE).await,
        Err(VoiceGatewayError::Indeterminate)
    ));

    let cancelled = start_join(voice, 19, 9);
    assert_eq!(receive_opcode(socket, 4).await["d"]["guild_id"], "19");
    cancelled.abort();
    assert!(cancelled.await.unwrap_err().is_cancelled());
    assert!(matches!(
        voice.join(Id::new(19), Id::new(9), DEADLINE).await,
        Err(VoiceGatewayError::Indeterminate)
    ));
}

async fn send_state(
    socket: &mut WebSocketStream<TcpStream>,
    sequence: u64,
    guild: &str,
    user: &str,
    channel: Option<&str>,
    session: &str,
) {
    let payload = json!({"op":0,"s":sequence,"t":"VOICE_STATE_UPDATE","d":{
        "guild_id":guild,"channel_id":channel,"user_id":user,"session_id":session,
        "deaf":false,"mute":false,"self_deaf":false,"self_mute":false,"self_video":false,"suppress":false
    }});
    socket.send(Frame::text(payload.to_string())).await.unwrap();
}

async fn send_server(
    socket: &mut WebSocketStream<TcpStream>,
    sequence: u64,
    guild: &str,
    endpoint: Option<&str>,
    token: &str,
) {
    let payload = json!({"op":0,"s":sequence,"t":"VOICE_SERVER_UPDATE","d":{"guild_id":guild,"endpoint":endpoint,"token":token}});
    socket.send(Frame::text(payload.to_string())).await.unwrap();
}
