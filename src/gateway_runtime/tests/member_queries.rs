use super::*;

#[tokio::test]
async fn full_member_query_orders_chunks_and_reports_discord_rate_limits() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (ready, mut started) = mpsc::channel(1);
    let mut events = Events::new();
    events.on_ready(async move |_: &Ready| ready.send(()).await);
    let (bot, query) = Bot::new(FIXTURE_TOKEN)
        .unwrap()
        .add_intents(Intents::GUILD_MEMBERS | Intents::GUILD_PRESENCES)
        .with_events(events)
        .with_member_query();
    let (mut bot, shutdown) = bot.with_shutdown();
    bot.gateway_test_port = Some(address.port());

    let gateway = async {
        let mut socket = accept(&listener).await;
        let identify = receive_opcode(&mut socket, 2).await;
        assert_eq!(
            identify["d"]["intents"],
            (Intents::GUILD_MEMBERS | Intents::GUILD_PRESENCES).bits()
        );
        send_ready(
            &mut socket,
            &format!("ws://{address}"),
            "all-members",
            [0, 1],
        )
        .await;

        let request = receive_opcode(&mut socket, 8).await;
        assert_eq!(request["d"]["query"], "");
        assert_eq!(request["d"]["limit"], 0);
        assert_eq!(request["d"]["presences"], true);
        assert_eq!(request["d"]["guild_id"], "4");
        let nonce = request["d"]["nonce"].as_str().unwrap();
        let mut second = member_query_chunk(nonce, "7", &[]);
        second["chunk_index"] = json!(1);
        second["chunk_count"] = json!(2);
        send_dispatch(&mut socket, 2, "GUILD_MEMBERS_CHUNK", second).await;

        let mut first = member_query_chunk(nonce, "6", &[]);
        first["chunk_count"] = json!(2);
        first["presences"] = json!([{
            "guild_id":"4", "user":{"id":"6"}, "status":"online",
            "activities":[], "client_status":{"desktop":"online"}
        }]);
        send_dispatch(&mut socket, 3, "GUILD_MEMBERS_CHUNK", first).await;

        let request = receive_opcode(&mut socket, 8).await;
        assert_eq!(request["d"]["guild_id"], "5");
        send_dispatch(
            &mut socket,
            4,
            "RATE_LIMITED",
            json!({
                "opcode":8, "retry_after":42.25,
                "meta":{"guild_id":"5", "nonce":request["d"]["nonce"]}
            }),
        )
        .await;
        expect_close(&mut socket, 1000).await;
    };
    let queries = async {
        assert_eq!(started.recv().await, Some(()));
        let deadline = Duration::from_secs(3);
        let result = query
            .query_all_with_presences(Id::new(4), 2000, deadline)
            .await
            .unwrap();
        assert_eq!(result.members().len(), 2);
        assert_eq!(result.members()[0].user.id, Id::new(6));
        assert_eq!(result.members()[1].user.id, Id::new(7));
        assert_eq!(result.presences().len(), 1);

        assert!(matches!(
            query.query_all(Id::new(4), 2000, deadline).await,
            Err(crate::MemberQueryError::RateLimited { .. })
        ));
        assert!(matches!(
            query.query_all(Id::new(5), 2000, deadline).await,
            Err(crate::MemberQueryError::RateLimited { retry_after })
                if retry_after == Duration::from_millis(42_250)
        ));
        shutdown.request();
    };
    let (result, (), ()) = timeout(Duration::from_secs(10), async {
        tokio::join!(bot.start(), gateway, queries)
    })
    .await
    .unwrap();
    assert!(result.is_ok(), "{result:?}");
}
