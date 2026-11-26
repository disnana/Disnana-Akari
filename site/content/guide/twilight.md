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

## Intent と暗号プロバイダー

Intent の指定は、[Intent の設定](intents.md) を参照してください。Akari は Intent を自動では追加しません。

Akari は、プロセス全体の暗号プロバイダー（`rustls` の `CryptoProvider`）を登録しません。`rustls` が最初の使用時に自動で選びます。アプリで別のプロバイダー（`aws-lc-rs` など）も有効にする場合は、起動前に、アプリ側で `CryptoProvider::install_default` を呼んでください。
