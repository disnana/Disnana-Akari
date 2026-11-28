use std::num::NonZeroU16;

use super::*;
use crate::StartupWarmup;

#[tokio::test]
async fn startup_readiness_waits_for_guilds_and_configured_member_bootstrap() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (ready, mut raw_ready) = mpsc::channel(1);
    let mut events = Events::new();
    events.on_ready(async move |_: &Ready| ready.send(()).await);
    let (bot, cache) = Bot::new(FIXTURE_TOKEN)
        .unwrap()
        .add_intents(Intents::GUILDS | Intents::GUILD_MEMBERS)
        .with_events(events)
        .with_member_cache(50);
    let (bot, startup) = bot
        .with_member_cache_bootstrap([Id::new(4)], NonZeroU16::new(10).unwrap())
        .with_startup_readiness(Duration::from_secs(3), true)
        .unwrap();
    let (mut bot, shutdown) = bot.with_shutdown();
    bot.gateway_test_port = Some(address.port());
    bot.http_test_port = Some(http.local_addr().unwrap().port());

    let gateway = async {
        let mut socket = accept(&listener).await;
        receive_opcode(&mut socket, 2).await;
        send_ready_with_guilds(
            &mut socket,
            &format!("ws://{address}"),
            "initial-warmup",
            [0, 1],
            json!([{"id":"4", "unavailable":true}, {"id":"5", "unavailable":true}]),
        )
        .await;
        assert_eq!(raw_ready.recv().await, Some(()));
        assert!(startup.snapshot().is_none());

        let mut guild = available_guild_payload(4);
        guild["member_count"] = 1.into();
        send_dispatch(&mut socket, 2, "GUILD_CREATE", guild).await;
        send_dispatch(
            &mut socket,
            3,
            "GUILD_DELETE",
            json!({"id":"5","unavailable":true}),
        )
        .await;
        expect_close(&mut socket, 1000).await;
    };
    let requests = async {
        let (socket, _) = http.accept().await.unwrap();
        let mut request = BufReader::new(socket);
        let mut line = String::new();
        request.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("GET /api/v10/guilds/4/members"));
        loop {
            line.clear();
            assert_ne!(request.read_line(&mut line).await.unwrap(), 0);
            if line == "\r\n" {
                break;
            }
        }
        assert!(startup.snapshot().is_none());
        let body = member_query_chunk("fixture", "6", &[])["members"].to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        request
            .get_mut()
            .write_all(response.as_bytes())
            .await
            .unwrap();

        let report = startup.wait().await.unwrap();
        assert!(!report.timed_out());
        assert_eq!(report.warmup(), StartupWarmup::Settled);
        assert_eq!(report.shards().len(), 1);
        assert_eq!(report.shards()[0].available_guilds(), 1);
        assert_eq!(report.shards()[0].unavailable_guilds(), &[Id::new(5)]);
        assert!(report.shards()[0].unresolved_guilds().is_empty());
        assert_eq!(
            cache.get(Id::new(4), Id::new(6)).unwrap().user.id,
            Id::new(6)
        );
        shutdown.request();
    };
    let (result, (), ()) = timeout(Duration::from_secs(8), async {
        tokio::join!(bot.start(), gateway, requests)
    })
    .await
    .unwrap();
    assert!(result.is_ok(), "{result:?}");
}

#[tokio::test]
async fn startup_readiness_requires_explicit_settings_and_closes_on_early_exit() {
    let (bot, startup) = Bot::new(FIXTURE_TOKEN)
        .unwrap()
        .with_startup_readiness(Duration::from_secs(2), false)
        .unwrap();
    assert!(
        matches!(bot.start().await, Err(Error::MissingEventIntents { any_of }) if any_of == Intents::GUILDS)
    );
    assert!(startup.wait().await.is_none());

    let (bot, startup) = Bot::new(FIXTURE_TOKEN)
        .unwrap()
        .add_intents(Intents::GUILDS)
        .with_startup_readiness(Duration::from_secs(2), true)
        .unwrap();
    assert!(matches!(
        bot.start().await,
        Err(Error::StartupMemberBootstrapRequired)
    ));
    assert!(startup.wait().await.is_none());
}
