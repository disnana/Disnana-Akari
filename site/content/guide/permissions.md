# 権限の確認

コマンドを「サーバーの管理者だけ」「BAN の権限を持つ人だけ」に絞るには、ハンドラーの先頭で確認します。Akari が黙って確認することはなく、**呼ぶと確認し、通らなければエラーを返す** だけです（`?` で抜けます）。

> [!NOTE]
> 現在対応しているのは **スラッシュコマンド** です。テキストコマンドとハイブリッドの権限確認は、今後追加します。

## ハンドラーで確認する

```rust
use disnana_akari::model::guild::Permissions;
use disnana_akari::{Result, SlashContext};

async fn ban(ctx: &mut SlashContext<'_>) -> Result<()> {
    // 呼び出した人が BAN_MEMBERS を持っていなければ、ここでエラーになる
    ctx.require_permissions(Permissions::BAN_MEMBERS)?;
    ctx.reply("確認に通りました。").await
}
```

| メソッド | 動作 |
| --- | --- |
| `ctx.require_guild()` | サーバーの中で呼ばれたことを確認し、サーバー ID を返します |
| `ctx.require_permissions(perms)` | 呼び出した人が、指定した権限をすべて持つことを確認します |
| `ctx.guild_id()` | サーバー ID（DM では `None`） |
| `ctx.permissions()` | 呼び出した人の、このチャンネルでの権限 |

`ctx.permissions()` は、Discord がロールとチャンネルごとの上書きを反映して計算した値です。管理者はすべての権限を持つものとして返ります。

## 確認に通らなかったとき

確認が失敗すると、`Error::Check(CheckFailure)` が返ります。

| `CheckFailure` | 意味 |
| --- | --- |
| `GuildOnly` | サーバーの外（DM など）で呼ばれた |
| `MissingPermissions(perms)` | 足りない権限（`perms`）がある |
| `PermissionsUnavailable` | 権限の情報が届かなかった（安全のため、実行させません） |

何もしなければ、Akari が本人にだけ `Messages::check_failed`（既定は「このコマンドを使う権限がありません。」）を返し、`Bot::on_error` にも報告します。文言を変える・送らない設定は[エラーと自動で送る文言](errors.md)を見てください。

足りない権限の名前を使って、自分で返信することもできます。

```rust
use disnana_akari::model::guild::Permissions;
use disnana_akari::{CheckFailure, Error, Result, SlashContext};

async fn ban(ctx: &mut SlashContext<'_>) -> Result<()> {
    match ctx.require_permissions(Permissions::BAN_MEMBERS) {
        Err(Error::Check(CheckFailure::MissingPermissions(missing))) => {
            return ctx
                .reply_ephemeral(&format!("足りない権限: {missing:?}"))
                .await;
        }
        other => other?,
    }
    ctx.reply("確認に通りました。").await
}
```

## コマンドを画面から隠す

`SlashCommands::default_member_permissions` を使うと、指定した権限を持たない人には、Discord の画面にコマンドが出なくなります。

```rust
use disnana_akari::model::guild::Permissions;
use disnana_akari::{Result, SlashCommands, SlashContext};

async fn ban(_: &mut SlashContext<'_>) -> Result<()> {
    Ok(())
}

fn register(slash: &mut SlashCommands) -> Result<()> {
    slash.register("ban", "メンバーを BAN します", vec![], ban)?;
    slash.default_member_permissions("ban", Permissions::BAN_MEMBERS)
}
```

> [!WARNING]
> サーバーの管理者は、この既定を Discord の設定で変えられます。**これだけに頼らず、ハンドラーでも `require_permissions` を呼んでください。**
