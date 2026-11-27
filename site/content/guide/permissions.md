# 権限の確認

コマンドを「サーバーの管理者だけ」「BAN の権限を持つ人だけ」に絞るには、ハンドラーの先頭で確認します。Akari が黙って確認することはなく、**呼ぶと確認し、通らなければエラーを返す** だけです（`?` で抜けます）。

スラッシュコマンド、テキストコマンド、ハイブリッドコマンドのどれでも、同じ名前のメソッドで確認できます（テキストコマンドとハイブリッドは `.await` が付きます）。

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

## テキストコマンドで確認する

メッセージには権限が付いていないため、Akari が Discord からサーバーとチャンネルの情報を取得して計算します（Twilight 公式の `twilight-util` の計算を使います）。ロールやチャンネルごとの上書きを反映し、サーバーのオーナーと管理者はすべての権限を持つものとして扱います。

スレッドでは親チャンネルの上書きを使い、添付や埋め込みなどの権限はスレッドに投稿できるかも見て判定します。また、メッセージに有効なタイムアウトの期限が入っている場合、一般メンバーの権限を閲覧と履歴の読み取りに絞ります。

```rust
use disnana_akari::model::guild::Permissions;
use disnana_akari::{CommandContext, Result};

async fn purge(ctx: CommandContext<'_>) -> Result<()> {
    ctx.require_permissions(Permissions::MANAGE_MESSAGES).await?;
    ctx.reply("確認に通りました。").await
}
```

DM では `CheckFailure::GuildOnly` になります。

> [!TIP]
> 確認のたびに Discord へ問い合わせます。呼び出しが多い Bot は、`Bot::cache_permissions(Duration::from_secs(60))` で取得した情報を一定時間だけ保存できます。`Intents::GUILDS` が有効なら、ロールやチャンネルの変更イベントを受けた時点で該当する保存を破棄し、次の権限確認で情報を取得し直します。イベントが届かない場合は保存期間が反映の目安です。スラッシュコマンドの権限は Discord が送るため、この設定の影響を受けません。

## 複数のコマンドで同じ確認を使う

`CommandCheck` はテキストとアプリコマンドの両方に登録できます。次の例では、`maintenance` グループだけでサーバーと権限を確認します。正式な名前で判定するため、テキストの別名から実行しても同じ確認がかかります。

```rust
use disnana_akari::{CommandCheck, CommandCheckContext, Commands, Result, SlashCommands};
use disnana_akari::model::guild::Permissions;

fn install_checks(text: &mut Commands, slash: &mut SlashCommands) {
    let maintenance = CommandCheck::new(async |ctx: CommandCheckContext<'_>| -> Result<()> {
        if ctx.command_path().next() == Some("maintenance") {
            ctx.require_guild()?;
            ctx.require_permissions(Permissions::MANAGE_MESSAGES).await?;
        }
        Ok(())
    });

    text.add_check(maintenance.clone());
    slash.add_check(maintenance);
}
```

複数のチェックは登録順に実行し、最初のエラーで止めます。Bot 独自の条件で拒否する場合は `Err(CheckFailure::Rejected.into())` を返します。コマンドでは既存の拒否応答を使い、補完を拒否した場合は候補を空で返します。チェックは応答用のメソッドを持たないため、処理の結果を返すのはコマンドハンドラーに任せます。 チェックの時間もアプリコマンドの初回応答期限（約3秒）に含まれます。長い処理は、ハンドラーが応答を defer した後に実行できる構成にします。

## 確認に通らなかったとき

確認が失敗すると、`Error::Check(CheckFailure)` が返ります。

| `CheckFailure` | 意味 |
| --- | --- |
| `GuildOnly` | サーバーの外（DM など）で呼ばれた |
| `MissingPermissions(perms)` | 足りない権限（`perms`）がある |
| `PermissionsUnavailable` | 権限の情報が届かなかった（安全のため、実行させません） |

何もしなければ、Akari が `Messages::check_failed`（既定は「このコマンドを使う権限がありません。」）を返し、`Bot::on_error` にも報告します。スラッシュコマンドでは本人にだけ、テキストコマンドでは元の投稿への返信になります。文言を変える・送らない設定は[エラーと自動で送る文言](errors.md)を見てください。

公開の `defer()` 後に権限チェックが失敗した場合、公開の待機表示は「処理を終了しました。」に変わり、`check_failed` の文言は本人への追加応答で届きます。待機表示も非公開にするには `defer_ephemeral()` を使います。

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
