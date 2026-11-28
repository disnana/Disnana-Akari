use super::{
    confirmations::{button, interaction_request},
    *,
};
use crate::{
    ButtonContext, SelectContext, UiSession, UiSessionActions, UiSessionLimits, UiSessionOutcome,
    UiSessions,
};

#[tokio::test]
async fn ui_session_routes_select_and_finish_and_closes_remaining_actions_on_shutdown() {
    let gateway = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = gateway.local_addr().unwrap();
    let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sessions = UiSessions::new(UiSessionLimits::default()).unwrap();
    let mut events = Events::new();
    sessions.register(&mut events).unwrap();
    let (selected, mut selection) = mpsc::channel(1);
    let mut actions = UiSessionActions::new();
    actions
        .on_select(
            "color",
            async move |ctx: &mut SelectContext<'_>, _: UiSession| {
                assert_eq!(ctx.values(), &["blue"]);
                ctx.reply_ephemeral("selected blue").await?;
                selected.try_send(()).map_err(Error::handler)
            },
        )
        .unwrap();
    actions
        .on_button(
            "finish",
            async |ctx: &mut ButtonContext<'_>, session: UiSession| {
                ctx.reply_ephemeral("closed by owner").await?;
                assert!(session.finish());
                Ok(())
            },
        )
        .unwrap();
    let owner = Id::new(6);
    let draft = sessions
        .begin(Id::new(20), owner, Duration::from_secs(10), actions)
        .unwrap();
    let color = draft.custom_id("color").unwrap();
    let finish = draft.custom_id("finish").unwrap();
    let mut original: Value = serde_json::from_str(MESSAGE).unwrap();
    let session = draft
        .bind(
            Some(Id::new(4)),
            &serde_json::from_value(original["d"].clone()).unwrap(),
        )
        .unwrap();
    let mut outcome = session.subscribe();

    let mut other_actions = UiSessionActions::new();
    other_actions
        .on_button("finish", async |_: &mut ButtonContext<'_>, _: UiSession| {
            Ok(())
        })
        .unwrap();
    original["d"]["id"] = json!("11");
    let other = sessions
        .begin(Id::new(21), owner, Duration::from_secs(10), other_actions)
        .unwrap()
        .bind(
            Some(Id::new(4)),
            &serde_json::from_value(original["d"].clone()).unwrap(),
        )
        .unwrap();

    let (mut bot, shutdown) = Bot::new(FIXTURE_TOKEN)
        .unwrap()
        .with_events(events)
        .with_shutdown();
    bot.gateway_test_port = Some(address.port());
    bot.http_test_port = Some(http.local_addr().unwrap().port());
    let (accepted, mut replies) = mpsc::channel(1);
    let gateway = async {
        let mut socket = accept(&gateway).await;
        receive_opcode(&mut socket, 2).await;
        send_ready(
            &mut socket,
            &format!("ws://{address}"),
            "ui-session",
            [0, 1],
        )
        .await;
        let mut wrong_owner = button("22", finish.as_str());
        wrong_owner["member"]["user"]["id"] = json!("7");
        send_dispatch(&mut socket, 2, "INTERACTION_CREATE", wrong_owner).await;
        assert_eq!(replies.recv().await, Some(()));
        let mut select = button("23", color.as_str());
        select["data"]["component_type"] = json!(3);
        select["data"]["values"] = json!(["blue"]);
        send_dispatch(&mut socket, 3, "INTERACTION_CREATE", select).await;
        assert_eq!(selection.recv().await, Some(()));
        send_dispatch(
            &mut socket,
            4,
            "INTERACTION_CREATE",
            button("24", finish.as_str()),
        )
        .await;
        expect_close(&mut socket, 1000).await;
    };
    let callbacks = async {
        for (id, expected) in [
            ("22", "この画面は操作できません。"),
            ("23", "selected blue"),
            ("24", "closed by owner"),
        ] {
            let (line, body, mut socket) = interaction_request(&http).await;
            assert!(line.starts_with("POST ") && line.contains(&format!("/interactions/{id}/")));
            assert_eq!(body["type"], 4);
            assert_eq!(body["data"]["content"], expected);
            assert_eq!(body["data"]["flags"], 64);
            socket
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            if id == "22" {
                accepted.send(()).await.unwrap();
            }
        }
        assert_eq!(
            *outcome.wait_for(Option::is_some).await.unwrap(),
            Some(UiSessionOutcome::Finished)
        );
        shutdown.request();
    };
    let (result, (), ()) = timeout(Duration::from_secs(6), async {
        tokio::join!(bot.start(), gateway, callbacks)
    })
    .await
    .unwrap();
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(session.outcome(), Some(UiSessionOutcome::Finished));
    assert_eq!(other.outcome(), Some(UiSessionOutcome::Closed));
}
