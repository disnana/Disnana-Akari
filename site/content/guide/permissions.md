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

## 権限を名前で調べる

数値やビットを書かずに、権限を名前で調べられます。`use disnana_akari::prelude::*;` で取り込みます。

```rust
use disnana_akari::model::guild::Permissions;
use disnana_akari::prelude::*;
use disnana_akari::{Result, SlashContext};

async fn info(ctx: &mut SlashContext<'_>) -> Result<()> {
    let held = ctx.permissions().unwrap_or_else(Permissions::empty);
    let mut text = String::new();
    if held.manage_messages() {
        text.push_str("メッセージを管理できます。\n");
    }
    // 足りない権限を、日本語の名前で並べる
    let missing: Vec<_> = (Permissions::moderator() - held).labels().collect();
    if !missing.is_empty() {
        text.push_str(&format!("足りない権限: {}", missing.join("、")));
    }
    ctx.reply_ephemeral(&text).await
}
```

| メソッド | 動作 |
| --- | --- |
| `perms.manage_messages()` など | 権限の定数名を小文字にした名前で、持っているかを返します |
| `perms.names()` | 持っている権限の定数名（`MANAGE_MESSAGES` など）を返します |
| `perms.labels()` | 持っている権限の日本語名を返します |
| `perms.unknown_bits()` | 名前の表にないビットです。Discord が権限を追加したとき、黙って捨てずに確認できます |
| `Permissions::text()`、`voice()`、`moderator()` | 目的ごとの権限のまとまりです |

追加や削除には、ビットフラグ標準の `insert`、`remove`、`set` を使います。

同じ方法が、メッセージ（`MessageFlagsExt`）、ユーザーのバッジ（`UserFlagsExt`）、システムチャンネル（`SystemChannelFlagsExt`）、チャンネル（`ChannelFlagsExt`）のフラグにも使えます。チャンネルの種類や認証レベルなどの設定は、`label()` で日本語の名前にできます。

```rust
use disnana_akari::model::{channel::ChannelType, guild::VerificationLevel};
use disnana_akari::prelude::*;

fn describe(kind: ChannelType, level: VerificationLevel) -> String {
    format!("{} / 認証レベル: {}", kind.label(), level.label())
}
```

## 必要な権限を登録時に宣言する

権限の確認を、ハンドラーの中ではなく、登録のときにまとめて宣言できます。`Needs` に、実行者と Bot に必要な権限を書き、`require` で、コマンドに結び付けます。確認は、ハンドラーの前に Akari が行い、足りなければハンドラーを実行しません。

```rust
use disnana_akari::model::guild::Permissions;
use disnana_akari::{Commands, Needs, Result, SlashCommands, SlashContext};

fn register(slash: &mut SlashCommands, commands: &mut Commands) -> Result<()> {
    slash.register("purge", "メッセージを消します", vec![], async |ctx: &mut SlashContext<'_>| -> Result<()> {
        // ここに来た時点で、実行者と Bot の権限は確認済み
        ctx.reply_ephemeral("消します。").await
    })?;
    let needs = Needs::new()
        .author(Permissions::MANAGE_MESSAGES)
        .bot(Permissions::MANAGE_MESSAGES | Permissions::READ_MESSAGE_HISTORY);
    slash.require("purge", needs)?;

    // テキストコマンドも、同じ宣言を使えます
    commands.register("purge", async |ctx| -> Result<()> { ctx.reply("消します。").await })?;
    commands.require("purge", needs)?;
    Ok(())
}
```

- 実行者の権限が足りなければ `CheckFailure::MissingPermissions`、Bot の権限が足りなければ `CheckFailure::BotMissingPermissions` で失敗します。実行者を先に確認します。
- グループやコマンドを指定すると、配下のサブコマンドにも適用します（`"config"` と宣言すると、`config set` にも効きます）。サブコマンドだけを指定することもできます（`"config set"`）。
- 登録されていないコマンドやサブコマンドを指定すると、登録の時点で失敗します。名前の打ち間違いで、確認が効かないまま動くことはありません。
- スラッシュコマンドでは、実行者の権限を、最上位のコマンドの Discord 側の既定の表示条件にも反映します（すでに設定済みなら、変えません）。サーバーの管理者は、この既定を変えられるので、確認の本体は、実行時の宣言です。
- サーバーの外（DM）で呼ばれたときは、権限を確認できないので、拒否します。
- コンテキストメニューには使えません。`add_check` で、共通のチェックを使ってください。
- ハイブリッドコマンドは、テキストとスラッシュの両方に、同じ `Needs` を渡します。
- 独自のチェックの中でも、`ctx.access()` で、ここまでの確認を使えます。

## Bot の権限、ほかのメンバー、ロールの階層を確認する

`ctx.access()` で取得する `Access` に、実行者以外の確認がまとまっています。スラッシュコマンドでは、実行者と Bot の権限を Discord が送ってくるので、通信は要りません。

```rust
use disnana_akari::model::guild::Permissions;
use disnana_akari::{Result, SlashContext};

async fn purge(ctx: &mut SlashContext<'_>) -> Result<()> {
    let access = ctx.access();
    // 実行者にも、Bot にも、必要な権限があることを確認する（足りなければ、ここで失敗する）
    access.require_permissions(Permissions::MANAGE_MESSAGES).await?;
    access
        .require_bot_permissions(Permissions::MANAGE_MESSAGES | Permissions::READ_MESSAGE_HISTORY)
        .await?;
    ctx.reply_ephemeral("確認に通りました。").await
}
```

| メソッド | 動作 |
| --- | --- |
| `access.can(perms).await?` | 実行者が持っているかを返します（拒否はしません） |
| `access.missing(perms).await?` | 実行者に足りない権限を返します |
| `access.bot_can(perms)`、`bot_missing(perms)` | Bot について、同じことを調べます |
| `access.permissions_of(user_id).await?` | 指定したメンバーの、このチャンネルでの権限を返します（メンバーを 1 回取得します） |
| `access.require_permissions(perms)`、`require_bot_permissions(perms)` | 足りなければ失敗します（Bot の場合は `BotMissingPermissions`） |
| `access.snapshot().await?` | 実行者と Bot の権限と、実行者が所有者かどうかを、まとめて取得します（`PermissionSnapshot`） |
| `access.require_guild_owner().await?` | 実行者がサーバーの所有者であることを確認します |
| `access.require_role(role)`、`require_any_role(&[..])` | 実行者が、指定したロールを持つことを確認します（通信なし） |
| `access.require_can_moderate(user_id).await?` | ロールの階層から、実行者と Bot が対象を操作できることを確認します |

判断できないときは、許可せずに失敗します。「持っていない」と偽って返すこともありません。

### ロールの階層

`require_can_moderate` は、BAN やキック、ニックネームの変更などの前に呼びます。Discord に送れば 403 で失敗する操作を、送る前に、理由つきで止められます。

```rust
use disnana_akari::model::{guild::Permissions, id::{Id, marker::UserMarker}};
use disnana_akari::{CheckFailure, Error, HierarchyDenied, Result, SlashContext};

async fn kick(ctx: &mut SlashContext<'_>, target: Id<UserMarker>) -> Result<()> {
    let access = ctx.access();
    access.require_permissions(Permissions::KICK_MEMBERS).await?;
    access.require_bot_permissions(Permissions::KICK_MEMBERS).await?;
    match access.require_can_moderate(target).await {
        Err(Error::Check(CheckFailure::Hierarchy(HierarchyDenied::TargetIsOwner))) => {
            return ctx.reply_ephemeral("サーバーの所有者は操作できません。").await;
        }
        other => other?,
    }
    // ここまで来れば、Discord に送っても階層では失敗しない
    ctx.reply_ephemeral("操作できます。").await
}
```

- サーバーの所有者は、誰にも操作されません。
- 最上位ロールが、対象と同じ位置か低いときは、拒否します。管理者権限でも、階層は免除されません。
- 対象のメンバーと Bot のメンバーを、それぞれ 1 回取得します。対象がサーバーにいないときは、HTTP のエラーです。

## チャンネルの権限の上書きと、ロールの権限を読み書きする

チャンネルの権限の上書きを、数値やビットを書かずに、許可、拒否、継承の 3 値で読み書きできます。

```rust
use disnana_akari::model::{guild::Permissions, id::{Id, marker::{ChannelMarker, RoleMarker}}};
use disnana_akari::{Overwrite, OverwriteTarget, Result, SlashContext, Tri};

async fn lock(
    ctx: &mut SlashContext<'_>,
    channel: Id<ChannelMarker>,
    members: Id<RoleMarker>,
) -> Result<()> {
    let access = ctx.access();
    // 実行した人が、操作してよいかは、呼ぶ側で確認する（Akari は確認しません）
    access.require_permissions(Permissions::MANAGE_ROLES).await?;

    let target = OverwriteTarget::Role(members);
    // いまの設定を読む（キャッシュは使いません）
    let already = access
        .overwrite_of(channel, target)
        .await?
        .is_some_and(|current| current.state(Permissions::SEND_MESSAGES) == Some(Tri::Deny));
    if !already {
        // 1 つの対象だけを更新します。ほかの対象の設定は変えません
        access
            .set_overwrite(channel, target, Overwrite::new().deny(Permissions::SEND_MESSAGES), "メンテナンスのため")
            .await?;
    }
    ctx.reply_ephemeral("発言を止めました。").await
}
```

| メソッド | 動作 |
| --- | --- |
| `access.overwrite_of(channel, target).await?` | 1 つの対象の上書きを読みます（なければ `None`） |
| `access.set_overwrite(channel, target, overwrite, reason).await?` | 1 つの対象の上書きを設定します |
| `access.delete_overwrite(channel, target, reason).await?` | 1 つの対象の上書きを削除します（すべて継承に戻ります） |
| `access.role_permissions(role).await?` | ロールの権限を読みます |
| `access.set_role_permissions(role, permissions, reason).await?` | ロールの権限を置き換えます |

- `Overwrite` は、`allow`、`deny`、`inherit` で、権限ごとの状態を指定します。同じ権限を指定し直すと、前の指定を置き換えます。`state(権限)` で、`Tri::Allow`、`Tri::Deny`、`Tri::Inherit` を読めます。
- **書き込みの前に、Bot の権限を確認します。** Bot が `MANAGE_ROLES` を持たない、Bot 自身が持たない権限を付けようとした（管理者は除く）、対象のロールが Bot の最上位ロール以上、のどれかなら、通信せずに失敗します。
- **実行した人への確認は、しません。** `require_permissions` などで、先に確認してください。
- 理由は監査ログに残る文字列で、512 文字以内です。
- ロールの権限は、Discord が全体を置き換える仕様です。`role_permissions` で現在の値を読み、足したい権限だけを足して渡してください。読んでから書くまでの間に別の変更があると、それを上書きします。
- チャンネルの上書きは、1 つの対象だけを更新するので、ほかの対象の設定は巻き戻りません。

## メンバーを管理する（キック、BAN、タイムアウトなど）

`ctx.manage()` で、メンバーとロール、ボイスチャンネルの管理操作ができます。どの操作も、**Discord に送る前に、Bot の権限とロールの階層を確認し、足りなければ通信せずに失敗します。** 実行した人に操作してよいかは、確認しません。先に `require_permissions` と `require_can_moderate` で確認してください。

```rust
use std::time::Duration;

use disnana_akari::model::{guild::Permissions, id::{Id, marker::UserMarker}};
use disnana_akari::{Result, SlashContext};

async fn mute(ctx: &mut SlashContext<'_>, target: Id<UserMarker>) -> Result<()> {
    let access = ctx.access();
    // 実行した人の権限と階層を、先に確認する（Akari は確認しません）
    access.require_permissions(Permissions::MODERATE_MEMBERS).await?;
    access.require_can_moderate(target).await?;

    // Bot の権限と階層は、manage の中で確認される。10 分のタイムアウト
    ctx.manage()
        .timeout_member(target, Duration::from_secs(600), "スパムのため")
        .await?;
    ctx.reply_ephemeral("タイムアウトしました。").await
}
```

| メソッド | 必要な Bot の権限 | 補足 |
| --- | --- | --- |
| `kick_member(user, reason)` | `KICK_MEMBERS` | ロール階層を確認します |
| `ban_member(user, delete_messages, reason)` | `BAN_MEMBERS` | サーバーにいないユーザーも BAN できます。削除するメッセージは 7 日以内 |
| `unban_member(user, reason)` | `BAN_MEMBERS` | |
| `timeout_member(user, duration, reason)` | `MODERATE_MEMBERS` | 28 日以内。`Duration::ZERO` で解除 |
| `set_nickname(user, nickname, reason)` | `MANAGE_NICKNAMES` | `None` で解除 |
| `add_role(user, role, reason)`、`remove_role(...)` | `MANAGE_ROLES` | ロールが Bot の最上位ロールより低い必要があります |
| `move_member(user, channel, reason)` | `MOVE_MEMBERS` | `None` で切断 |
| `set_voice_mute(...)`、`set_voice_deafen(...)` | `MUTE_MEMBERS`、`DEAFEN_MEMBERS` | |

- 失敗の理由は、`CheckFailure::BotMissingPermissions`（Bot の権限不足）、`CheckFailure::Hierarchy`（階層）、`InvalidArgument`（タイムアウトや削除期間が Discord の範囲外）で分かります。
- 理由は監査ログに残る文字列で、512 文字以内です。
- 1 つの操作は、Discord への 1 回のリクエストです（確認のための取得は別です）。

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
