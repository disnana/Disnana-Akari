use tokio::io::AsyncReadExt;

use super::*;
use crate::{ConfirmationLimits, ConfirmationOutcome, Limits};

#[tokio::test]
async fn confirmation_completes_while_the_only_interaction_worker_is_waiting() {
    let gateway = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = gateway.local_addr().unwrap();
    let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (identifier, mut identifiers) = mpsc::channel(1);
    let (finished, mut outcomes) = mpsc::channel(1);
    let (bot, confirmations) = Bot::new(FIXTURE_TOKEN)
        .unwrap()
        .limits(Limits::default().interaction_workers(1))
        .with_confirmations(ConfirmationLimits::default())
        .unwrap();
    let mut events = Events::new();
    events
        .on_button(
            "open",
            async move |ctx: &mut crate::ButtonContext<'_>| -> crate::Result<()> {
                let session = confirmations
                    .begin(
                        ctx.interaction_id(),
                        ctx.author_id().ok_or_else(|| Error::msg("missing owner"))?,
                        Duration::from_secs(5),
                    )
                    .map_err(Error::handler)?;
                session
                    .bind(
                        ctx.guild_id(),
                        ctx.message().ok_or_else(|| Error::msg("missing source"))?,
                    )
                    .map_err(Error::handler)?;
                ctx.acknowledge().await?;
                identifier
                    .send(session.confirm_id().to_owned())
                    .await
                    .map_err(Error::handler)?;
                finished
                    .send(session.wait().await)
                    .await
                    .map_err(Error::handler)
            },
        )
        .unwrap();
    let (mut bot, shutdown) = bot.with_events(events).with_shutdown();
    bot.gateway_test_port = Some(address.port());
    bot.http_test_port = Some(http.local_addr().unwrap().port());
    let server = async {
        let mut socket = accept(&gateway).await;
        receive_opcode(&mut socket, 2).await;
        send_ready(&mut socket, &format!("ws://{address}"), "confirm", [0, 1]).await;
        send_dispatch(&mut socket, 2, "INTERACTION_CREATE", button("21", "open")).await;
        let id = identifiers.recv().await.unwrap();
        send_dispatch(&mut socket, 3, "INTERACTION_CREATE", button("22", &id)).await;
        assert_eq!(outcomes.recv().await, Some(ConfirmationOutcome::Confirmed));
        shutdown.request();
        expect_close(&mut socket, 1000).await;
    };
    let callbacks = async {
        acknowledge(&http, "21").await;
        acknowledge(&http, "22").await;
    };
    let (result, (), ()) = timeout(Duration::from_secs(10), async {
        tokio::join!(bot.start(), server, callbacks)
    })
    .await
    .unwrap();
    assert!(result.is_ok(), "{result:?}");
}

pub(super) fn button(id: &str, custom_id: &str) -> Value {
    let mut original: Value = serde_json::from_str(MESSAGE).unwrap();
    let payload = json!({
        "id":id, "application_id":"2", "type":3, "version":1,
        "token":"local-confirmation-token", "attachment_size_limit":10485760, "authorizing_integration_owners":{"0":"4"}, "guild_id":"4", "channel_id":"5",
        "member":{"user":{"id":"6","username":"member","discriminator":"0"}, "roles":[], "flags":0, "permissions":"0", "deaf":false, "mute":false},
        "message":original["d"].take(), "data":{"custom_id":custom_id,"component_type":2}
    });
    serde_json::from_value::<twilight_model::application::interaction::Interaction>(
        payload.clone(),
    )
    .unwrap();
    payload
}

async fn acknowledge(listener: &TcpListener, id: &str) {
    let (line, payload, mut socket) = interaction_request(listener).await;
    assert!(
        line.starts_with("POST ")
            && line.contains(&format!(
                "/interactions/{id}/local-confirmation-token/callback"
            ))
    );
    assert_eq!(payload["type"], 6);
    socket
        .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
}

pub(super) async fn interaction_request(listener: &TcpListener) -> (String, Value, TcpStream) {
    let (socket, _) = listener.accept().await.unwrap();
    let mut request = BufReader::new(socket);
    let mut path = String::new();
    request.read_line(&mut path).await.unwrap();
    let mut length = None;
    let mut line = String::new();
    loop {
        line.clear();
        assert_ne!(request.read_line(&mut line).await.unwrap(), 0);
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = Some(value.trim().parse::<usize>().unwrap());
        }
    }
    let mut body = vec![0; length.unwrap()];
    request.read_exact(&mut body).await.unwrap();
    let payload = serde_json::from_slice(&body).unwrap();
    (path, payload, request.into_inner())
}
