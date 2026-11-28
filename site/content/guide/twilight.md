# Twilight を直接使う

Akari は [Twilight](https://twilight.rs/) の上に作られています。Akari にない機能は、Twilight の型と HTTP クライアントで補えます。

## 型を使う

`disnana_akari::model` は Twilight のモデル型（`twilight_model`）です。Akari と同じバージョンの型が使えます。

```rust
use disnana_akari::model::channel::Message;
use disnana_akari::model::id::{Id, marker::UserMarker};

fn is_author(message: &Message, user: Id<UserMarker>) -> bool {
    message.author.id == user
}
```

## HTTP クライアントを使う

`ctx.http()` は、Bot が使っている Twilight の HTTP クライアントを返します。Akari の返信と同じ接続とレート制限の管理をそのまま使えます。`CommandContext`、`SlashContext`、`HybridContext`、`ButtonContext`、`SelectContext`、`ModalContext` で使えます。

```rust
use disnana_akari::model::channel::Channel;
use disnana_akari::{CommandContext, Result};

async fn channel_name(ctx: CommandContext<'_>) -> Result<()> {
    let channel_id = ctx.context().message().channel_id;
    let channel: Channel = ctx.http().channel(channel_id).await?.model().await?;
    let name = channel.name.unwrap_or_default();
    ctx.reply(&format!("このチャンネルは `{name}` です。")).await
}
```

> [!WARNING]
> 初回応答や更新が済んだかどうかを Akari が記録しているのは、`reply`、`defer`、`update_message` などのメソッドを通したときだけです。同じ Interaction に、HTTP クライアントで直接応答を送ると、Akari の記録とずれてしまいます。応答には、コンテキストのメソッドを使ってください。

`disnana_akari::http` は `twilight_http` の再エクスポートです。

## メッセージ履歴をページ単位で読む

`MessageHistory` は、ページを要求するたびに Discord から取得します。1ページ最大100件です。取得結果を処理したあとページを破棄すれば、読み進めても全件をメモリに保持しません。

```rust
use disnana_akari::{Error, HistoryCursor, MessageHistory, Result};
use disnana_akari::{http::Client, model::id::{Id, marker::ChannelMarker}};

async fn print_recent(http: &Client, channel: Id<ChannelMarker>) -> Result<()> {
    let mut history = MessageHistory::new(http, channel, HistoryCursor::Latest, 250)
        .map_err(Error::handler)?;
    while let Some(page) = history.next_page().await.map_err(Error::handler)? {
        for message in page {
            println!("{}", message.id);
        }
    }
    Ok(())
}
```

`Latest` と `Before(id)` は新しい順、`After(id)` は古い順です。`Before` と `After` は境界の投稿を含みません。`Around(id)` は指定した投稿付近を一度だけ取得します。再開するには最後に処理した ID を保存し、新しい取得に渡します。Bot に対象チャンネルの閲覧と履歴の取得権限が必要です。 [Discord の履歴仕様](https://docs.discord.com/developers/resources/message#get-channel-messages) · [discord.py の history](https://discordpy.readthedocs.io/en/v2.7.1/api.html#discord.abc.Messageable.history)

## サーバーのメンバーとリアクションをページ単位で読む

`MemberPages` は List Guild Members、`ReactionPages` は Get Reactions を使います。両方ともユーザー ID の昇順で返します。`after` で取得を再開でき、`limit` で総件数を制限できます。既定の1ページはメンバー最大1,000件、リアクション最大100件です。失敗したページではカーソルが進みません。

```rust
use disnana_akari::{Error, MemberPages, ReactionPages, Result};
use disnana_akari::http::{Client, request::channel::reaction::RequestReactionType};
use disnana_akari::model::id::{Id, marker::{ChannelMarker, GuildMarker, MessageMarker}};

async fn print_members(http: &Client, guild: Id<GuildMarker>) -> Result<()> {
    let mut pages = MemberPages::new(http, guild).limit(2_500);
    while let Some(page) = pages.next_page().await.map_err(Error::handler)? {
        for member in page {
            println!("{}", member.user.id);
        }
    }
    Ok(())
}

async fn print_reactions(http: &Client, channel: Id<ChannelMarker>, message: Id<MessageMarker>) -> Result<()> {
    let emoji = RequestReactionType::Unicode { name: "👍" };
    let mut pages = ReactionPages::new(http, channel, message, emoji).limit(250);
    while let Some(page) = pages.next_page().await.map_err(Error::handler)? {
        for user in page {
            println!("{}", user.id);
        }
    }
    Ok(())
}
```

メンバー一覧には Discord 側の特権 Intent `GUILD_MEMBERS` が必要です。リアクションは `kind(ReactionType::Burst)` でスーパーリアクションを指定できます。どちらの API も結果を自動でキャッシュしません。取得中に参加・退出やリアクションの変更があると、ページ間で一覧が変わることがあります。 [Discord のメンバー一覧](https://docs.discord.com/developers/resources/guild#list-guild-members) · [Discord のリアクション一覧](https://docs.discord.com/developers/resources/message#get-reactions)

## BAN・投票者・アーカイブ済みスレッドをページ単位で読む

`BanPages` は BAN 一覧を一度に最大1,000件取得します。ページ内はユーザー ID の昇順です。`before` では、ページを進めるごとに小さい ID の範囲へ移ります。BAN 一覧を読むには Bot に `BAN_MEMBERS` または `VIEW_AUDIT_LOG` が必要です。 [Discord の BAN 一覧](https://docs.discord.com/developers/resources/guild#get-guild-bans)

`PollVoterPages` は一つの選択肢に投票した人を一度に最大100件取得します。`ThreadPages` は `public`、`private`、`joined_private` で対象を選びます。開始位置は公開・非公開一覧ではアーカイブ日時、参加済みの非公開一覧ではスレッド ID で指定します。 [Discord の投票者一覧](https://docs.discord.com/developers/resources/poll#get-answer-voters) · [Discord のアーカイブ済みスレッド一覧](https://docs.discord.com/developers/resources/channel#list-public-archived-threads)

```rust
use disnana_akari::{BanPages, Error, PollVoterPages, Result, ThreadPages};
use disnana_akari::http::Client;
use disnana_akari::model::id::{Id, marker::{ChannelMarker, GuildMarker, MessageMarker}};
use std::num::NonZeroU8;

async fn list_bans(http: &Client, guild: Id<GuildMarker>) -> Result<()> {
    let mut pages = BanPages::new(http, guild).limit(1_200);
    while let Some(page) = pages.next_page().await.map_err(Error::handler)? {
        for ban in page {
            println!("{}", ban.user.id);
        }
    }
    Ok(())
}

async fn list_voters(
    http: &Client, channel: Id<ChannelMarker>, message: Id<MessageMarker>, answer: NonZeroU8,
) -> Result<()> {
    let mut pages = PollVoterPages::new(http, channel, message, answer).limit(200);
    while let Some(page) = pages.next_page().await.map_err(Error::handler)? {
        for voter in page {
            println!("{}", voter.id);
        }
    }
    Ok(())
}

async fn list_archived(http: &Client, channel: Id<ChannelMarker>) -> Result<()> {
    let mut pages = ThreadPages::public(http, channel, None).limit(200);
    while let Some(page) = pages.next_page().await.map_err(Error::handler)? {
        for thread in page.threads {
            println!("{}", thread.id);
        }
    }
    Ok(())
}
```

それぞれ `next_page` を呼ぶまで通信せず、エラーが出た場合は同じページから再試行できます。Discord 側の一覧は取得の途中でも変わることがあります。

## 監査ログ・予定イベントの登録ユーザー・ピン留めを読む

`AuditLogPages` は監査ログを最大100件ずつ取得し、エントリーに関連するユーザーやスレッドなどの情報もページごとに返します。既定は新しい順で、`oldest_first()` または `after(entry_id)` を指定すると古い順です。`actor` と `action` で絞り込めます。Bot に `VIEW_AUDIT_LOG` が必要です。Discord が操作種別を `null` で返す行は読み飛ばします。そのページの項目が空でも、`next_page()` が `None` を返すまで読み進められます。 [Discord の監査ログ API](https://docs.discord.com/developers/resources/audit-log#get-guild-audit-log) · [discord.py の監査ログ実装](https://github.com/Rapptz/discord.py/blob/v2.7.1/discord/guild.py#L4008-L4013)

`ScheduledEventUserPages` は予定イベントに「興味あり」を登録したユーザーを最大100件ずつ取得します。`with_member(true)` を指定すると、Discord が返せる場合にサーバーメンバー情報も取得できます。メンバー内でユーザー情報が省略された場合は、同じ登録項目のユーザー情報を使います。`before` を指定してもページ内は ID の昇順で、ページを進めるごとに小さい ID の範囲に移ります。 [Discord の予定イベント登録ユーザー API](https://docs.discord.com/developers/resources/guild-scheduled-event#get-guild-scheduled-event-users)

`PinPages` はピン留めを最大50件ずつ、ピン留めされた日時の新しい順に取得します。同じピン留め日時がページ境界にまたがると、Discord の日時カーソルで区別できません。スレッドの日時カーソルにも同じ制約があります。 [Discord のピン留め一覧 API](https://docs.discord.com/developers/resources/message#get-channel-pins)

```rust
use disnana_akari::{AuditLogPages, Error, PinPages, Result, ScheduledEventUserPages};
use disnana_akari::http::Client;
use disnana_akari::model::id::{Id, marker::{ChannelMarker, GuildMarker, ScheduledEventMarker}};

async fn print_lists(
    http: &Client, guild: Id<GuildMarker>, event: Id<ScheduledEventMarker>,
    channel: Id<ChannelMarker>,
) -> Result<()> {
    let mut logs = AuditLogPages::new(http, guild).limit(200);
    while let Some(page) = logs.next_page().await.map_err(Error::handler)? {
        for entry in page.entries {
            println!("監査ログ {}", entry.id);
        }
    }
    let mut users = ScheduledEventUserPages::new(http, guild, event).with_member(true);
    while let Some(page) = users.next_page().await.map_err(Error::handler)? {
        for subscription in page {
            println!("登録ユーザー {}", subscription.user.id);
        }
    }
    let mut pins = PinPages::new(http, channel);
    while let Some(page) = pins.next_page().await.map_err(Error::handler)? {
        for pin in page {
            println!("ピン留め {}", pin.message.id);
        }
    }
    Ok(())
}
```

## ファイル付きのメッセージを作る

チャンネルへ新規に投稿する場合は、`create_message` にファイル名とバイト列を渡します。次の例では、本文に含まれるメンションで通知が出ないようにしています。

```rust
use disnana_akari::{CommandContext, Result};
use disnana_akari::model::{
    channel::message::AllowedMentions,
    http::attachment::Attachment,
};

async fn report_file(ctx: CommandContext<'_>) -> Result<()> {
    let files = [Attachment::from_bytes(
        "report.txt".into(),
        b"Requests: 42\n".to_vec(),
        0,
    )];
    ctx.http()
        .create_message(ctx.context().message().channel_id)
        .content("レポートを添付しました。")
        .attachments(&files)
        .allowed_mentions(Some(&AllowedMentions::default()))
        .await?;
    Ok(())
}
```

同じ `Client` を使うと、複数のハンドラーからの REST リクエストも Twilight のレート制限管理を共有します。[Discord のレート制限](https://docs.discord.com/developers/topics/rate-limits)

スレッド、招待、Webhook、予定などの例は [Discord の REST 操作](discord-rest.md) にあります。

## 安定性

Akari は Twilight 0.18 の上に作られていて、公開 API に Twilight の型が出ています（`model`、`http`、`Error` の `Http` など、スラッシュコマンドの `CommandOption`、`Intents`、`Ready`、`InfoEvent` の `InteractionType`）。Twilight は 0.x なので、Twilight の更新に合わせて Akari も、互換性のない変更を入れることがあります。

- Twilight の型を使うときは、`twilight-*` を自分の依存に加えず、`disnana_akari::model` と `disnana_akari::http` を使ってください。版がずれて、型が一致しなくなることを防げます。
- Akari が Twilight の互換性のない版に上げるときは、Akari の 0.x のマイナーバージョンを上げ、[変更履歴](../reference/changelog.md)に書きます。

## Intent と暗号プロバイダー

Intent の指定は、[Intent の設定](intents.md) を参照してください。Akari は Intent を自動では追加しません。

Akari は、プロセス全体の暗号プロバイダー（`rustls` の `CryptoProvider`）を登録しません。`rustls` が最初の使用時に自動で選びます。アプリで別のプロバイダー（`aws-lc-rs` など）も有効にする場合は、起動前に、アプリ側で `CryptoProvider::install_default` を呼んでください。
