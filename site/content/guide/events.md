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

## サーバーへの参加と復旧を区別する

`GUILD_CREATE` は、新しく追加されたときに加え、起動直後の読み込みと一時停止からの復旧にも届きます。参加時だけ初期設定を案内するには、`on_guild_lifecycle` の `Joined` を使います。

```rust
use disnana_akari::{Bot, Events, GuildLifecycleEvent, GuildLifecycleKind, Result};
use disnana_akari::model::gateway::Intents;

fn configure() -> Result<Bot> {
    let mut events = Events::new();
    events.on_guild_lifecycle(on_guild_lifecycle);
    Ok(Bot::from_env()?.add_intents(Intents::GUILDS).with_events(events))
}

async fn on_guild_lifecycle(event: &GuildLifecycleEvent) -> Result<()> {
    if event.kind() == GuildLifecycleKind::Joined {
        println!("新しく参加したサーバー {}", event.guild_id());
    }
    Ok(())
}
```

| 種類 | 届くとき |
| --- | --- |
| `InitialAvailable` | `Ready` で通知されたサーバーを、この接続で初めて利用できたとき |
| `Joined` | Bot が新しいサーバーに参加し、利用できるようになったとき |
| `Available` | 利用中のサーバーが一時停止から復旧したとき |
| `Unavailable` | 利用中のサーバーが一時的に利用できなくなったとき |
| `Removed` | Bot がサーバーから退出したとき |

状態はシャードごとに管理します。再開（Resume）では状態を引き継ぎ、新しい `Ready` で読み込みをやり直します。関連する Gateway ペイロードの解釈に失敗すると、そのシャードの分類は次の `Ready` まで停止します。サーバーの詳しいデータも必要なら、`on_gateway` で `GuildCreate` や `GuildDelete` をあわせて受け取れます。 [Discord の Gateway イベント](https://docs.discord.com/developers/events/gateway-events#guild-create) · [discord.py のサーバーイベント](https://discordpy.readthedocs.io/en/v2.7.1/api.html#discord.on_guild_join)

## 次のメッセージやリアクションを待つ

`Bot::with_waiters` で待機を有効にし、返された `EventWaiters` をコマンドやタスクで共有します。チャンネルとユーザーを指定し、次の投稿やリアクションが届くまで期限付きで待てます。登録は呼び出した時点で行われるので、ボットが質問を送る前に待機を作っておくと、すぐに返った回答も受け取れます。

```rust
use std::time::Duration;
use disnana_akari::{
    Bot, EventWaiters, Error, Result, WaitLimits,
    model::{gateway::Intents, id::{Id, marker::{ChannelMarker, UserMarker}}},
};

fn configure(bot: Bot) -> Result<(Bot, EventWaiters)> {
    bot.add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)
        .with_waiters(WaitLimits::default())
        .map_err(Error::handler)
}

async fn answer(
    waiters: &EventWaiters,
    channel: Id<ChannelMarker>,
    user: Id<UserMarker>,
) -> Result<String> {
    let pending = waiters.wait_for_message_where(
        channel,
        user,
        Duration::from_secs(30),
        |message| matches!(message.content.as_str(), "はい" | "いいえ"),
    ).map_err(Error::handler)?;
    let message = pending.await.map_err(Error::handler)?;
    Ok(message.content.clone())
}
```

`wait_for_message` と `wait_for_reaction_add` は、指定のチャンネルとユーザーに一致する次のイベントを返します。`_where` 付きのメソッドでは、本文・メッセージ ID・絵文字などに条件を追加できます。待機を作れるのは Bot の起動後です。メッセージには `GUILD_MESSAGES` または `DIRECT_MESSAGES`、リアクションには `GUILD_MESSAGE_REACTIONS` または `DIRECT_MESSAGE_REACTIONS` を指定します。上の例は本文を読むため `MESSAGE_CONTENT` も使います。

イベントは通常のハンドラーにも届きます。同じ対象を待つ複数のタスクがあれば、それぞれがイベントを受け取れます。待機数は Bot 全体とユーザー単位で制限され、待機を破棄するか期限が来ると枠が空きます。Bot が停止すると待機はキャンセルされます。条件関数は待機タスクで実行されるので、同期処理は短く保ってください。条件付き待機で候補の待ち行列があふれた場合は `WaitError::Overflow` を返します。 [discord.py の wait_for](https://discordpy.readthedocs.io/en/v2.7.1/api.html#discord.Client.wait_for)

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

### 保存済みのメンバーを参照する

通知を受ける代わりに現在の保存内容を読みたいときは、`with_member_cache` が返すハンドルを使います。コマンドに共有する場合は、ハンドルを `clone()` して渡せます。

```rust
use disnana_akari::{Bot, MemberCacheHandle, Result, model::gateway::Intents};
use disnana_akari::model::id::{Id, marker::{GuildMarker, UserMarker}};

fn enable_cache(token: &str) -> Result<(Bot, MemberCacheHandle)> {
    Ok(Bot::new(token)?
        .add_intents(Intents::GUILDS | Intents::GUILD_MEMBERS)
        .with_member_cache(1_000))
}

fn display_name(
    cache: &MemberCacheHandle,
    guild: Id<GuildMarker>,
    user: Id<UserMarker>,
) -> Option<String> {
    let member = cache.get(guild, user)?;
    Some(member.nick.clone().unwrap_or_else(|| member.user.name.clone()))
}
```

`guild_members(guild)` は、そのサーバーで現在参照できるスナップショットを返します。シャードが切断された場合は、そのシャードの値を隠し、再送が完了した時点で参照を再開します。取得済みのスナップショットは保持できます。権限確認ではコマンドコンテキストの `require_permissions` を使います。

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

## 起動時に指定サーバーのメンバーを補う

`with_member_cache_bootstrap` を使うと、明示したサーバーが利用可能になったときに、メンバーをバックグラウンドで取得します。件数はサーバーごとの指定上限とキャッシュ容量の小さい方までで、REST の各ページは最大1,000件です。Gateway の再開時には途中のページから続けます。`on_ready` は取得完了を待たないので、初回から必要なメンバーがいる場合は、キャッシュに見つからなかったときの処理も用意します。`GUILDS` と特権 Intent の `GUILD_MEMBERS` を設定します。

```rust
use std::num::NonZeroU16;
use disnana_akari::{Bot, MemberCacheHandle};
use disnana_akari::model::{
    gateway::Intents,
    id::{Id, marker::GuildMarker},
};

fn enable_seed(bot: Bot, guild: Id<GuildMarker>, per_guild_limit: NonZeroU16)
    -> (Bot, MemberCacheHandle)
{
    let (bot, members) = bot
        .add_intents(Intents::GUILDS | Intents::GUILD_MEMBERS)
        .with_member_cache(512);
    (bot.with_member_cache_bootstrap([guild], per_guild_limit), members)
}
```

キャッシュの空きだけを使います。取得中に Gateway から更新・退出が届いたメンバーは、その REST 結果で上書き・再追加しません。全件同期が必要な処理では、取得前後の変化も考慮して別途設計してください。[Discord の List Guild Members](https://docs.discord.com/developers/resources/guild#list-guild-members)

## Gateway でメンバーを探す

`Bot::with_member_query` が返す `MemberQuery` をコマンドなどと共有します。接続後に、ユーザー名の先頭か、ユーザー ID の一覧で検索できます。

```rust
use std::time::Duration;
use disnana_akari::{Bot, Error, MemberQuery, Result, model::id::{Id, marker::GuildMarker}};

fn configure(bot: Bot) -> (Bot, MemberQuery) {
    bot.with_member_query()
}

async fn names(query: &MemberQuery, guild: Id<GuildMarker>, prefix: &str) -> Result<Vec<String>> {
    let result = query.query_prefix(guild, prefix, 10, Duration::from_secs(5))
        .await.map_err(Error::handler)?;
    Ok(result.members().iter().map(|member| member.user.name.clone()).collect())
}
```

`query_ids` で一覧を照会すると、Discord が見つけられなかった ID を `not_found()` で確認できます。どちらも一度に最大100件です。応答を要求ごとに紐付け、複数チャンクをまとめて返します。シャードの送信が詰まった場合も、Gateway に渡す照会数に上限を設けており、送信待ちも期限に含みます。処理枠の超過、タイムアウト、対象シャードの切断は型付きエラーで返します。

この API は空の名前での全件取得やプレゼンスの取得を扱いません。名前の先頭と ID の照会に特権 Intent を自動追加する必要はありません。 Bot が対象サーバーに参加し、そのシャードが準備できている必要があります。 [Discord の Request Guild Members](https://docs.discord.com/developers/events/gateway-events#request-guild-members) · [discord.py の query_members](https://discordpy.readthedocs.io/en/v2.7.1/api.html#discord.Guild.query_members)

## メンバーの音声状態の変更前後を受け取る

`on_voice_state_transition` は接続、移動、退出、ミュートなどの変化を通知します。`before()` が `None` の場合は以前の状態が不明です。スナップショットの `channel_id()` が `None` の場合は、その状態でチャンネルに接続していません。

```rust
use disnana_akari::{Bot, Events};
use disnana_akari::model::gateway::Intents;

fn configure(bot: Bot, events: &mut Events) -> Bot {
    events.on_voice_state_transition(async |change| -> disnana_akari::Result<()> {
        let old = change.before().and_then(|state| state.channel_id());
        let new = change.after().channel_id();
        println!("{old:?} -> {new:?}");
        Ok(())
    });
    bot.add_intents(Intents::GUILDS | Intents::GUILD_VOICE_STATES)
}
```

キャッシュは登録した場合に作成し、シャードごとに既定で最大1,024件を保持します。`Events::set_voice_state_cache_capacity` で上限を指定できます。元の Gateway 通知も登録している場合は、同じイベントについて元の通知を先に処理します。

## ボイスチャンネルの効果音とサウンドボード

`Events::on_voice_channel_effect_send` は、VC の絵文字アニメーションやサウンドボードで音が鳴った通知を受け取ります。`GUILD_VOICE_STATES` Intent を指定します。

```rust
use disnana_akari::{Events, Result, VoiceChannelEffectSend};

fn listen(events: &mut Events) {
    events.on_voice_channel_effect_send(async |event: &VoiceChannelEffectSend| -> Result<()> {
        println!("チャンネル {} で効果音イベントを受信", event.channel_id);
        Ok(())
    });
}
```

サーバーのサウンドボード素材の作成・更新・削除は、`on_guild_soundboard_sound_create`、`on_guild_soundboard_sound_update`、`on_guild_soundboard_sound_delete` で受け取ります。複数の更新は `on_guild_soundboard_sounds_update` です。これらの通知には Twilight の `GUILD_EMOJIS_AND_STICKERS` Intent が必要です。`on_soundboard_sounds` は Gateway の一覧取得に対する応答を受け取ります。[Discord の Gateway イベント仕様](https://docs.discord.com/developers/events/gateway-events#voice-channel-effect-send)

既定音は `soundboard::default_sounds(ctx.http())`、サーバー固有の音は `soundboard::guild_sounds(ctx.http(), guild_id)` で REST から取得できます。`soundboard::play(ctx.http(), channel_id, sound_id, source_guild)` は、Bot がすでに接続しているボイスチャンネルで指定の音を再生します。既定音や同じサーバーの音では `source_guild` に `None` を渡し、別サーバーの音ではそのサーバー ID を渡します。Bot には `SPEAK` と `USE_SOUNDBOARD`、別サーバーの音には追加で `USE_EXTERNAL_SOUNDS` が必要です。[Discord のサウンドボード REST 仕様](https://docs.discord.com/developers/resources/soundboard)

## ボイスチャンネルのステータスと開始時刻

`Events::on_voice_channel_status_update` と `Events::on_voice_channel_start_time_update` は、`GUILDS` Intent を指定すると受信できます。イベント型は `disnana_akari::voice_channel` にあります。ステータスが解除されると `status` は `None` になり、開始時刻は Unix 時刻の秒単位です。[Discord のボイスチャンネルイベント仕様](https://docs.discord.com/developers/events/gateway-events#voice-channel-status-update)

`Events::on_channel_info` は `Request Channel Info` に対するサーバーごとの応答を受信します。Akari が提供するのは受信の登録です。登録だけでは Discord に情報を要求しません。[Discord のチャンネル情報イベント仕様](https://docs.discord.com/developers/events/gateway-events#channel-info)
