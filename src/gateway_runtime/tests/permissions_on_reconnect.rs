use twilight_model::guild::Permissions;

use super::*;

struct GuildProbe {
    guild: u64,
    channel: u64,
}

// チャンネル ID でシャードを判定すると、どちらも逆のシャードになります。
const STEADY: GuildProbe = GuildProbe {
    guild: 4,
    channel: 1 << 22,
};
const RECONNECTING: GuildProbe = GuildProbe {
    guild: 1 << 22,
    channel: 8,
};

#[tokio::test]
async fn silent_eof_refreshes_only_affected_shard_permissions_before_resumed() {
    let gateway = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = gateway.local_addr().unwrap();
    let url = format!("ws://{address}");
    let (received, mut observations) = mpsc::unbounded_channel();
    let (mut bot, shutdown) = Bot::new(FIXTURE_TOKEN)
        .unwrap()
        .with_recommended_shards()
        .add_intents(Intents::GUILDS | Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)
        .cache_permissions(Duration::from_secs(60))
        .with_shutdown();
    bot.gateway_test_port = Some(address.port());
    bot.http_test_port = Some(http.local_addr().unwrap().port());

    let client = bot.run(async move |ctx: Context<'_>| -> crate::Result<()> {
        let permissions = ctx.permissions().await?;
        received
            .send((
                ctx.message().id.get(),
                permissions.contains(Permissions::BAN_MEMBERS),
            ))
            .map_err(crate::Error::handler)
    });
    let server = async {
        respond_to_discovery(&http, 2).await;
        let (mut steady, mut reconnecting) = accept_shards(&gateway).await;
        send_ready(&mut steady, &url, "permissions-steady", [0, 2]).await;
        send_ready(&mut reconnecting, &url, "permissions-reconnecting", [1, 2]).await;

        STEADY.send(&mut steady, 10, 2).await;
        STEADY.reply_permissions(&http, true).await;
        assert_eq!(observations.recv().await, Some((10, true)));
        RECONNECTING.send(&mut reconnecting, 11, 2).await;
        RECONNECTING.reply_permissions(&http, true).await;
        assert_eq!(observations.recv().await, Some((11, true)));

        // Discord の Opcode 7 や WebSocket の Close を送らず TCP を切ります。
        drop(reconnecting);
        let mut resumed = accept(&gateway).await;
        let resume = receive_opcode(&mut resumed, 6).await;
        assert_eq!(resume["d"]["session_id"], "permissions-reconnecting");
        assert_eq!(resume["d"]["seq"], 2);

        STEADY.send(&mut steady, 12, 3).await;
        tokio::select! {
            seen = observations.recv() => assert_eq!(seen, Some((12, true))),
            socket = http.accept() => panic!("unaffected shard made a REST request: {:?}", socket.unwrap().1),
        }

        // Resume の再送イベントは RESUMED より前にも届くため、この時点で権限が新しくなります。
        RECONNECTING.send(&mut resumed, 13, 3).await;
        tokio::select! {
            seen = observations.recv() => panic!("permission check used a stale result: {seen:?}"),
            () = RECONNECTING.reply_permissions(&http, false) => {}
        }
        assert_eq!(observations.recv().await, Some((13, false)));
        resumed
            .send(Frame::text(r#"{"op":0,"s":4,"t":"RESUMED","d":{}}"#))
            .await
            .unwrap();
        shutdown.request();
        tokio::join!(
            expect_close(&mut steady, 1000),
            expect_close(&mut resumed, 1000)
        );
    };
    let (result, ()) = timeout(Duration::from_secs(15), async {
        tokio::join!(client, server)
    })
    .await
    .expect("two-shard permission fixture completed");
    result.unwrap();
}

async fn accept_shards(
    gateway: &TcpListener,
) -> (WebSocketStream<TcpStream>, WebSocketStream<TcpStream>) {
    let mut first = accept(gateway).await;
    let first_id = receive_opcode(&mut first, 2).await["d"]["shard"].clone();
    let mut second = accept(gateway).await;
    let second_id = receive_opcode(&mut second, 2).await["d"]["shard"].clone();
    if first_id == json!([0, 2]) {
        assert_eq!(second_id, json!([1, 2]));
        (first, second)
    } else {
        assert_eq!(first_id, json!([1, 2]));
        assert_eq!(second_id, json!([0, 2]));
        (second, first)
    }
}

impl GuildProbe {
    async fn send(&self, socket: &mut WebSocketStream<TcpStream>, message: u64, sequence: u64) {
        let mut probe: Value = serde_json::from_str(MESSAGE).unwrap();
        probe["s"] = sequence.into();
        probe["d"]["id"] = message.to_string().into();
        probe["d"]["guild_id"] = self.guild.to_string().into();
        probe["d"]["channel_id"] = self.channel.to_string().into();
        probe["d"]["member"] = json!({"roles":[], "deaf":false, "mute":false, "flags":0});
        socket.send(Frame::text(probe.to_string())).await.unwrap();
    }

    async fn reply_permissions(&self, listener: &TcpListener, ban: bool) {
        let mut permissions = Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES;
        permissions.set(Permissions::BAN_MEMBERS, ban);
        let guild = self.guild.to_string();
        let guild_body = json!({
            "id":guild, "owner_id":"1", "name":"fixture", "afk_timeout":60,
            "default_message_notifications":0, "explicit_content_filter":0, "features":[],
            "mfa_level":0, "nsfw_level":0, "preferred_locale":"ja", "verification_level":0,
            "premium_progress_bar_enabled":false, "system_channel_flags":0,
            "roles":[{"id":guild,"name":"@everyone","color":0,"colors":{"primary_color":0},
                "hoist":false,"managed":false,"mentionable":false,"position":0,"flags":0,
                "permissions":permissions.bits().to_string()}]
        });
        reply(listener, &format!("guilds/{guild}"), guild_body).await;
        let channel = self.channel.to_string();
        let channel_body =
            json!({"id":channel,"guild_id":guild,"type":0,"permission_overwrites":[]});
        reply(listener, &format!("channels/{channel}"), channel_body).await;
    }
}

async fn reply(listener: &TcpListener, path: &str, body: Value) {
    let (socket, _) = listener.accept().await.unwrap();
    let mut request = BufReader::new(socket);
    let mut line = String::new();
    request.read_line(&mut line).await.unwrap();
    assert_eq!(line.trim_end(), format!("GET /api/v10/{path} HTTP/1.1"));
    while line != "\r\n" {
        line.clear();
        assert_ne!(request.read_line(&mut line).await.unwrap(), 0);
    }
    let body = body.to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    request
        .into_inner()
        .write_all(response.as_bytes())
        .await
        .unwrap();
}
