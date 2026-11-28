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

Webhook の ID とトークンを別々に渡します。`wait()` で作成されたメッセージが返ります。Webhook トークンはログや返信に含めないでください。

```rust
use disnana_akari::{Result, http::Client};
use disnana_akari::model::{
    channel::{Message, message::AllowedMentions},
    id::{Id, marker::WebhookMarker},
};

async fn post_update(http: &Client, webhook: Id<WebhookMarker>, token: &str) -> Result<Message> {
    Ok(http.execute_webhook(webhook, token)
        .content("レポートを更新しました。")
        .allowed_mentions(Some(&AllowedMentions::default()))
        .wait()
        .await?
        .model()
        .await?)
}
```

同じ送信が既に受理されたか分からない場合は、再送でメッセージが重複する可能性があります。[Discord の Webhook API](https://docs.discord.com/developers/resources/webhook#execute-webhook)

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

## 絵文字・投票・有料機能を扱う

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
