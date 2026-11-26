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

## Intent を調整する

Akari は、使う機能に合わせて Gateway Intent を自動で選びます。テキストコマンドやメッセージのハンドラーを使うと `GUILD_MESSAGES` と `MESSAGE_CONTENT` を要求し、スラッシュコマンドとボタンだけなら、どちらも要求しません。

足したい Intent があるときは `add_intents`、完全に置き換えたいときは `with_intents` を使います。

```rust
use disnana_akari::model::gateway::Intents;
use disnana_akari::{Bot, Result};

fn build() -> Result<Bot> {
    Ok(Bot::from_env()?.add_intents(Intents::GUILD_MEMBERS))
}
```

> [!WARNING]
> `with_intents` で置き換えるときは、テキストコマンドを使うなら `GUILD_MESSAGES` と `MESSAGE_CONTENT` を **自分で含めてください**。含めないと、コマンドに反応しなくなります。
