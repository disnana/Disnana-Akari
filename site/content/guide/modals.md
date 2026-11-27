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

## ファイルを添付してもらう

モーダルの入力欄としてファイルのアップロードを受け付けられます。次の例では添付を任意にし、最大 3 件に設定しています。

```rust
use disnana_akari::{ModalContext, Result, SlashContext, ui::{FileUpload, Modal}};

async fn open(ctx: &mut SlashContext<'_>) -> Result<()> {
    let files = FileUpload::new("evidence", "参考ファイル")
        .description("スクリーンショットなど")
        .min_values(0)
        .max_values(3)
        .required(false);
    ctx.show_modal(&Modal::new("report", "問い合わせ").file_upload(files)).await
}

async fn submit(ctx: &mut ModalContext<'_>) -> Result<()> {
    let ids = ctx.file_upload("evidence").unwrap_or_default();
    let names: Vec<_> = ids.iter()
        .filter_map(|&id| ctx.attachment(id).map(|file| file.filename.as_str()))
        .collect();
    let response = format!("添付 {} 件を確認しました。", names.len());
    ctx.reply_ephemeral(&response).await
}
```

`file_upload` はフォームにその欄がない場合は `None`、任意の欄に添付しなかった場合は空のスライスを返します。`attachment` は Discord の通知に対応するメタデータがあるときに、ファイル名、サイズ、URL などを返します。添付を必須にする場合は最小件数を 1 以上にします。[Discord の File Upload 仕様](https://docs.discord.com/developers/components/reference#file-upload)

## チェックボックスを追加する

```rust
use disnana_akari::{ModalContext, Result, ui::{Checkbox, Modal}};

fn form() -> Modal {
    Modal::new("preferences", "通知の設定").checkbox(
        Checkbox::new("subscribe", "通知を受け取る")
            .description("新着の通知を受け取ります。")
            .selected_by_default(false),
    )
}

async fn submit(ctx: &mut ModalContext<'_>) -> Result<()> {
    let message = match ctx.checkbox("subscribe") {
        Some(true) => "通知を有効にします。",
        Some(false) => "通知を無効にします。",
        None => "通知の設定を取得できませんでした。",
    };
    ctx.reply_ephemeral(message).await
}
```

単一のチェックボックスは Discord の仕様で選択を必須にできません。欄が送信内容にある場合は、未選択でも `Some(false)` を返します。[Discord の Checkbox 仕様](https://docs.discord.com/developers/components/reference#checkbox)

## 選択メニューを追加する

```rust
use disnana_akari::{ModalContext, Result, ui::{Modal, SelectOption, StringSelect}};

fn form() -> Modal {
    let color = StringSelect::new("color", vec![
        SelectOption::new("青", "blue"),
        SelectOption::new("緑", "green"),
    ]).min_values(0);
    Modal::new("profile", "プロフィール").select("好きな色", color, false)
}

async fn submit(ctx: &mut ModalContext<'_>) -> Result<()> {
    let Some(colors) = ctx.string_select("color") else {
        return ctx.reply_ephemeral("選択欄を確認できませんでした。").await;
    };
    ctx.reply_ephemeral(&format!("選択数は {} 件です。", colors.len())).await
}
```

ユーザー・ロール・チャンネルは対応する型の ID を返します。ユーザーとロールを混ぜる場合は `ctx.mentionable_select(id)?` で両者を区別します。任意の欄では最小選択数に 0 を指定できます。フォームではメッセージ用の `disabled` 設定は使えません。

## 複数のチェックボックスを追加する

```rust
use disnana_akari::{ModalContext, Result, ui::{CheckboxGroup, CheckboxGroupOption, Modal}};

fn form() -> Modal {
    let options = vec![
        CheckboxGroupOption::new("お知らせ", "news").selected_by_default(true),
        CheckboxGroupOption::new("メンテナンス", "maintenance"),
    ];
    Modal::new("notifications", "通知設定").checkbox_group(
        CheckboxGroup::new("topics", "受け取る通知", options)
            .min_values(0)
            .max_values(2)
            .required(false),
    )
}

async fn submit(ctx: &mut ModalContext<'_>) -> Result<()> {
    let Some(topics) = ctx.checkbox_group("topics") else {
        return ctx.reply_ephemeral("選択欄がありません。").await;
    };
    ctx.reply_ephemeral(&format!("選択数は {} 件です。", topics.len())).await
}
```

任意の欄では選択数が 0 件のことがあります。その場合は空のスライスで、フォームに欄がない場合は `None` です。[Discord の Checkbox Group 仕様](https://docs.discord.com/developers/components/reference#checkbox-group)

## 一つだけ選べるラジオグループを追加する

```rust
use disnana_akari::{ModalContext, Result, ui::{Modal, RadioGroup, RadioGroupOption}};

fn form() -> Modal {
    Modal::new("format", "出力形式").radio_group(
        RadioGroup::new("type", "形式", vec![
            RadioGroupOption::new("テキスト", "text"),
            RadioGroupOption::new("CSV", "csv"),
        ]).required(false),
    )
}

async fn submit(ctx: &mut ModalContext<'_>) -> Result<()> {
    let reply = match ctx.radio_group("type") {
        Some(Some(format)) => format!("形式を選びました: {format}"),
        Some(None) => "形式の指定を省略しました。".into(),
        None => "形式の入力欄がありません。".into(),
    };
    ctx.reply_ephemeral(&reply).await
}
```

選択肢は 2〜10 個で、初期選択は 1 個までです。任意の欄で選ばなかったときは `Some(None)`、その欄がないときは `None` になります。[Discord の Radio Group 仕様](https://docs.discord.com/developers/components/reference#radio-group)

## 送信後の応答

フォームの送信にも、3 秒以内の応答が必要です。使えるメソッドは次のとおりです。

| メソッド | 動作 |
| --- | --- |
| `ctx.reply(text)` / `ctx.reply_ephemeral(text)` | 返信します。 |
| `ctx.defer()` / `ctx.defer_ephemeral()` | 「処理中」を出して、あとで `reply` で置き換えます。 |
| `ctx.followup(text)` / `ctx.followup_ephemeral(text)` | 追加のメッセージを送ります。 |
| `ctx.followup_files(text, &files, limits)` / `ctx.followup_files_ephemeral(text, &files, limits)` | 添付付きの追加メッセージを送ります。遅延応答後は先に `reply` などで待機表示を完成させます。[添付付きフォローアップ](responding.md#追加のメッセージにファイルを添付する) |

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

`original_response` でフォーム送信への元の応答を取得し、`edit_original_response`、`edit_original_ui`、`edit_original_v2` で編集できます。`delete_original_response` はその応答を削除します。これらを呼ぶ前に、フォーム送信へ返信するか defer してください。

```rust
use disnana_akari::{ModalContext, Result};

async fn submit(ctx: &mut ModalContext<'_>) -> Result<()> {
    ctx.defer_ephemeral().await?;
    ctx.edit_original_response("入力内容を保存しました。").await
}
```

ボタンからフォームを開いた場合も、編集先はフォーム送信への応答です。

フォームの識別子に `DynamicId` を使い、機能名を `on_dynamic_modal` に登録できます。送信時に `ctx.dynamic_id()` で対象の識別子を受け取ります。[動的 ID の使い方](buttons.md#再起動後も同じボタンを使う)

## 受け取れる情報

`ctx.custom_id()`（フォームの ID）、`ctx.author_id()`（送信した人）、`ctx.guild_id()`（サーバー）、`ctx.timing()`、`ctx.http()` が使えます。
