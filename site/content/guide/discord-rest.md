# Discord の REST 操作

コマンドやイベントの中から `ctx.http()` で取得したクライアントを使うと、Bot の認証とレート制限の管理を共有できます。以下の関数にはそのクライアントを渡します。Bot が行う操作の権限は Discord が検証します。利用者に操作を許可するかは、コマンド側で確認してください。

## メッセージからスレッドを作る

discord.py の `Message.create_thread` にあたります。元のメッセージ ID と、そのメッセージがあるチャンネル ID を渡します。

```rust
use disnana_akari::{Result, http::Client};
use disnana_akari::model::channel::{Channel, Message};

async fn discussion(http: &Client, message: &Message) -> Result<Channel> {
    Ok(http
        .create_thread_from_message(message.channel_id, message.id, "感想と質問")
        .await?
        .model()
        .await?)
}
```

作成したスレッドに送るときは、返された `Channel` の `id` を `create_message` に渡します。既存のスレッドを探す場合やフォーラムに投稿する場合は専用のルートを使います。[Discord のスレッド API](https://docs.discord.com/developers/topics/threads)

## フォーラム・メディアに投稿を作る

`ForumPost` はスレッドと最初のメッセージを一度に作り、Discord から返った両方を取得します。対象のフォーラム・メディアチャンネルでは Bot に `SEND_MESSAGES` が必要です。メディア投稿で必要な画像などは最初のメッセージに添付します。 [Discord のフォーラム作成 API](https://docs.discord.com/developers/resources/channel#start-thread-in-forum-or-media-channel) · [discord.py の ForumChannel.create_thread](https://discordpy.readthedocs.io/en/v2.7.1/api.html#discord.ForumChannel.create_thread)


```rust
use disnana_akari::{ForumPost, Result, http::Client};
use disnana_akari::model::id::{Id, marker::{ChannelMarker, TagMarker}};

async fn announce(
    http: &Client, forum: Id<ChannelMarker>, tag: Id<TagMarker>,
) -> Result<Id<ChannelMarker>> {
    let tags = [tag];
    let post = ForumPost::new("週末の予定")
        .content("参加できる時間を返信してください。")
        .applied_tags(&tags)
        .slowmode(5)
        .send(http, forum)
        .await?;
    Ok(post.channel.id)
}
```

タイトル、本文、タグや添付の上限を送信前に確認し、最初のメッセージでメンション通知を抑止します。作成の応答で通信に失敗した場合は、再送する前に投稿が作成済みか確認します。

## スレッドに参加したユーザーを調べる

特定ユーザーは `fetch_thread_member` で取得でき、Discord が `404` を返す場合は `None` です。`ThreadMemberPages` はユーザー ID の昇順で一覧を最大100件ずつ取得します。Discord API v10 のページ取得ではメンバー情報も要求し、一覧には特権 Intent の `GUILD_MEMBERS` が必要です。 [Discord のスレッド参加者 API](https://docs.discord.com/developers/resources/channel#list-thread-members)

```rust
use disnana_akari::{Error, Result, ThreadMemberPages, fetch_thread_member, http::Client};
use disnana_akari::model::id::{Id, marker::{ChannelMarker, UserMarker}};

async fn show_thread_members(
    http: &Client, thread: Id<ChannelMarker>, actor: Id<UserMarker>,
) -> Result<()> {
    if fetch_thread_member(http, thread, actor).await.map_err(Error::handler)?.is_some() {
        println!("操作者は参加済みです。");
    }
    let mut pages = ThreadMemberPages::new(http, thread).limit(200);
    while let Some(page) = pages.next_page().await.map_err(Error::handler)? {
        for member in page {
            if let Some(user) = member.user_id {
                println!("参加ユーザー {user}");
            }
        }
    }
    Ok(())
}
```

## メンバーを取得し、ロールを付ける

1人の情報は `guild_member` で取得します。一覧を続けて読む場合は `guild_members(guild).limit(1000)` の末尾にある `member.user.id` を次の `after` に使います。一覧の取得には特権 Intent の `GUILD_MEMBERS` を Developer Portal で有効にしてください。

```rust
use disnana_akari::{Result, SlashContext};
use disnana_akari::model::{
    guild::Permissions,
    id::{Id, marker::{GuildMarker, RoleMarker, UserMarker}},
};

async fn assign_role(
    ctx: &mut SlashContext<'_>,
    guild: Id<GuildMarker>,
    member: Id<UserMarker>,
    role: Id<RoleMarker>,
) -> Result<()> {
    ctx.require_permissions(Permissions::MANAGE_ROLES)?;
    if ctx.guild_id() != Some(guild) {
        return Err(disnana_akari::Error::handler(std::io::Error::other("対象のサーバーが異なります")));
    }
    // アプリで許可したロール ID と、実行者が管理できる範囲を呼び出し元で確認する。
    ctx.http().add_guild_member_role(guild, member, role).await?;
    ctx.reply_ephemeral("ロールを付けました。").await
}
```

Bot が管理できるロールは Discord のロール階層で決まります。運用コマンドでは Bot の権限に加え、利用者が操作してよいメンバーやロールも絞り込んでください。モデレーションのリクエストは `create_ban`、`delete_ban`、`update_guild_member` などの型付きルートを使えます。[Discord のサーバー API](https://docs.discord.com/developers/resources/guild)

## 期限と利用回数を指定して招待を作る

```rust
use disnana_akari::{Result, http::Client};
use disnana_akari::model::id::{Id, marker::ChannelMarker};

async fn single_use_invite(http: &Client, channel: Id<ChannelMarker>) -> Result<String> {
    let invite = http.create_invite(channel)
        .max_age(900)
        .max_uses(1)
        .unique(true)
        .await?
        .model()
        .await?;
    Ok(invite.code)
}
```

例では有効期限を15分に設定します。招待コードを渡す範囲はコマンド側で決めてください。[Discord の招待作成仕様](https://docs.discord.com/developers/resources/channel#create-channel-invite)

## Webhook を使ってメッセージを送る

Webhook の ID と専用トークンを `WebhookPost::send` に渡すと、Discord が投稿を確認し、作成されたメッセージを返します。メンション通知を抑止します。クライアントの既定ヘッダーに `Authorization` を設定しないでください。

Akari はこの操作の HTTP リクエストのスパンを抑止します。Twilight のレート制限を担当する別タスクはトークンを含むパスを記録することがあります。ログの対象を Akari に絞るか、購読側で `twilight_http_ratelimiting=off` を指定してください。 [ログの設定](logging.md)

```rust
use disnana_akari::{Error, Result, WebhookPost, http::Client};
use disnana_akari::model::{
    channel::Message,
    id::{Id, marker::{ChannelMarker, TagMarker, WebhookMarker}},
};

async fn post_update(http: &Client, webhook: Id<WebhookMarker>, token: &str) -> Result<Message> {
    WebhookPost::new().content("レポートを更新しました。")
        .send(http, webhook, token).await.map_err(Error::handler)
}

async fn post_in_thread(
    http: &Client, webhook: Id<WebhookMarker>, token: &str, thread: Id<ChannelMarker>,
) -> Result<Message> {
    WebhookPost::new().content("続報です。").in_thread(thread)
        .send(http, webhook, token).await.map_err(Error::handler)
}

async fn publish_forum_update(
    http: &Client, webhook: Id<WebhookMarker>, token: &str, tags: &[Id<TagMarker>],
) -> Result<Message> {
    WebhookPost::new().content("今週の更新です。").new_thread("週次報告", tags)
        .send(http, webhook, token).await.map_err(Error::handler)
}
```

本文・Embed・添付・投票は組み合わせて使えます。`in_thread` は指定した既存スレッドへ投稿し、必要に応じて Discord がアーカイブを解除します。`new_thread` は Forum または Media にスレッドを作成します。同じ送信が受理されたか分からない場合は、再送でメッセージが重複する可能性があります。 [Discord の Webhook API](https://docs.discord.com/developers/resources/webhook#execute-webhook) · [discord.py の Webhook.send](https://discordpy.readthedocs.io/en/v2.7.1/api.html#discord.Webhook.send)

### 投稿を取得・編集・削除する

投稿時の Webhook ID と、返されたメッセージの ID から `WebhookMessageRef` を作ります。投稿がスレッドにある場合は `in_thread` にメッセージの `channel_id` を渡します。編集項目を指定しなければ元の値を保持します。`content(None)` で本文を、`embeds(&[])` で Embed を消せます。添付を編集するときは、残す添付 ID と追加するファイルの両方を指定します。

```rust
use disnana_akari::{Result, Error, WebhookMessageEdit, WebhookMessageRef, http::Client};
use disnana_akari::model::{
    channel::Message,
    id::{Id, marker::{ChannelMarker, MessageMarker, WebhookMarker}},
};

async fn update_report(
    http: &Client, token: &str, webhook: Id<WebhookMarker>,
    message: Id<MessageMarker>, thread: Id<ChannelMarker>,
) -> Result<Message> {
    let target = WebhookMessageRef::new(webhook, message).in_thread(thread);
    let edit = WebhookMessageEdit::new().content(Some("集計が確定しました。"));
    target.edit(http, token, &edit).await.map_err(Error::handler)
}
```

同じ参照の `fetch(http, token)` で現在の投稿を取得し、`delete(http, token)` で削除できます。 [Discord の Webhook メッセージ操作](https://docs.discord.com/developers/resources/webhook#get-webhook-message) · [discord.py の WebhookMessage](https://discordpy.readthedocs.io/en/v2.7.1/api.html#discord.WebhookMessage)

## サーバー外の予定を登録する

外部イベントには、開催場所と開始・終了時刻を設定します。時刻は RFC 3339 の文字列から `Timestamp` に変換できます。

```rust
use disnana_akari::{Result, http::Client};
use disnana_akari::model::{
    guild::scheduled_event::{GuildScheduledEvent, PrivacyLevel},
    id::{Id, marker::GuildMarker},
    util::Timestamp,
};

async fn schedule(
    http: &Client,
    guild: Id<GuildMarker>,
    start: &Timestamp,
    end: &Timestamp,
) -> Result<GuildScheduledEvent> {
    Ok(http.create_guild_scheduled_event(guild, PrivacyLevel::GuildOnly)
        .external("勉強会", "オンライン配信", start, end)
        .await?
        .model()
        .await?)
}
```

作成・更新・削除の通知も受け取る場合は、[イベントのガイド](events.md) から Intent と登録方法を確認できます。[Discord の予定イベント API](https://docs.discord.com/developers/resources/guild-scheduled-event)

## 投票を作成する

`PollDraft` は質問を300文字、選択肢を2～10件、各選択肢を55文字、受付時間を1～768時間までに制限します。設定を省略すると24時間、単一選択です。投稿先のギルドチャンネルで Bot に `SEND_POLLS` と投稿権限が必要です。コマンドから投稿する場合、実行者に求める条件はコマンド側に書きます。 [Discord の投票 API](https://docs.discord.com/developers/resources/poll#poll-create-request-object) · [discord.py の Poll](https://discordpy.readthedocs.io/en/v2.7.1/api.html#discord.Poll)

```rust
use disnana_akari::{PollAnswerInput, PollDraft, PollVoterPages, Error, Result, http::Client};
use disnana_akari::model::{channel::Message, id::{Id, marker::ChannelMarker}};
use std::num::NonZeroU8;

async fn publish_poll(http: &Client, channel: Id<ChannelMarker>) -> Result<Message> {
    let answers = [PollAnswerInput::new("土曜日"), PollAnswerInput::new("日曜日")];
    let poll = PollDraft::new("いつ集まる？", &answers)
        .and_then(|poll| poll.duration_hours(48))
        .map_err(Error::handler)?
        .allow_multiselect(true);
    poll.send(http, channel).await.map_err(Error::handler)
}

async fn first_answer_voters(http: &Client, message: &Message) -> Result<usize> {
    let Some(answer) = message.poll.as_ref()
        .and_then(|poll| poll.answers.first())
        .and_then(|answer| NonZeroU8::new(answer.answer_id))
    else {
        return Ok(0);
    };
    let mut voters = PollVoterPages::new(http, message.channel_id, message.id, answer);
    let mut count = 0;
    while let Some(page) = voters.next_page().await.map_err(Error::handler)? {
        count += page.len();
    }
    Ok(count)
}
```

選択肢に絵文字を付けるには `PollAnswerInput::emoji` に `parse_reaction_emoji` で変換した値を渡します。投票者の取得には投稿結果の回答 ID を使います。コマンドやフォームの応答に投票を付ける場合は、[投票で応答する](responding.md#投票で応答する)を参照してください。

## 絵文字・投票の終了・有料機能を扱う

```rust
use disnana_akari::{EntitlementPages, Error, Result, SubscriptionPages, http::Client};
use disnana_akari::model::{
    channel::Message,
    id::{Id, marker::{ApplicationMarker, GuildMarker, SkuMarker, UserMarker}},
};

async fn guild_catalog(http: &Client, guild: Id<GuildMarker>) -> Result<(usize, usize)> {
    let emoji = http.emojis(guild).await?.models().await?;
    let stickers = http.guild_stickers(guild).await?.models().await?;
    Ok((emoji.len(), stickers.len()))
}

async fn close_authored_poll(http: &Client, message: &Message) -> Result<Message> {
    Ok(http.end_poll(message.channel_id, message.id).await?.model().await?)
}

async fn entitlement_count(
    http: &Client,
    app: Id<ApplicationMarker>,
    user: Id<UserMarker>,
) -> Result<usize> {
    let mut pages = EntitlementPages::new(http, app)
        .user_id(user)
        .exclude_ended(true)
        .exclude_deleted(true);
    let mut count = 0;
    while let Some(page) = pages.next_page().await.map_err(Error::handler)? {
        count += page.len();
    }
    Ok(count)
}

async fn print_subscriptions(
    http: &Client,
    sku: Id<SkuMarker>,
    user: Id<UserMarker>,
) -> Result<()> {
    let mut pages = SubscriptionPages::new(http, sku, user);
    while let Some(page) = pages.next_page().await.map_err(Error::handler)? {
        for subscription in page {
            println!("購読 ID {}", subscription.id);
        }
    }
    Ok(())
}
```

投票の終了はその投票を投稿した Bot が行います。有料機能の一覧は最大100件ずつ取得します。`EntitlementPages` では対象ユーザー、サーバー、SKU を絞り込み、期限切れと削除済みを個別に除外できます。`SubscriptionPages` を Bot トークンで使うには SKU とユーザーの ID が必要です。取得失敗時は同じ位置から再試行できます。 [絵文字 API](https://docs.discord.com/developers/resources/emoji) · [投票 API](https://docs.discord.com/developers/resources/poll) · [Entitlement API](https://docs.discord.com/developers/resources/entitlement#list-entitlements) · [Subscription API](https://docs.discord.com/developers/resources/subscription#list-sku-subscriptions)

スラッシュコマンドでは `ctx.entitlements()` で今回の Interaction に Discord が付けた購入権利を参照できます。ハイブリッドや共通チェックからも参照でき、テキスト呼び出しでは `None` です。利用可否を判定するときは対象 SKU、利用者またはサーバー、有効期間、消費や削除の状態をアプリ側で確認します。特典の利用可否には購入権利を使い、購読状態は契約の表示や運用に使います。 [Discord の購入権利](https://docs.discord.com/developers/resources/entitlement)

補完、ボタン、すべてのセレクトメニュー、モーダルでも `ctx.entitlements()` を使えます。UI セッションのハンドラーでは、画面を開いた時点の情報ではなく、ボタンの押下やフォーム送信と一緒に届いた購入権利を受け取ります。操作者はその時点の `author_id()`、サーバーは `guild_id()` で照合します。 [Discord の Interaction 構造](https://docs.discord.com/developers/interactions/receiving-and-responding#interaction-object)

### テスト用の購入権利と消費型 SKU を扱う

`monetization::create_test_entitlement` は、型付き ID でテスト用の購入権利を作ります。ユーザーとサーバーのどちらへ付与するかは `CreateTestEntitlementOwner` で指定します。片付けには Twilight の `delete_test_entitlement` を使います。作成の応答を読み取れなかった場合は、再作成の前に一覧で結果を確かめます。 [Discord のテスト用購入権利 API](https://docs.discord.com/developers/resources/entitlement#create-test-entitlement)

```rust
use disnana_akari::{
    Error, Result, http::Client,
    monetization::{CreateTestEntitlementOwner, create_test_entitlement},
};
use disnana_akari::model::id::{Id, marker::{
    ApplicationMarker, EntitlementMarker, SkuMarker, UserMarker,
}};

async fn grant_test_access(
    http: &Client, app: Id<ApplicationMarker>, sku: Id<SkuMarker>, user: Id<UserMarker>,
) -> Result<Id<EntitlementMarker>> {
    let entitlement = create_test_entitlement(http, app, sku, CreateTestEntitlementOwner::User(user))
        .await
        .map_err(Error::handler)?;
    Ok(entitlement.id)
}

async fn revoke_test_access(
    http: &Client, app: Id<ApplicationMarker>, entitlement: Id<EntitlementMarker>,
) -> Result<()> {
    http.delete_test_entitlement(app, entitlement).await?;
    Ok(())
}

async fn consume_confirmed_purchase(
    http: &Client, app: Id<ApplicationMarker>, entitlement: Id<EntitlementMarker>,
) -> Result<()> {
    http.consume_entitlement(app, entitlement).await?;
    Ok(())
}
```

消費は、対象 SKU が買い切りの消費型で、今回の利用に対応する権利だとアプリが確認した後に呼びます。Discord が成功を返すと、以降の一覧で `consumed` が `true` になります。 [Discord の購入権利消費 API](https://docs.discord.com/developers/resources/entitlement#consume-an-entitlement)

## メッセージを転送・削除する

`forward_message` は元チャンネルとメッセージの ID を指定して転送し、Discord が作ったメッセージを返します。通知は抑止され、Discord が返したスナップショットは `message_snapshots` から読めます。コマンドで使う場合は、実行者が転送元を閲覧でき、転送先へ投稿できることを確かめます。 [Discord のメッセージ参照](https://docs.discord.com/developers/resources/message#message-reference-object) · [discord.py の転送](https://discordpy.readthedocs.io/en/v2.7.1/api.html#discord.Message.forward)

`delete_messages` は渡した ID だけを対象にします。空なら通信せず、1件なら通常削除、2〜100件なら一括削除を使います。一括削除はサーバー内で Bot に `MANAGE_MESSAGES` が必要です。重複や作成から14日を超えるメッセージは送信前に拒否します。一括削除に失敗した場合は、その失敗を返します。 [Discord の一括削除仕様](https://docs.discord.com/developers/resources/message#bulk-delete-messages)

```rust
use disnana_akari::{Error, Result, delete_messages, forward_message, http::Client};
use disnana_akari::model::id::{Id, marker::{ChannelMarker, MessageMarker}};

async fn copy_to_log(
    http: &Client, source: Id<ChannelMarker>, message: Id<MessageMarker>, log: Id<ChannelMarker>,
) -> Result<Id<MessageMarker>> {
    let copied = forward_message(http, source, message, log).await.map_err(Error::handler)?;
    Ok(copied.id)
}

async fn delete_confirmed(
    http: &Client, channel: Id<ChannelMarker>, confirmed: &[Id<MessageMarker>],
) -> Result<()> {
    delete_messages(http, channel, confirmed, Some("確認済みの投稿を整理"))
        .await.map_err(Error::handler)
}
```

## リアクションを付ける・外す

`parse_reaction_emoji` は Unicode 絵文字、`<:name:id>`、`<a:name:id>`、`name:id` を Twilight のリアクション指定に変換します。同じ指定を `ReactionPages` の一覧取得にも使えます。Discord が絵文字を使えるか判定します。Bot 自身のリアクションを外す場合と、他のユーザーのリアクションを消す場合は別の API を使います。 [Discord のリアクション仕様](https://docs.discord.com/developers/resources/message#create-reaction)

```rust
use disnana_akari::{Error, Result, parse_reaction_emoji, http::Client};
use disnana_akari::model::channel::Message;

async fn react(http: &Client, message: &Message, emoji: &str) -> Result<()> {
    let emoji = parse_reaction_emoji(emoji).map_err(Error::handler)?;
    http.create_reaction(message.channel_id, message.id, &emoji).await?;
    Ok(())
}
```

## エラーを処理する

読み取りの結果が `404` なら、対象が見つからないという結果にできます。認証、権限、レート制限、レスポンスのデコードに失敗した場合は、エラーを呼び出し元に渡します。

```rust
use disnana_akari::{Result, http::{Client, error::ErrorType}};
use disnana_akari::model::{
    channel::Channel,
    id::{Id, marker::ChannelMarker},
};

async fn channel_if_present(http: &Client, id: Id<ChannelMarker>) -> Result<Option<Channel>> {
    match http.channel(id).await {
        Ok(response) => Ok(Some(response.model().await?)),
        Err(error) => match error.kind() {
            ErrorType::Response { status, .. } if status.get() == 404 => Ok(None),
            _ => Err(error.into()),
        },
    }
}
```

同じクライアントを使うと、Twilight がルートごとの枠をリクエスト間で共有します。現在の Twilight は Discord から `429` を受けたとき自動で再試行します。コマンド全体の待ち時間に上限が必要なら、操作を `tokio::time::timeout` で囲みます。書き込みがタイムアウトした場合は、Discord が操作を受理したか確認してから再試行してください。[Discord のレート制限仕様](https://docs.discord.com/developers/topics/rate-limits)
