# 入力フォーム（モーダル）

画面の中央に開く、文章を入力してもらうためのフォームです。「問い合わせの内容を入力」「名前を変更」のような用途に向いています。

## 流れ

1. スラッシュコマンド、ボタン、選択メニューの **最初の応答** として `show_modal` でフォームを開きます。
2. 利用者が入力して送信すると、`events.on_modal` に登録した関数が呼ばれます。
3. 関数の中で、`text_input` で入力された値を読み取り、返信します。

## 例

```rust
use disnana_akari::ui::{Modal, TextInput};
use disnana_akari::{Events, ModalContext, Result, SlashContext};

const FEEDBACK: &str = "akari:demo:feedback";
const BODY: &str = "body";

// 1. フォームを開く
async fn feedback(ctx: &mut SlashContext<'_>) -> Result<()> {
    let form = Modal::new(FEEDBACK, "ご意見をお聞かせください").text_input(
        TextInput::new(BODY, "ご意見")
            .placeholder("気になったことを書いてください")
            .paragraph()
            .min_length(5)
            .max_length(500),
    );
    ctx.show_modal(&form).await
}

fn register(events: &mut Events) -> Result<()> {
    events.on_modal(FEEDBACK, submitted)
}

// 2. 送信されたら呼ばれる
async fn submitted(ctx: &mut ModalContext<'_>) -> Result<()> {
    let Some(text) = ctx.text_input(BODY) else {
        return ctx.reply_ephemeral("入力を読み取れませんでした。").await;
    };
    // 入力内容そのものはログに出さない（個人情報が含まれるかもしれません）
    ctx.reply_ephemeral(&format!("{} 文字のご意見を受け取りました。", text.chars().count())).await
}
```

## 入力欄の設定

| メソッド | 動作 |
| --- | --- |
| `TextInput::new(custom_id, label)` | 入力欄を作ります。`label` が見出しとして表示されます。 |
| `.placeholder(text)` | 未入力のときに表示する案内 |
| `.paragraph()` | 複数行の入力欄にします（既定は 1 行）。 |
| `.required(false)` | 必須にしません（既定は必須）。 |
| `.min_length(n)` / `.max_length(n)` | 入力できる文字数（最大 4,000） |
| `.value(text)` | 最初から入っている文字 |
| `Modal::new(id, title).text_input(...)` | フォームに入力欄を追加します（最大 5 個）。タイトルは 45 文字まで。 |

> [!NOTE]
> フォームの `custom_id` と、入力欄の `custom_id` は別物です。`on_modal` に渡すのはフォームの ID、`ctx.text_input(...)` に渡すのは入力欄の ID です。

## 送信後の応答

フォームの送信にも、3 秒以内の応答が必要です。使えるメソッドは次のとおりです。

| メソッド | 動作 |
| --- | --- |
| `ctx.reply(text)` / `ctx.reply_ephemeral(text)` | 返信します。 |
| `ctx.defer()` / `ctx.defer_ephemeral()` | 「処理中」を出して、あとで `reply` で置き換えます。 |
| `ctx.followup(text)` / `ctx.followup_ephemeral(text)` | 追加のメッセージを送ります。 |

```rust
use disnana_akari::{ModalContext, Result};

async fn save(ctx: &mut ModalContext<'_>) -> Result<()> {
    ctx.defer_ephemeral().await?;                 // 先に受け付ける
    // ここで保存などの時間のかかる処理を待つ
    ctx.reply("保存しました。").await?;            // 処理中の表示を置き換える
    ctx.followup_ephemeral("受け付け番号は 1234 です。").await
}
```

ハンドラーが何も返信せずに成功した場合は、Akari が「入力を受け付けました。」と知らせます。この文言は、`Messages::modal_accepted` で変えたり、`None` にして送らないようにしたりできます（[エラーと自動で送る文言](errors.md)）。

## 受け取れる情報

`ctx.custom_id()`（フォームの ID）、`ctx.author_id()`（送信した人）、`ctx.guild_id()`（サーバー）、`ctx.timing()`、`ctx.http()` が使えます。
