# 定期処理

Bot の接続中に集計やメンテナンスを行うときは、Tokio のタイマーで処理を繰り返せます。discord.py の `tasks.loop`、`before_loop`、`stop` にあたる動作を、タイマーと通知で組み立てます。開始条件、停止の順序、再試行するエラーはアプリで決めます。[discord.py の定期処理](https://discordpy.readthedocs.io/en/stable/ext/tasks/)

## Ready を待ち、実行中の処理が終わってから停止する

次の例は、Gateway が最初に Ready になったあとに、集計を 60 秒ごとに実行します。`on_ready` は準備を通知するだけなので、再接続で Ready が再び届いてもループは 1 本です。`events` には組み立て済みのイベント一覧、`stop` にはアプリの終了通知が届くと完了する Future を渡します。アプリ側の Tokio では `macros`、`rt`、`sync`、`time` 機能を使います。[モジュールで登録をまとめる](modules.md)

```rust
use std::{future::Future, io, time::Duration};

use disnana_akari::{Bot, Error, Events, Ready, Result};
use tokio::{
    sync::{oneshot, watch},
    time::{self, MissedTickBehavior},
};

async fn run_until(
    bot: Bot,
    mut events: Events,
    stop: impl Future<Output = ()>,
    collect: impl AsyncFnMut() -> io::Result<()>,
) -> Result<()> {
    let (ready, mut ready_rx) = watch::channel(false);
    let (stop_worker, worker_stop) = oneshot::channel();
    events.on_ready(async move |_: &Ready| -> Result<()> {
        ready.send_replace(true);
        Ok(())
    });

    let (bot, shutdown) = bot.with_events(events).with_shutdown();
    let running = bot.start();
    let worker = periodic(&mut ready_rx, worker_stop, collect);
    tokio::pin!(running, worker);

    tokio::select! {
        result = &mut running => {
            let _ = stop_worker.send(());
            result.and(worker.await.map_err(Error::handler))
        }
        () = stop => {
            let _ = stop_worker.send(());
            shutdown.request();
            let (result, work) = tokio::join!(running, worker);
            result.and(work.map_err(Error::handler))
        }
        work = &mut worker => {
            shutdown.request();
            work.map_err(Error::handler).and(running.await)
        }
    }
}

async fn periodic(
    ready: &mut watch::Receiver<bool>,
    mut stop: oneshot::Receiver<()>,
    mut collect: impl AsyncFnMut() -> io::Result<()>,
) -> io::Result<()> {
    tokio::select! {
        biased;
        _ = &mut stop => return Ok(()),
        ready = ready.wait_for(|is_ready| *is_ready) => {
            if ready.is_err() {
                return Ok(());
            }
        }
    }

    let mut interval = time::interval(Duration::from_secs(60));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = &mut stop => return Ok(()),
            _ = interval.tick() => {}
        }

        let mut delay = Duration::from_secs(1);
        loop {
            // 実行中に停止通知が届いても、この 1 回の処理は最後まで待ちます。
            match collect().await {
                Ok(()) => break,
                Err(error) if error.kind() == io::ErrorKind::TimedOut
                    && delay < Duration::from_secs(8) => {}
                Err(error) => return Err(error),
            }

            tokio::select! {
                biased;
                _ = &mut stop => return Ok(()),
                _ = time::sleep(delay) => {}
            }
            delay *= 2;
        }
    }
}
```

最初の実行は Ready の直後です。集計が長引いた場合は `MissedTickBehavior::Skip` で、過ぎた間隔の分をまとめて実行するのを避けます。Ready、次の実行時刻、再試行を待っている間に停止通知が届くと、その場で終了します。すでに集計が始まっていれば、その回の処理が戻ってから停止します。Bot が先に終了した場合も集計へ停止を通知し、完了を待ちます。[Tokio の Interval](https://docs.rs/tokio/latest/tokio/time/struct.Interval.html)

テキストコマンドと組み合わせる場合は、`bot.start()` の箇所を `bot.run_commands(commands)` に替えます。

### 一時的なエラーを再試行する

この例では `TimedOut` のときだけ、1 秒、2 秒、4 秒と待ち時間を延ばして最大 3 回再試行します。上限に達した場合やほかのエラーでは定期処理を終え、`run_until` が Bot に停止を要求してエラーを返します。失敗の種類は実際に使うサービスに合わせ、繰り返しても結果が重複しない操作に限って再試行してください。タイムアウト前に書き込みが完了していた可能性がある場合は、識別子による重複防止や結果の照合を処理に組み込みます。

停止時間の上限が必要な場合は、`collect` の I/O に制限時間を設定します。この例は実行中の `collect` が戻るまで待ち続けます。CPU を長く使う処理や同期 I/O は `tokio::task::spawn_blocking` に移します。

## Bot に終了を要求する

システムの終了シグナルなどを受けたら、`ShutdownHandle::request()` を呼び、Bot の起動メソッドを最後まで待ちます。次の例では停止通知に使う Future をアプリから渡します。

```rust
use std::future::Future;
use disnana_akari::{Bot, Result};

async fn run_until(bot: Bot, stop: impl Future<Output = ()>) -> Result<()> {
    let (bot, shutdown) = bot.with_shutdown();
    let running = bot.start();
    tokio::pin!(running);
    tokio::select! {
        result = &mut running => result,
        () = stop => {
            shutdown.request();
            running.await
        }
    }
}
```

終了を要求すると新着の受付を止めます。Gateway の切断には最大2秒、受付済みの処理には最大5秒を取り、両方を並行して待ちます。期限を過ぎた処理は中止されます。自分で起動したバックグラウンドタスクも停止する場合は、別途そのタスクへ通知して完了を待ちます。

## 推奨シャード数で起動する

`with_recommended_shards()` を使うと、認証済みの Gateway Bot エンドポイントを調べ、Discord が推奨する全シャードを同じプロセスで起動します。

```rust
use disnana_akari::{Bot, Events, Ready, Result};

async fn run(bot: Bot) -> Result<()> {
    let mut events = Events::new();
    events.on_ready(async |ready: &Ready| -> Result<()> {
        if let Some(shard) = ready.shard {
            println!("シャード {}/{} が準備できました。", shard.number() + 1, shard.total());
        }
        Ok(())
    });
    bot.with_recommended_shards().with_events(events).start().await
}
```

セッション開始枠が全シャード分残っていない場合は、Gateway に接続する前に起動がエラーになります。REST クライアント、スラッシュコマンドの同期、ハンドラーの並行枠は共有します。RTT は通知を受信したシャードの値です。複数シャードの通知には全体での順序保証はありません。

再接続と Presence の再送はシャードごとに行います。権限キャッシュを使っている場合、切断や再接続で再読み込みを必要とするのは、そのシャードが担当するサーバーです。`ShutdownHandle` で終了を要求すると、接続中のすべてのシャードを並行して閉じます。[Discord のシャーディング仕様](https://docs.discord.com/developers/events/gateway#sharding)

## ボイスチャンネルへの参加と退出を要求する

`Bot::with_voice_gateway` で取得したハンドルをコマンドから呼び出します。Ready 後に使い、`GUILD_VOICE_STATES` Intent は明示的に設定します。

```rust
use disnana_akari::{Bot, Result, VoiceGateway, VoiceStateChange};
use disnana_akari::model::{gateway::Intents, id::{Id, marker::{ChannelMarker, GuildMarker}}};

fn enable_voice(token: &str) -> Result<(Bot, VoiceGateway)> {
    Ok(Bot::new(token)?
        .add_intents(Intents::GUILD_VOICE_STATES)
        .with_voice_gateway())
}

async fn move_bot(
    voice: &VoiceGateway,
    guild: Id<GuildMarker>,
    channel: Id<ChannelMarker>,
) -> Result<()> {
    voice.set_state(guild, VoiceStateChange::join(channel).self_deaf(true)).await?;
    Ok(())
}
```

退出を要求するときは `VoiceStateChange::leave()` を渡します。要求を受け付けた後、Bot 自身の `VoiceStateUpdate` で実際の参加先を確認できます。音声の送受信には別途音声ドライバーが必要です。ドライバーの接続には `VoiceServerUpdate` も受け取り、エンドポイントが無い間は接続を待ちます。イベントに含まれるトークンはログや返信に残さないでください。[Discord の音声接続仕様](https://docs.discord.com/developers/topics/voice-connections#connecting-to-voice)

### 音声サーバーの割り当てを待つ

`join` は、参加先が一致する Bot 自身の音声状態と、サーバーの音声サーバー割り当てが揃うまで待ちます。`leave` は Bot 自身の退出通知を待ちます。

```rust
use std::time::Duration;
use disnana_akari::{Result, VoiceConnection, VoiceGateway};
use disnana_akari::model::id::{Id, marker::{ChannelMarker, GuildMarker}};

async fn join_channel(
    voice: &VoiceGateway,
    guild: Id<GuildMarker>,
    channel: Id<ChannelMarker>,
) -> Result<VoiceConnection> {
    Ok(voice.join(guild, channel, Duration::from_secs(30)).await?)
}
```

`connection.status()` で現在の状態を取得し、`connection.changed().await` で更新を待てます。`Ready(info)` の `info.channel_id` が参加先です。`AwaitingServer` では再割り当てを待ち、`Disconnected` で管理が終了します。`Ready` の接続情報を音声ドライバーに渡して、音声サーバーに接続します。送信キューへ渡した後に参加がタイムアウトした場合、遅れて届く通知との混同を避けるため、次の新規 Gateway セッションまで同じサーバーの参加を再試行できません。その場合も `leave` は使えます。トークンとセッション ID は明示的に取得し、保存やログ出力を避けてください。[Discord の音声サーバー割り当て](https://docs.discord.com/developers/topics/voice-connections#retrieving-voice-server-information)

### WAV を再生・録音する

リポジトリの `examples/voice` に、Akari の接続情報を Songbird に渡して WAV を再生・録音する例があります。Songbird の Twilight 統合は使わず、音声の依存はこの例の Cargo プロジェクトで管理します。

Bot のトークンに加え、`AKARI_VOICE_GUILD`、`AKARI_VOICE_CHANNEL` を環境変数に設定します。再生するには `AKARI_VOICE_WAV` に入力ファイル、録音するには `AKARI_VOICE_RECORD_WAV` に新規ファイルのパスを設定します。リポジトリのルートから次のコマンドで実行します。

```sh
cargo run --locked --manifest-path examples/voice/Cargo.toml
```

再割り当て時は新しい音声サーバーへ接続し直します。録音は受信処理からファイル書き出しを分け、キューが満杯なら破棄したフレーム数を記録します。Ctrl+C で停止し、録音していれば WAV を確定します。[Songbird の音声ドライバー実装](https://github.com/serenity-rs/songbird/blob/v0.6.1/src/driver/mod.rs)

音声を Discord のコマンドから操作するには、`AKARI_VOICE_RECORD_DIR` にローカルの録音先を設定します。参加先を省略して `/vc-join` を呼ぶと、自分が参加中の VC に Bot が接続します。 `/vc-playtest` で短い2音を再生できます。`/vc-record` で録音を始め、`format:FLAC` を選ぶとロスレス圧縮で保存できます。`/vc-stop` で実行先のチャンネルに録音を投稿し、`/vc-leave` で退出します。このサンプルでは合計 10 MiB、最長5分まで録音できます。[Discord の音声状態取得 API](https://docs.discord.com/developers/resources/voice#get-user-voice-state)

受信録音に保存されるのは、Discord の音声接続を通じて Bot に届いた音声です。Bot 自身が再生した音は同じ接続の受信側に戻らず、ミックスにも入りません。`/vc-record` で Bot 自身を個別トラックに選ぶと録音開始前に拒否します。コマンドを実行した参加者自身は録音できます。読み上げなどの再生音も残したい場合は、アプリで送信前の音源を別に保存するか、録音に合成します。[Discord の音声接続仕様](https://docs.discord.com/developers/topics/voice-connections)

## シャード別の状態と RTT を監視する

`let (bot, status) = bot.with_gateway_status();` で監視を設定します。`status.shards()` は、起動待ち、初回接続中、準備済み、再接続中、終了のいずれかと、計測した RTT をシャード番号順に返します。シャード数が決まるまでは空の一覧です。切断したシャードの RTT は、新しい接続でハートビートの応答を受けるまで `None` です。 起動に失敗して状態が更新されない場合に備え、監視は Bot の起動処理と並行し、その処理が終了した時点でも打ち切ります。

変化を待つ場合は、起動前に `let mut updates = status.subscribe();` で購読し、`updates.next().await` を Bot と並行して実行します。短い間隔で複数回変化したときは、最新の一覧が届きます。停止時まで監視する場合は、すべてのシャードが `GatewayShardState::Closed` になったところで監視を終えます。
