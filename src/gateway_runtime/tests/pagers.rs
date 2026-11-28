use super::{
    confirmations::{button, interaction_request},
    *,
};
use crate::{PagerLimits, PagerPage, Pagers, ui::UiMessage};
use twilight_model::channel::message::Component;

#[tokio::test]
async fn pager_routes_only_owner_and_edits_existing_message() {
    let gateway = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = gateway.local_addr().unwrap();
    let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let pages = Pagers::new(PagerLimits::default()).unwrap();
    let mut events = Events::new();
    pages.register(&mut events).unwrap();
    let draft = pages
        .begin(
            Id::new(20),
            Id::new(6),
            Duration::from_secs(10),
            vec![
                UiMessage::new().content("first page").into(),
                UiMessage::new().content("second page").into(),
            ],
        )
        .unwrap();
    let PagerPage::Classic(first) = draft.page() else {
        panic!("expected classic page");
    };
    let next = first
        .components
        .iter()
        .find_map(|component| {
            let Component::ActionRow(row) = component else {
                return None;
            };
            row.components.iter().find_map(|child| {
                let Component::Button(button) = child else {
                    return None;
                };
                (!button.disabled)
                    .then(|| button.custom_id.clone())
                    .flatten()
            })
        })
        .unwrap();
    let original: Value = serde_json::from_str(MESSAGE).unwrap();
    let original = serde_json::from_value(original["d"].clone()).unwrap();
    let pager = draft.bind(Some(Id::new(4)), &original).unwrap();
    assert_eq!(pager.position(), Some((1, 2)));
    let (mut bot, shutdown) = Bot::new(FIXTURE_TOKEN)
        .unwrap()
        .with_events(events)
        .with_shutdown();
    bot.gateway_test_port = Some(address.port());
    bot.http_test_port = Some(http.local_addr().unwrap().port());

    let (refused, mut refusal) = mpsc::channel(1);
    let server = async {
        let mut socket = accept(&gateway).await;
        receive_opcode(&mut socket, 2).await;
        send_ready(&mut socket, &format!("ws://{address}"), "pager", [0, 1]).await;
        let mut wrong_owner = button("21", &next);
        wrong_owner["member"]["user"]["id"] = json!("7");
        send_dispatch(&mut socket, 2, "INTERACTION_CREATE", wrong_owner).await;
        assert_eq!(refusal.recv().await, Some(()));
        send_dispatch(&mut socket, 3, "INTERACTION_CREATE", button("22", &next)).await;
        expect_close(&mut socket, 1000).await;
    };
    let callbacks = async {
        for (id, kind) in [("21", 4), ("22", 6)] {
            let (line, payload, mut socket) = interaction_request(&http).await;
            assert!(line.starts_with("POST ") && line.contains(&format!("/interactions/{id}/")));
            assert_eq!(payload["type"], kind);
            if kind == 4 {
                assert_eq!(payload["data"]["flags"], 64);
            }
            socket
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            if kind == 4 {
                refused.send(()).await.unwrap();
            }
        }
        let (line, payload, mut socket) = interaction_request(&http).await;
        assert!(
            line.starts_with("PATCH ")
                && line.contains("/webhooks/2/local-confirmation-token/messages/@original")
        );
        assert_eq!(payload["content"], "second page");
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .await
            .unwrap();
        loop {
            if pager.position() == Some((2, 2)) {
                shutdown.request();
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    let (result, (), ()) = timeout(Duration::from_secs(6), async {
        tokio::join!(bot.start(), server, callbacks)
    })
    .await
    .unwrap();
    assert!(result.is_ok(), "{result:?}");
}
