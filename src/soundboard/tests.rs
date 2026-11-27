use std::time::Duration;

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::timeout,
};

use super::*;

async fn reply(
    listener: &TcpListener,
    method_path: &str,
    expected: Option<Value>,
    body: Option<Value>,
) {
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut request = BufReader::new(&mut socket);
    let mut line = String::new();
    request.read_line(&mut line).await.unwrap();
    assert_eq!(line.trim_end(), format!("{method_path} HTTP/1.1"));
    let mut length = 0;
    let mut authorization = false;
    loop {
        line.clear();
        assert_ne!(request.read_line(&mut line).await.unwrap(), 0);
        if line == "\r\n" {
            break;
        }
        let (key, value) = line.split_once(':').unwrap();
        if key.eq_ignore_ascii_case("authorization") {
            assert_eq!(value.trim(), "Bot fixture");
            authorization = true;
        } else if key.eq_ignore_ascii_case("content-length") {
            length = value.trim().parse().unwrap();
        }
    }
    assert!(authorization);
    let mut bytes = vec![0; length];
    request.read_exact(&mut bytes).await.unwrap();
    let received = (!bytes.is_empty()).then(|| serde_json::from_slice(&bytes).unwrap());
    assert_eq!(received, expected);
    let response = match body {
        Some(body) => {
            let body = body.to_string();
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        }
        None => "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_owned(),
    };
    socket.write_all(response.as_bytes()).await.unwrap();
}

#[tokio::test]
async fn soundboard_contract_uses_shared_client_and_exact_routes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = Client::builder()
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .token("fixture".into())
        .build();
    let server =
        async {
            reply(&listener, "GET /api/v10/soundboard-default-sounds", None, Some(json!([
            {"sound_id":"1", "name":"quack", "volume":1, "emoji_name":"🦆", "available":true}
        ]))).await;
            reply(
                &listener,
                "GET /api/v10/guilds/7/soundboard-sounds",
                None,
                Some(json!({"items":[
                    {"sound_id":"12", "guild_id":"7", "name":"bird", "volume":0.5, "available":true}
                ]})),
            )
            .await;
            reply(
                &listener,
                "POST /api/v10/channels/9/send-soundboard-sound",
                Some(json!({"sound_id":"1"})),
                None,
            )
            .await;
            reply(
                &listener,
                "POST /api/v10/channels/9/send-soundboard-sound",
                Some(json!({"sound_id":"12","source_guild_id":"7"})),
                None,
            )
            .await;
        };
    let client = async {
        let default = default_sounds(&http).await.unwrap();
        assert_eq!(default[0].name, "quack");
        let guild = guild_sounds(&http, Id::new(7)).await.unwrap();
        assert_eq!(guild[0].guild_id, Some(Id::new(7)));
        play(&http, Id::new(9), default[0].sound_id, None)
            .await
            .unwrap();
        play(&http, Id::new(9), guild[0].sound_id, guild[0].guild_id)
            .await
            .unwrap();
    };
    timeout(Duration::from_secs(5), async {
        tokio::join!(server, client);
    })
    .await
    .unwrap();
}
