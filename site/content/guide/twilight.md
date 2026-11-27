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

## 安定性

Akari は Twilight 0.18 の上に作られていて、公開 API に Twilight の型が出ています（`model`、`http`、`Error` の `Http` など、スラッシュコマンドの `CommandOption`、`Intents`、`Ready`、`InfoEvent` の `InteractionType`）。Twilight は 0.x なので、Twilight の更新に合わせて Akari も、互換性のない変更を入れることがあります。

- Twilight の型を使うときは、`twilight-*` を自分の依存に加えず、`disnana_akari::model` と `disnana_akari::http` を使ってください。版がずれて、型が一致しなくなることを防げます。
- Akari が Twilight の互換性のない版に上げるときは、Akari の 0.x のマイナーバージョンを上げ、[変更履歴](../reference/changelog.md)に書きます。

## Intent と暗号プロバイダー

Intent の指定は、[Intent の設定](intents.md) を参照してください。Akari は Intent を自動では追加しません。

Akari は、プロセス全体の暗号プロバイダー（`rustls` の `CryptoProvider`）を登録しません。`rustls` が最初の使用時に自動で選びます。アプリで別のプロバイダー（`aws-lc-rs` など）も有効にする場合は、起動前に、アプリ側で `CryptoProvider::install_default` を呼んでください。
