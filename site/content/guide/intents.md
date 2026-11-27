# Intent の設定

**Intent** は、Bot が Discord から受け取りたいイベントの種類です。Akari は Intent を **自動では追加しません**。必要なものを、あなたが指定します（discord.py の `intents=` と同じ考え方です）。

## 何を指定するか

| 使う機能 | 必要な Intent |
| --- | --- |
| テキストコマンド（`run_commands`） | `GUILD_MESSAGES` と `MESSAGE_CONTENT` |
| `Events::on_message`、`Bot::run` のメッセージ処理 | `GUILD_MESSAGES` と `MESSAGE_CONTENT` |
| スラッシュコマンド、ボタン、選択メニュー、フォーム、`on_ready` | 不要 |

`Bot::add_intents` で足します。

```rust
use disnana_akari::model::gateway::Intents;
use disnana_akari::{Bot, Commands, Result};

async fn run(commands: Commands) -> Result<()> {
    Bot::from_env()?
        .add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)
        .run_commands(commands)
        .await
}
```

## 足りないとどうなるか

メッセージを扱う機能を登録しているのに、Intent が足りないときは、**Discord に接続する前に** エラーで終了します。足りない Intent と、直し方が表示されます。

```text
Error: テキストコマンドやメッセージのハンドラーには、Intent Intents(GUILD_MESSAGES | MESSAGE_CONTENT) が必要です。
`Bot::add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)` で追加してください。…
```

## 特権 Intent（Message Content）

`MESSAGE_CONTENT` は **特権 Intent** です。コードで指定するだけでなく、Developer Portal の **Bot** ページでも有効にします（[手順](../getting-started/discord-setup.md#_3-message-content-intent-を有効にする)）。

有効にしていないと、接続したあとに Gateway がコード `4014` で閉じます。理由は [`Bot::on_error`](errors.md) と `tracing` のログで確認できます。[ログを出す](logging.md)

## 設定の確認と置き換え

- `bot.intents()` は、起動時に Discord へ要求する Intent を返します。
- `with_intents` は、基本の集合を決めます。既定は空です。`add_intents` は、それに足します。呼ぶ順序によらず、結果は和集合です。

```rust
use disnana_akari::model::gateway::Intents;
use disnana_akari::{Bot, Result};

fn check() -> Result<()> {
    let bot = Bot::from_env()?
        .with_intents(Intents::GUILD_MESSAGES)
        .add_intents(Intents::MESSAGE_CONTENT);
    println!("要求する Intent: {:?}", bot.intents());
    Ok(())
}
```
