# 定期処理

Bot の接続中に集計やメンテナンスを行うときは、Tokio のタイマーで処理を繰り返せます。起動する場所、間隔、エラーが起きたときの動作をアプリで決めます。discord.py の `discord.ext.tasks` で使う定期処理にあたります。

## Ready を待ってから動かす

次の例は、Gateway が最初に Ready になったあとに集計を始めます。再接続で Ready が再び来ても、定期処理は 1 本だけです。Bot が終了すると、同じ関数の中で定期処理も終了します。

```rust
use std::time::Duration;
use disnana_akari::{Bot, Events, Ready, Result};
use tokio::{sync::watch, time::{self, MissedTickBehavior}};

async fn run(bot: Bot) -> Result<()> {
    let (ready, mut ready_rx) = watch::channel(false);
    let mut events = Events::new();
    events.on_ready(async move |_: &Ready| -> Result<()> {
        ready.send_replace(true);
        Ok(())
    });

    let periodic = async {
        if ready_rx.wait_for(|ready| *ready).await.is_err() {
            return;
        }
        let mut interval = time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            collect_statistics().await;
        }
    };

    tokio::select! {
        result = bot.with_events(events).start() => result,
        () = periodic => Ok(()),
    }
}

async fn collect_statistics() {
    // ここでアプリの集計処理を行います。
}
```

最初の実行は Ready の直後です。集計が長引いた場合、過ぎた間隔の分を連続して実行しないよう `MissedTickBehavior::Skip` を指定しています。集計関数の実行中に Bot が終了すると、集計の Future はその時点で破棄されます。完了まで待つ必要がある書き込みは、終了を通知してタスクを `await` する構成にします。

ネットワーク障害の再試行を入れる場合は、処理が安全に繰り返せることを確かめ、対象のエラーと待ち時間を決めます。CPU を長く使う処理や同期 I/O は `tokio::task::spawn_blocking` に移します。

[discord.py の定期処理](https://discordpy.readthedocs.io/en/stable/ext/tasks/) · [Tokio の Interval](https://docs.rs/tokio/latest/tokio/time/struct.Interval.html)

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

## シャード別の状態と RTT を監視する

`let (bot, status) = bot.with_gateway_status();` で監視を設定します。`status.shards()` は、起動待ち、初回接続中、準備済み、再接続中、終了のいずれかと、計測した RTT をシャード番号順に返します。シャード数が決まるまでは空の一覧です。切断したシャードの RTT は、新しい接続でハートビートの応答を受けるまで `None` です。 起動に失敗して状態が更新されない場合に備え、監視は Bot の起動処理と並行し、その処理が終了した時点でも打ち切ります。

変化を待つ場合は、起動前に `let mut updates = status.subscribe();` で購読し、`updates.next().await` を Bot と並行して実行します。短い間隔で複数回変化したときは、最新の一覧が届きます。停止時まで監視する場合は、すべてのシャードが `GatewayShardState::Closed` になったところで監視を終えます。
