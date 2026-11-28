# Embed とボタン

見栄えのよい返信（Embed）と、押せるボタンを作ります。`disnana_akari::ui` に、部品を組み立てる型がそろっています。

## Embed を送る

```rust
use disnana_akari::ui::{Embed, UiMessage};
use disnana_akari::{Result, SlashContext};

async fn about(ctx: &mut SlashContext<'_>) -> Result<()> {
    let message = UiMessage::new().embed(
        Embed::new("Disnana Akari")
            .description("Rust 製の Discord Bot フレームワークです。")
            .color(0x5865F2)
            .field("言語", "Rust", true)
            .field("土台", "Twilight", true),
    );
    ctx.reply_ui(&message).await
}
```

| ビルダー | 動作 |
| --- | --- |
| `UiMessage::new().content(text)` | 本文を設定します。 |
| `.embed(embed)` | Embed を追加します（最大 10 個）。 |
| `.buttons(vec![...])` | ボタンを 1 行追加します（1 行に最大 5 個、最大 5 行）。 |
| `Embed::new(title)` | タイトルを持つ Embed を作ります。 |
| `.description(text)` / `.color(rgb)` | 説明と、`0x5865F2` のような RGB の色を設定します。 |
| `.field(name, value, inline)` | 項目を追加します。`inline` が `true` なら、横に並べられます。 |

> [!NOTE]
> テキストコマンドの返信に Embed を付けるときは、Bot に **Embed Links** の権限が必要です。

送信前に、文字数や個数が検証されます。本文は 2,000 文字、Embed は合計 6,000 文字までです。Discord が拒否する内容は、送信前の `Err` になります。

## ボタンを付ける

Akari の処理を呼ぶボタンには `custom_id` を付けます。同じ文字列で `events.on_button` に関数を登録すると、押されたときにその関数が呼ばれます。

```rust
use disnana_akari::ui::{Button, Embed, UiMessage};
use disnana_akari::{Bot, ButtonContext, Events, Result, SlashCommands, SlashContext};
use disnana_akari::model::id::Id;

// 表示側と登録側で同じ値を使うので、定数にしておく
const CONFIRM: &str = "akari:example:confirm";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut slash = SlashCommands::for_guild(Id::new(123456789012345678));
    slash.register("confirm", "確認ボタンを表示します", vec![], show)?;

    let mut events = Events::new();
    events.on_button(CONFIRM, confirm)?;

    Bot::from_env()?
        .with_events(events)
        .with_slash_commands(slash)
        .start()
        .await
}

async fn show(ctx: &mut SlashContext<'_>) -> Result<()> {
    let message = UiMessage::new()
        .embed(Embed::new("確認").description("下のボタンを押してください。"))
        .buttons(vec![Button::primary(CONFIRM, "確認する")]);
    ctx.reply_ui(&message).await
}

async fn confirm(ctx: &mut ButtonContext<'_>) -> Result<()> {
    ctx.reply_ephemeral("確認しました。").await
}
```

> [!TIP]
> `custom_id` は 1〜100 文字です。`akari:機能名:操作` のように、ほかと重ならない名前にしておくと安心です。同じメッセージの中で `custom_id` が重複すると、送信時にエラーになります。

## ボタンの種類

| 作り方 | 見た目 |
| --- | --- |
| `Button::primary(id, label)` | 青（主な操作） |
| `Button::secondary(id, label)` | グレー（控えめな操作） |
| `Button::success(id, label)` | 緑（確認、完了） |
| `Button::danger(id, label)` | 赤（削除など） |
| `Button::link(url, label)` | URL を開く。押してもイベントは届きません。 |
| `Button::premium(sku_id)` | 指定された SKU の購入画面を開く。Discord が表示と購入操作を扱います。 |

```rust
use disnana_akari::model::channel::message::EmojiReactionType;
use disnana_akari::ui::Button;

fn buttons() -> Vec<Button> {
    vec![
        Button::success("akari:demo:ok", "OK")
            .emoji(EmojiReactionType::Unicode { name: "✅".into() }),
        Button::danger("akari:demo:cancel", "やめる").disabled(true),
        Button::link("https://github.com/disnana/Disnana-Akari", "GitHub"),
    ]
}
```

- `.emoji(...)` で絵文字を付けられます。
- `.disabled(true)` で、押せない状態にします。
- リンクボタンの URL は、512 文字以内の絶対 HTTP(S) です。

### 購入ボタン

購入できるアプリの SKU を、`Id<SkuMarker>` で渡します。購入ボタンは従来形式と Components V2 の両方で使えます。ボタンを押すと Discord が購入画面を開きます。

```rust
use disnana_akari::model::id::{Id, marker::SkuMarker};
use disnana_akari::ui::{Button, UiMessage};

fn purchase(sku_id: Id<SkuMarker>) -> UiMessage {
    UiMessage::new().buttons(vec![Button::premium(sku_id)])
}
```

## ボタンのメッセージを書き換える

ボタンを押した結果を、同じメッセージに反映したいときは `update_message` です。**本文・Embed・ボタンをまとめて置き換える** ので、残したい部品も新しいメッセージに含めます。

```rust
use disnana_akari::ui::{Button, Embed, UiMessage};
use disnana_akari::{ButtonContext, Result};

const COUNT: &str = "akari:demo:count";

fn panel(count: u32) -> UiMessage {
    UiMessage::new()
        .embed(Embed::new("カウンター").description(format!("現在 {count} 回です。")))
        .buttons(vec![Button::primary(COUNT, "+1")])
}

async fn on_count(ctx: &mut ButtonContext<'_>) -> Result<()> {
    // メッセージ本文から現在の値を読み取る例（本来は DB などに保存します）
    let current = ctx
        .message()
        .and_then(|m| m.embeds.first())
        .and_then(|e| e.description.as_deref())
        .and_then(|d| d.split_whitespace().nth(1))
        .and_then(|n| n.parse::<u32>().ok())
        .unwrap_or(0);
    ctx.update_message(&panel(current + 1)).await
}
```

## 画面の状態の持たせ方

Akari にはボタンごとのオブジェクト（d.py の View）がありません。画面を変えるときは、今の状態を取り出して、新しい `UiMessage` を組み立て直します。状態の置き場所は、次の 3 つから選びます。

| 状態の種類 | 置き場所 | 取り出し方 |
| --- | --- | --- |
| ページ番号や対象 ID など、短い値 | ボタンの ID（`DynamicId`、100 文字まで） | `ctx.dynamic_id()` |
| すでに画面に出ている内容 | 押されたメッセージ | `ctx.message()` |
| 画面に出さない状態、大きな状態 | アプリの DB など | 対象の ID から検索する |

`ctx.message()` は、Discord が操作と一緒に送ってきた内容を借りるだけなので、追加の API リクエストはありません。`update_message` も、その操作への応答です。

次の例は、ページ番号をボタンの ID に載せて、ページ送りをします。表示から読み取らないので、画面の文言を変えても壊れません。

```rust
use disnana_akari::ui::{Button, DynamicId, Embed, UiMessage};
use disnana_akari::{ButtonContext, Events, Result};

const PAGES: [&str; 3] = ["1 ページ目", "2 ページ目", "3 ページ目"];

fn page(index: usize) -> Result<UiMessage> {
    let previous = DynamicId::new("page", &index.saturating_sub(1).to_string())?;
    let next = DynamicId::new("page", &(index + 1).min(PAGES.len() - 1).to_string())?;
    Ok(UiMessage::new()
        .embed(Embed::new("ページ").description(PAGES[index]))
        .buttons(vec![
            Button::secondary(previous, "前へ").disabled(index == 0),
            Button::primary(next, "次へ").disabled(index + 1 == PAGES.len()),
        ]))
}

fn register_pages(events: &mut Events) -> Result<()> {
    events.on_dynamic_button("page", async |ctx: &mut ButtonContext<'_>| -> Result<()> {
        let Some(id) = ctx.dynamic_id() else {
            return Ok(());
        };
        let index: usize = id.value().parse().map_err(disnana_akari::Error::handler)?;
        let index = index.min(PAGES.len() - 1);
        ctx.update_message(&page(index)?).await
    })
}
```

押した人だけに操作させたいときは、`ctx.author_id()` と、メッセージを作った人（スラッシュコマンドの実行者など）の ID を比べます。作った人の ID は、`DynamicId` に載せるか、DB に保存します。

## 時間のかかる処理

ボタンも 3 秒以内の応答が必要です。長くかかるときは、先に `defer_update` で受け付けて、あとで `update_message` や `followup_ephemeral` を呼びます。作業結果をファイル付きの追加メッセージとして送る場合は、`followup_files` または `followup_files_ephemeral` を使います。[追加のメッセージにファイルを添付する](responding.md#追加のメッセージにファイルを添付する)

```rust
use disnana_akari::ui::UiMessage;
use disnana_akari::{ButtonContext, Result};

async fn slow(ctx: &mut ButtonContext<'_>) -> Result<()> {
    ctx.defer_update().await?;                        // まず受け付ける
    // ここで時間のかかる処理を待つ
    ctx.update_message(&UiMessage::new().content("更新しました。")).await?;
    ctx.followup_ephemeral("完了しました。").await    // 本人にだけ通知
}
```

## 操作への非公開返信を編集する

ボタンや選択メニューへの `reply_ephemeral` の後は、`original_response` で返信を取得し、`edit_original_response`、`edit_original_ui`、`delete_original_response` で編集や削除ができます。

```rust
use disnana_akari::{ButtonContext, Result};

async fn confirm(ctx: &mut ButtonContext<'_>) -> Result<()> {
    ctx.reply_ephemeral("この操作を受け付けました。").await?;
    ctx.edit_original_response("操作が完了しました。").await
}
```

Components V2 を非公開で返すときは `reply_v2_ephemeral`、その返信を編集するときは `edit_original_v2` を使います。ボタンが付いた元の投稿を編集する場合は、引き続き `update_message` や `update_v2` を使います。`defer_update` は元の投稿を更新するための応答です。

## ボタンの操作者を確認する

> [!WARNING]
> ボタンは、そのメッセージを見られる **誰でも押せます**。「コマンドを実行した本人だけ」「管理者だけ」といった制限は、ハンドラーで確認する必要があります。

`ctx.author_id()` が「押した人」、`ctx.invoker_id()` が「そのメッセージを作るきっかけになった人」です。2 つが同じかどうかで、本人かどうかが分かります。

```rust
use disnana_akari::{ButtonContext, Result};

async fn owner_only(ctx: &mut ButtonContext<'_>) -> Result<()> {
    let is_owner = matches!(
        (ctx.author_id(), ctx.invoker_id()),
        (Some(author), Some(invoker)) if author == invoker
    );
    if !is_owner {
        return ctx.reply_ephemeral("コマンドを実行した本人だけが押せます。").await;
    }
    ctx.reply_ephemeral("OK です。").await
}
```

テキストコマンドへの返信では、Discord が返信先を送ってこないことがあります。そのときは `resolve_invoker_id()` で取得します。通信が入るので、**先に `defer_update` で受け付けて** から呼んでください。

```rust
use disnana_akari::{ButtonContext, Result};

async fn confirm(ctx: &mut ButtonContext<'_>) -> Result<()> {
    ctx.defer_update().await?;

    let owner = ctx.resolve_invoker_id().await?;
    if !matches!((ctx.author_id(), owner), (Some(a), Some(o)) if a == o) {
        return ctx.followup_ephemeral("元の投稿者だけが操作できます。").await;
    }
    ctx.followup_ephemeral("操作を受け付けました。").await
}
```

返信先が削除されていれば `Ok(None)` が返ります。履歴を読む権限がないなど、取得に失敗したときは `Err` です。

## 再起動後も同じボタンを使う

同じ操作を複数のメッセージに付ける場合、`DynamicId` で機能名と対象の識別子を組み合わせます。機能名で一度登録すれば、それぞれのボタンを押したときに対象の識別子を受け取れます。

```rust
use disnana_akari::{ButtonContext, Events, Result, SlashContext};
use disnana_akari::ui::{Button, DynamicId, UiMessage};

async fn show_project(ctx: &mut SlashContext<'_>, project: u64) -> Result<()> {
    let id = DynamicId::new("project", &project.to_string())?;
    let message = UiMessage::new().buttons(vec![Button::primary(id, "番号を表示")]);
    ctx.reply_ui(&message).await
}

fn register_project_buttons(events: &mut Events) -> Result<()> {
    events.on_dynamic_button("project", async |ctx: &mut ButtonContext<'_>| -> Result<()> {
        let Some(id) = ctx.dynamic_id() else {
            return Ok(());
        };
        let project: u64 = id.value().parse().map_err(disnana_akari::Error::handler)?;
        ctx.reply_ephemeral(&format!("プロジェクト番号は {project} です。")).await
    })
}
```

再起動時にも同じ機能名で登録すると、Discord に残っているボタンを処理できます。Akari は個々のメッセージを保存しません。対象や権限は操作のたびにアプリ側で確認し、識別子に機密情報を入れないでください。選択メニューとモーダルも同じ方法で登録できます。

## 確定か取消を期限付きで待つ

`Bot::with_confirmations` で確認セッションを有効にします。返信を送ってから `bind` でそのメッセージを登録すると、待っている間に実行者本人が押したボタンだけを受け付けます。`wait` は確定、取消、期限切れなどを返します。

```rust
use disnana_akari::{Bot, ConfirmationLimits, ConfirmationOutcome, Confirmations, Error, Result, SlashContext};
use disnana_akari::ui::{Button, UiMessage};
use std::time::Duration;

fn configure(bot: Bot) -> Result<(Bot, Confirmations)> {
    bot.with_confirmations(ConfirmationLimits::default()).map_err(Error::handler)
}

async fn confirm(ctx: &mut SlashContext<'_>, confirmations: &Confirmations) -> Result<()> {
    let owner = ctx.author_id().ok_or_else(|| Error::msg("実行者が分かりません"))?;
    let confirmation = confirmations
        .begin(ctx.interaction_id(), owner, Duration::from_secs(30))
        .map_err(Error::handler)?;
    let prompt = UiMessage::new().content("保存しますか？").buttons(vec![
        Button::success(confirmation.confirm_id(), "確定"),
        Button::danger(confirmation.cancel_id(), "取消"),
    ]);
    ctx.reply_ui_ephemeral(&prompt).await?;
    let source = ctx.original_response().await?;
    confirmation.bind(ctx.guild_id(), &source).map_err(Error::handler)?;
    let text = if confirmation.wait().await == ConfirmationOutcome::Confirmed {
        "確定を受け付けました。"
    } else {
        "確認を終了しました。"
    };
    ctx.edit_original_ui(&UiMessage::new().content(text)).await
}
```

確認ボタンはコマンドと別の処理枠で応答します。セッションを有効にすると `akari-confirm:` から始まる ID は確認用に予約されます。セッションはメモリ内にあり、Bot の停止時に終了します。別の処理から終了する場合は `stop_handle().stop()` を使います。

## 受け取れる情報

| メソッド | 返す値 |
| --- | --- |
| `ctx.custom_id()` | 押されたボタンの ID |
| `ctx.author_id()` | 押したユーザーの ID |
| `ctx.invoker_id()` | 元のメッセージのきっかけになったユーザーの ID |
| `ctx.guild_id()` | 押されたサーバーの ID（DM では `None`） |
| `ctx.message()` | ボタンが付いている元のメッセージ |
| `ctx.timing()` | 受信から処理までの時間の測定値 |
