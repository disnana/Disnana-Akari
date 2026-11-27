# イベントを受け取る

接続が完了したときや、メッセージが届いたときに、関数を呼ぶ仕組みです。discord.py の `on_ready` や `on_message` にあたります。

## 基本

```rust
use disnana_akari::model::channel::Message;
use disnana_akari::model::gateway::Intents;
use disnana_akari::{Bot, Events, Ready, Result};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut events = Events::new();
    events.on_ready(on_ready);
    events.on_message(on_message);

    Bot::from_env()?
        .add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT) // on_message に必要
        .with_events(events)
        .start()
        .await
}

async fn on_ready(ready: &Ready) -> Result<()> {
    println!("{} としてログインしました", ready.user.name);
    Ok(())
}

async fn on_message(message: &Message) -> Result<()> {
    println!("{}: {}", message.author.name, message.content);
    Ok(())
}
```

| メソッド | 呼ばれるとき |
| --- | --- |
| `events.on_ready(関数)` | Discord との新しいセッションの準備ができたとき |
| `events.on_message(関数)` | 人がサーバーに送った新しいメッセージ（Bot と Webhook は除く） |
| `events.on_button(id, 関数)` | [ボタン](buttons.md)が押されたとき |
| `events.on_select(id, 関数)` ほか | [選択メニュー](select-menus.md)が操作されたとき |
| `events.on_modal(id, 関数)` | [入力フォーム](modals.md)が送信されたとき |

`on_ready` と `on_message` は、複数の関数を登録できます。登録した順に呼ばれ、1 つがエラーを返しても次へ進みます（エラーは [`Bot::on_error`](errors.md) に報告されます）。

> [!NOTE]
> `on_message` には、`GUILD_MESSAGES` と `MESSAGE_CONTENT` の Intent が必要です。Akari は自動では追加しないので、`add_intents` で指定してください（足りないときは、接続前にエラーになります）。Developer Portal の **Message Content Intent** も有効にします。[Intent の設定](intents.md)

## 編集・削除を受け取る

キャッシュに依存しない、discord.py の raw イベントに対応しています。

| 登録 | ハンドラーの引数 |
| --- | --- |
| `on_raw_message_edit` | `&MessageUpdate`。本文や Embed など、Discord が送った更新項目が入ります。 |
| `on_raw_message_delete` | `&MessageDelete`。削除された 1 件の ID は `id` で取得できます。 |
| `on_raw_bulk_message_delete` | `&MessageDeleteBulk`。一括削除された ID 一覧は `ids` で取得できます。 |

```rust
use disnana_akari::{Bot, Events, MessageUpdate, Result, model::gateway::Intents};

async fn watch_edits() -> Result<()> {
    let mut events = Events::new();
    events.on_raw_message_edit(async |update: &MessageUpdate| -> Result<()> {
        if let Some(content) = &update.content {
            println!("{} の本文を受信しました: {content}", update.id);
        }
        Ok(())
    });
    Bot::from_env()?
        .add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)
        .with_events(events)
        .start()
        .await
}
```

サーバーの通知には `GUILD_MESSAGES`、DM の編集・単独削除には `DIRECT_MESSAGES` を指定します。一括削除はサーバーのみです。本文が不要なら `MESSAGE_CONTENT` なしで登録できます。本文が `None` になる Embed だけの更新も届きます。編集前や削除時の内容も必要なら、次の節のメッセージキャッシュを使います。

### 保存したメッセージの編集前後を受け取る

編集前の内容も使いたいときは、`cache_messages` で保存件数の上限を指定し、`on_message_edit` に登録します。

```rust
use disnana_akari::{Bot, CachedMessageEdit, Events, Result, model::gateway::Intents};

fn bot(token: &str) -> Result<Bot> {
    let mut events = Events::new();
    events.on_message_edit(async |edit: &CachedMessageEdit| -> Result<()> {
        if edit.before.content != edit.after.content {
            println!("本文が変わったメッセージ {}", edit.after.id);
        }
        Ok(())
    });

    Ok(Bot::new(token)?
        .with_events(events)
        .cache_messages(1_000)
        .add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT))
}
```

`on_message_delete` は単独削除で保存していた内容を、`on_bulk_message_delete` は一括削除で保存していた分を渡します。キャッシュが扱うのは接続中に受信できたものです。上限に達すると ID が最も古いものから外します。切断前のセッションを再開できれば保持します。新しいセッションが始まった場合や、メッセージの更新・削除を読み取れず履歴に欠落が生じた場合は、そのシャードの保存分を消します。保存されているかに関係なく通知が欲しい場合は raw イベントを使います。

## リアクションの追加・削除を受け取る

サーバーの通知には `GUILD_MESSAGE_REACTIONS`、DM には `DIRECT_MESSAGE_REACTIONS` を指定します。受け取るペイロードにメッセージの ID が含まれるので、以前にそのメッセージを受信していなくても扱えます。

```rust
use disnana_akari::{Events, ReactionAdd, Result};

fn register(events: &mut Events) {
    events.on_raw_reaction_add(async |event: &ReactionAdd| -> Result<()> {
        println!("{} にリアクションが付きました。", event.message_id);
        Ok(())
    });
}
```

`on_raw_reaction_remove` は単独削除、`on_raw_reaction_clear` はすべての削除、`on_raw_reaction_clear_emoji` は指定された絵文字をまとめて削除した通知を受け取ります。

## 投票と入力開始を受け取る

| 登録 | 取得する情報 | サーバーで必要な Intent | DM で必要な Intent |
| --- | --- | --- | --- |
| `on_raw_poll_vote_add`、`on_raw_poll_vote_remove` | 投票者とメッセージの ID、選択肢の `answer_id` | `GUILD_MESSAGE_POLLS` | `DIRECT_MESSAGE_POLLS` |
| `on_raw_typing` | ユーザーとチャンネルの ID、入力開始時刻（Unix 秒） | `GUILD_MESSAGE_TYPING` | `DIRECT_MESSAGE_TYPING` |

## サーバー・チャンネル・ロールの変更を受け取る

`GUILDS` を指定して、受け取りたい型のハンドラーを `on_gateway` に登録します。対応型は `GuildCreate`、`GuildUpdate`、`GuildDelete`、`ChannelCreate`、`ChannelUpdate`、`ChannelDelete`、`RoleCreate`、`RoleUpdate`、`RoleDelete` です。

```rust
use disnana_akari::{Events, Result};
use disnana_akari::model::gateway::payload::incoming::{GuildDelete, RoleUpdate};

fn register(events: &mut Events) {
    events.on_gateway(async |event: &GuildDelete| -> Result<()> {
        if event.unavailable == Some(true) {
            println!("{} は一時的に利用できません。", event.id);
        } else {
            println!("{} から Bot が外れました。", event.id);
        }
        Ok(())
    });
    events.on_gateway(async |event: &RoleUpdate| -> Result<()> {
        println!("{} のロール {} が更新されました。", event.guild_id, event.role.id);
        Ok(())
    });
}
```

`GuildCreate` には Bot の参加に加え、接続時の初期情報や一時的な障害からの復帰も含まれます。更新前の状態が必要なら、アプリ側で必要な項目を保存します。[Discord の Gateway イベント仕様](https://docs.discord.com/developers/events/gateway-events#guild-create)

### ほかの型付き Gateway 通知

| ペイロード | Intent |
| --- | --- |
| `ThreadCreate`、`ThreadUpdate`、`ThreadDelete`、`ThreadListSync`、`ThreadMemberUpdate` | `GUILDS` |
| `ThreadMembersUpdate` | Bot 自身の通知では `GUILDS`。他のメンバーの通知には `GUILD_MEMBERS` |
| `MemberAdd`、`MemberRemove` | `GUILD_MEMBERS` |
| `MemberUpdate` | 他のメンバーの通知には `GUILD_MEMBERS`。Bot 自身の更新はこの Intent を省略しても届きます |
| `ChannelPinsUpdate` | サーバーでは `GUILDS`、DM では `DIRECT_MESSAGES` |
| `PresenceUpdate`、`VoiceStateUpdate` | それぞれ `GUILD_PRESENCES`、`GUILD_VOICE_STATES` |
| `VoiceServerUpdate` | 追加の Intent 指定なし。音声サーバーの認証用トークンを含みます |

`GUILD_MEMBERS` と `GUILD_PRESENCES` は、Developer Portal でも有効にする特権 Intent です。`ThreadMemberUpdate` は Bot 自身のスレッド参加情報の更新を表します。[Discord の Intent 一覧と例外](https://docs.discord.com/developers/events/gateway#list-of-intents)

### メンバーの変更前後を受け取る

`Bot::cache_members(n)` は保存するメンバー数を Bot 全体で制限します。`GUILDS` と `GUILD_MEMBERS` を追加し、`on_member_update` と `on_member_remove` に必要な関数を登録します。

```rust
use disnana_akari::{Bot, CachedMemberUpdate, Events, Result, model::gateway::Intents};

fn bot_with_members(token: &str) -> Result<Bot> {
    let mut events = Events::new();
    events.on_member_update(async |update: &CachedMemberUpdate| -> Result<()> {
        if update.before.roles != update.after.roles {
            println!("ロールが更新されたメンバー {}", update.after.user.id);
        }
        Ok(())
    });
    Ok(Bot::new(token)?
        .with_events(events)
        .cache_members(1_000)
        .add_intents(Intents::GUILDS | Intents::GUILD_MEMBERS))
}
```

初期状態や加入通知で受信したメンバーを保存します。更新時に Discord が省略した項目は現在の値を保ちます。保存前や上限による削除後の通知も取りたい場合は、`Events::on_gateway` で `MemberUpdate` や `MemberRemove` を登録します。キャッシュ保存のために全メンバーを API から取得する処理は行いません。権限チェックは従来どおり、実行時の権限情報を使います。

### 管理・予定・有料機能の通知

下表の型は `model::gateway::payload::incoming` から読み込み、`events.on_gateway` で登録します。

| ペイロード | Intent |
| --- | --- |
| `BanAdd`、`BanRemove`、`GuildAuditLogEntryCreate` | `GUILD_MODERATION` |
| `AutoModerationRuleCreate`、`AutoModerationRuleUpdate`、`AutoModerationRuleDelete` | `AUTO_MODERATION_CONFIGURATION` |
| `AutoModerationActionExecution` | `AUTO_MODERATION_EXECUTION` |
| `GuildEmojisUpdate`、`GuildStickersUpdate` | `GUILD_EMOJIS_AND_STICKERS`（Discord の仕様では `GUILD_EXPRESSIONS`） |
| `GuildIntegrationsUpdate`、`IntegrationCreate`、`IntegrationUpdate`、`IntegrationDelete` | `GUILD_INTEGRATIONS` |
| `WebhooksUpdate` | `GUILD_WEBHOOKS` |
| `InviteCreate`、`InviteDelete` | `GUILD_INVITES` |
| `StageInstanceCreate`、`StageInstanceUpdate`、`StageInstanceDelete` | `GUILDS` |
| `GuildScheduledEventCreate`、`GuildScheduledEventUpdate`、`GuildScheduledEventDelete`、`GuildScheduledEventUserAdd`、`GuildScheduledEventUserRemove` | `GUILD_SCHEDULED_EVENTS` |
| `CommandPermissionsUpdate`、`UserUpdate`、`EntitlementCreate` / `Update` / `Delete`、`SubscriptionCreate` / `Update` / `Delete` | 追加の Intent 指定なし |

`AutoModerationActionExecution` の本文は、Discord が `MESSAGE_CONTENT` を理由に省略した場合、空文字になります。イベントが届く条件には Intent 以外に、Bot の権限やアプリに対する操作の種類も含まれます。[Discord Gateway Events](https://docs.discord.com/developers/events/gateway-events)

## メッセージに返信したいとき

`on_message` の関数は `&Message` だけを受け取るので、返信はできません。返信したいときは、`Bot::run` で `Context` を受け取ります。

```rust
use disnana_akari::model::gateway::Intents;
use disnana_akari::{Bot, Context, Result};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    Bot::from_env()?
        .add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)
        .run(async |ctx: Context<'_>| -> Result<()> {
            if ctx.message().content == "こんにちは" {
                ctx.reply("こんにちは！").await?;
            }
            Ok(())
        })
        .await
}
```

テキストコマンドと併用するときは、コマンドは `run_commands` に任せ、ほかの反応は `on_message` に書きます。

## 実行の順序

`on_ready`、`on_message`、メッセージの raw イベントは、**受信した順に 1 件ずつ** 処理されます。処理が遅いと、あとのイベントが待たされます。重い処理は、コマンドやボタンのハンドラー（並行して実行されます）に置くか、`tokio::spawn` で切り分けてください。詳しくは [Akari の動き方](how-it-works.md) を参照してください。
