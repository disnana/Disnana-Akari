use std::{sync::Arc, time::Instant};

use tokio::sync::Semaphore;

use super::*;
use crate::{CommandContext, Commands, Limits, QueueKind, model::channel::Message, work_queue};

const WORKERS: usize = 4;
const QUEUE_CAPACITY: usize = 64;
const BURST: usize = 256;
const QUEUE_SAMPLES: usize = 500;
const GATEWAY_SAMPLES: usize = 50;
const DEADLINE: Duration = Duration::from_secs(20);

#[tokio::test]
#[ignore = "manual release-mode measurement"]
async fn work_queue_latency_and_backpressure() {
    let (jobs, incoming) = mpsc::channel(QUEUE_CAPACITY);
    let (started, mut starts) = mpsc::channel(WORKERS);
    let (rejected, mut rejections) = mpsc::channel(QUEUE_CAPACITY);
    let release = Semaphore::new(0);
    let worker = work_queue::dispatch(
        incoming,
        WORKERS,
        |(queued_at, hold): (Instant, bool)| {
            let started = &started;
            let release = &release;
            async move {
                started.try_send(queued_at.elapsed()).unwrap();
                if hold {
                    let _permit = release.acquire().await.unwrap();
                }
            }
        },
        |(queued_at, _)| rejected.try_send(queued_at.elapsed()).unwrap(),
    );
    let measure = async {
        let mut idle = Vec::with_capacity(QUEUE_SAMPLES);
        for _ in 0..QUEUE_SAMPLES {
            jobs.try_send((Instant::now(), false)).unwrap();
            idle.push(starts.recv().await.unwrap());
        }
        report("queue.idle_enqueue_to_start", idle);

        for _ in 0..WORKERS {
            jobs.try_send((Instant::now(), true)).unwrap();
        }
        for _ in 0..WORKERS {
            starts.recv().await.unwrap();
        }

        // 同一タスクから await せずに投入し、送信側と実行枠側の拒否を区別する。
        let mut ingress_full = Vec::with_capacity(BURST - QUEUE_CAPACITY);
        for _ in 0..BURST {
            let offered_at = Instant::now();
            match jobs.try_send((offered_at, false)) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => ingress_full.push(offered_at.elapsed()),
                Err(error) => panic!("work queue closed during measurement: {error}"),
            }
        }
        assert_eq!(ingress_full.len(), BURST - QUEUE_CAPACITY);
        let mut worker_busy = Vec::with_capacity(QUEUE_CAPACITY);
        for _ in 0..QUEUE_CAPACITY {
            worker_busy.push(rejections.recv().await.unwrap());
        }
        println!(
            "perf queue workers={WORKERS} capacity={QUEUE_CAPACITY} offered={BURST} admitted=0 ingress_full={} worker_busy={}",
            ingress_full.len(),
            worker_busy.len(),
        );
        report("queue.ingress_full_reject", ingress_full);
        report("queue.worker_busy_reject", worker_busy);

        release.add_permits(WORKERS);
        drop(jobs);
    };
    timeout(DEADLINE, async { tokio::join!(worker, measure) })
        .await
        .expect("work queue measurement completed");
}

#[tokio::test]
#[ignore = "manual release-mode measurement over loopback WebSocket"]
async fn gateway_heartbeat_and_handlers_under_text_contention() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (ready, mut ready_rx) = mpsc::channel(1);
    let (probed, mut probes) = mpsc::channel(1);
    let (started, mut starts) = mpsc::channel(WORKERS);
    let (rejected, mut rejections) = mpsc::channel(BURST);
    let release = Arc::new(Semaphore::new(0));
    let permits = Arc::clone(&release);

    let mut events = Events::new();
    events.on_ready(async move |_: &Ready| ready.try_send(()));
    events.on_message(async move |message: &Message| {
        if message.content == "perf-probe" {
            probed.try_send((message.id.get(), Instant::now()))?;
        }
        Ok::<(), mpsc::error::TrySendError<_>>(())
    });
    let mut commands = Commands::new("!").unwrap();
    commands
        .register(
            "hold",
            async move |ctx: CommandContext<'_>| -> crate::Result<()> {
                started
                    .try_send(ctx.timing().queue_wait())
                    .map_err(Error::handler)?;
                let _permit = permits.acquire().await.map_err(Error::handler)?;
                Ok(())
            },
        )
        .unwrap();

    let (mut bot, shutdown) = Bot::new(FIXTURE_TOKEN)
        .unwrap()
        .with_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)
        .limits(
            Limits::default()
                .text_workers(WORKERS)
                .queue_capacity(QUEUE_CAPACITY),
        )
        .with_events(events)
        .on_error(move |report| {
            if matches!(report.error, Error::QueueFull(QueueKind::TextCommand)) {
                rejected.try_send(Instant::now()).unwrap();
            }
        })
        .with_shutdown();
    bot.gateway_test_port = Some(address.port());
    // シナリオに REST 呼び出しが加わった場合も接続先はローカルに限定する。
    bot.http_test_port = Some(address.port());

    let gateway = async move {
        let mut socket = accept(&listener).await;
        receive_opcode(&mut socket, 2).await;
        send_ready(
            &mut socket,
            &format!("ws://{address}"),
            "performance",
            [0, 1],
        )
        .await;
        ready_rx.recv().await.unwrap();

        let mut sequence = 1;
        measure_gateway(&mut socket, &mut probes, &mut sequence, "idle").await;

        // テキストの処理枠が埋まっている間の、独立したイベント経路と Gateway を計測する。
        for _ in 0..WORKERS {
            send_sample(&mut socket, &mut sequence, "!hold").await;
        }
        let mut accepted = Vec::with_capacity(WORKERS);
        for _ in 0..WORKERS {
            accepted.push(starts.recv().await.unwrap());
        }
        report("gateway.text_dispatch_queue_wait", accepted);
        measure_gateway(&mut socket, &mut probes, &mut sequence, "busy").await;

        let burst_start = Instant::now();
        for _ in 0..BURST {
            send_sample(&mut socket, &mut sequence, "!hold").await;
        }
        report(
            "gateway.burst_heartbeat_request_to_response",
            vec![heartbeat_response(&mut socket, sequence).await],
        );
        let mut busy = Vec::with_capacity(BURST);
        for _ in 0..BURST {
            busy.push(rejections.recv().await.unwrap().duration_since(burst_start));
        }
        println!(
            "perf gateway workers={WORKERS} offered={BURST} admitted=0 text_busy={}",
            busy.len()
        );
        report("gateway.text_reject_since_burst_start", busy);

        release.add_permits(WORKERS);
        shutdown.request();
        expect_close(&mut socket, 1000).await;
    };
    let (_, result) = timeout(DEADLINE, async {
        tokio::join!(gateway, bot.run_commands(commands))
    })
    .await
    .expect("loopback benchmark completed");
    result.unwrap();
}

async fn measure_gateway(
    socket: &mut WebSocketStream<TcpStream>,
    probes: &mut mpsc::Receiver<(u64, Instant)>,
    sequence: &mut usize,
    phase: &str,
) {
    let mut probe_latency = Vec::with_capacity(GATEWAY_SAMPLES);
    let mut heartbeat_latency = Vec::with_capacity(GATEWAY_SAMPLES);
    for _ in 0..GATEWAY_SAMPLES {
        let probe_started_at = Instant::now();
        send_sample(socket, sequence, "perf-probe").await;
        let (id, observed_at) = probes.recv().await.unwrap();
        assert_eq!(id, *sequence as u64);
        probe_latency.push(observed_at.duration_since(probe_started_at));

        heartbeat_latency.push(heartbeat_response(socket, *sequence).await);
    }
    report(
        &format!("gateway.{phase}_dispatch_to_event_handler"),
        probe_latency,
    );
    report(
        &format!("gateway.{phase}_heartbeat_request_to_response"),
        heartbeat_latency,
    );
}

async fn heartbeat_response(socket: &mut WebSocketStream<TcpStream>, sequence: usize) -> Duration {
    let requested_at = Instant::now();
    socket
        .send(Frame::text(r#"{"op":1,"d":null}"#))
        .await
        .unwrap();
    loop {
        let heartbeat = receive_opcode(socket, 1).await;
        let elapsed = requested_at.elapsed();
        socket.send(Frame::text(ACK)).await.unwrap();
        // 該当する連番まで、前のイベントを処理した時点での周期送信は計測から外す。
        if heartbeat["d"] == sequence {
            return elapsed;
        }
    }
}

async fn send_sample(socket: &mut WebSocketStream<TcpStream>, sequence: &mut usize, content: &str) {
    *sequence += 1;
    let mut payload: Value = serde_json::from_str(MESSAGE).unwrap();
    payload["d"]["id"] = sequence.to_string().into();
    payload["d"]["content"] = content.into();
    send_dispatch(socket, *sequence, "MESSAGE_CREATE", payload["d"].take()).await;
}

fn report(metric: &str, mut samples: Vec<Duration>) {
    samples.sort_unstable();
    let percentile = |percent: usize| {
        samples[(samples.len() * percent).div_ceil(100) - 1].as_secs_f64() * 1_000_000.0
    };
    println!(
        "perf metric={metric} n={} p50_us={:.2} p95_us={:.2} p99_us={:.2} max_us={:.2} debug_assertions={}",
        samples.len(),
        percentile(50),
        percentile(95),
        percentile(99),
        percentile(100),
        cfg!(debug_assertions),
    );
}
