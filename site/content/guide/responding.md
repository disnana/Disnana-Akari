# 応答のしかた

スラッシュコマンド、ボタン、選択メニュー、フォームのような **Interaction** には、Discord が決めたルールがあります。この章を読むと、「どのメソッドをいつ呼ぶか」が分かります。

## 3 つのルール

1. **最初の応答は 3 秒以内。** 間に合わないと、Discord に「アプリケーションが応答しませんでした」と表示されます。
2. **最初の応答は 1 回だけ。** 2 回目以降は、`followup`（追加のメッセージ）を使います。
3. **応答に使うトークンの有効期間は 15 分。** 続きのメッセージは有効期間内に送ります。

## 流れ

```text
操作を受信 ──▶ 3 秒以内に最初の応答 ──▶ （15 分以内なら）追加のメッセージ
                │
                ├─ reply / reply_ephemeral   すぐに返信する
                ├─ defer / defer_ephemeral   「考え中…」を出して、あとで reply
                ├─ update_message            ボタンのメッセージを書き換える（ボタン・メニュー）
                └─ show_modal                入力フォームを開く
```

## すぐ返す

```rust
use disnana_akari::{Result, SlashContext};

async fn ping(ctx: &mut SlashContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}
```

## 時間がかかるときは `defer`

```rust
use disnana_akari::{Result, SlashContext};

async fn report(ctx: &mut SlashContext<'_>) -> Result<()> {
    ctx.defer().await?;                         // 「考え中…」を表示
    // ここで時間のかかる処理を待つ
    ctx.reply("集計結果が出ました。").await?;    // 結果に置き換える
    ctx.followup("条件を変えると再集計できます。").await
}
```

> [!IMPORTANT]
> `defer` は **処理を始める前** に呼びます。`defer` の `?` で失敗を確認してから重い処理を始めると、初回応答に失敗したときに無駄な処理をせずに済みます。

## 本人にだけ見せる（ephemeral）

`_ephemeral` が付くメソッドは、操作した本人にだけ見えるメッセージを送ります。エラーの説明や、個人向けの結果に向いています。

**元の応答の公開範囲は、最初の応答で決まります。** 公開の `defer` のあとに `reply_ephemeral` を呼ぶと、エラーになります。元の応答を本人にだけ見せたいときは、最初から `defer_ephemeral` を使ってください。追加メッセージの公開範囲は、呼び出す `followup` 系メソッドで個別に選べます。

## 使えるメソッドの一覧

| やりたいこと | スラッシュ | ボタン・選択 | フォーム |
| --- | --- | --- | --- |
| すぐ返信 | `reply` | `reply_ephemeral` | `reply` / `reply_ephemeral` |
| 本人にだけ返信 | `reply_ephemeral` | `reply_ephemeral` | `reply_ephemeral` |
| 処理中にする | `defer` / `defer_ephemeral` | `defer_update` | `defer` / `defer_ephemeral` |
| 元のメッセージを更新 | – | `update_message` / `update_v2` | – |
| 追加のメッセージ | `followup` / `followup_ephemeral` | `followup` / `followup_ephemeral` | `followup` / `followup_ephemeral` |
| ファイル付きの追加メッセージ | `followup_files` / `followup_files_ephemeral` | `followup_files` / `followup_files_ephemeral` | `followup_files` / `followup_files_ephemeral` |
| フォームを開く | `show_modal` | `show_modal` | – |

## 元の応答を取得・更新・削除する

スラッシュコマンドやコンテキストメニューの `SlashContext` では、`original_response().await` で初回の応答メッセージを取得できます。非公開で送った応答も対象です。`edit_original_response` は本文だけ、`edit_original_ui` は本文・Embed・従来の部品、`edit_original_v2` は Components V2 の部品を更新します。公開範囲は元のままです。

```rust
use disnana_akari::{Result, SlashContext};

async fn report(ctx: &mut SlashContext<'_>) -> Result<()> {
    ctx.reply_ephemeral("集計しています。").await?;
    ctx.edit_original_response("集計が終わりました。").await?;
    let original = ctx.original_response().await?;
    ctx.followup_ephemeral(&format!("応答 ID: {}", original.id)).await
}
```

応答を取り除く場合は `delete_original_response().await` を使います。フォームを開く操作は元の応答メッセージを作りません。応答がない状態や削除済みの状態では、取得・編集・削除がエラーになります。遅延応答だけを削除した場合は追加応答もできません。追加応答を送る場合は、先に遅延応答を完成させてください。[Discord の応答メッセージ API](https://docs.discord.com/developers/interactions/receiving-and-responding#get-original-interaction-response)

## 送る前に制限を確認する

Akari の返信メソッドは、通常の本文が2,000文字以内か、HTTP リクエストを送る前に確認します。自分で確認したいときは `validation::message_content` を使います。`UiMessage::validate`、`ComponentsV2::validate`、`Modal::validate` は、本文や部品の配置、個数などをまとめて確認できます。

添付付きの V2 応答は、保持する分と新しいアップロードの合計が10件を超える場合に失敗します。Discord が Interaction に通知した1ファイルのサイズ上限も、ファイル本体を読み直さずに確認します。通常のチャンネル投稿でサイズ上限が分からない場合は、Discord の送信結果を確認します。[Discord のメッセージ仕様](https://docs.discord.com/developers/resources/message#create-message) · [ファイルの送信仕様](https://docs.discord.com/developers/reference#uploading-files)

## 何も応答しなかったら

Akari が、種類ごとに補います。文言は [`Messages`](errors.md) で変えたり、送らないようにしたりできます。どの場合も、エラーは `Bot::on_error` に報告されます。

| 種類 | ハンドラーが成功して応答なし | ハンドラーがエラー |
| --- | --- | --- |
| スラッシュコマンド | 「コマンドを処理できませんでした。」（本人にだけ） | 同左。`defer` 済みなら「考え中」を置き換える |
| ボタン・選択メニュー | 何も変えずに受け付ける（クリックが失敗扱いにならない） | 「…を処理できませんでした」を本人にだけ返す |
| フォーム | 「入力を受け付けました。」 | 「入力内容を処理できませんでした。」 |
| テキストコマンド | 何もしない | 既定では何も送らない（`Messages::text_command_failed` で返信できる） |

> [!TIP]
> エラーの原因を利用者に説明したいときは、`Err` を返さずに、自分で `reply_ephemeral` で理由を伝えて `Ok(())` で終わります。

## 通常の返信にファイルを添付する

`reply_files` はテキスト、スラッシュ、ハイブリッドで使えます。添付だけを送りたいときは本文に空文字を渡します。スラッシュの `reply_files_ephemeral` は実行者のみに表示します。先に defer した場合は、その応答の表示範囲に合わせます。

```rust
use disnana_akari::{
    CommandContext, Result, model::http::attachment::Attachment, validation::AttachmentLimits,
};

async fn report(ctx: CommandContext<'_>) -> Result<()> {
    let files = [Attachment::from_bytes(
        "report.txt".into(),
        b"Report ready".to_vec(),
        0,
    )];
    let limits = AttachmentLimits::default().max_file_bytes(Some(1024 * 1024));
    ctx.reply_files("", &files, limits).await
}
```

`AttachmentLimits` は Components V2 と共有できます。通常の返信も、本文の長さ、添付数、名前と ID、ファイルサイズを送信前に検証します。チャンネルへの添付返信では multipart の情報も含めて 25 MiB のリクエスト上限を確認します。アプリ独自の上限を使わない場合は `AttachmentLimits::default()` を渡します。[Discord のメッセージ作成仕様](https://docs.discord.com/developers/resources/message#create-message)

## 追加のメッセージにファイルを添付する

公開の追加メッセージには `followup_files`、操作した本人にだけ送るときは `followup_files_ephemeral` を使います。ファイルだけを送るときは本文に空文字を指定できます。通常の添付返信と同じ `AttachmentLimits` を渡します。[Discord の追加メッセージ API](https://docs.discord.com/developers/interactions/receiving-and-responding#create-followup-message)

スラッシュコマンドとフォーム送信では、`defer` や `defer_ephemeral` のあとに `reply` などで待機表示を完成させてから送ります。下の例では、集計の完了を知らせたあと、結果ファイルを本人にだけ追加で送ります。

```rust
use disnana_akari::{
    Result, SlashContext, model::http::attachment::Attachment, validation::AttachmentLimits,
};

async fn export_report(ctx: &mut SlashContext<'_>) -> Result<()> {
    ctx.defer_ephemeral().await?;
    let files = [Attachment::from_bytes(
        "summary.csv".into(),
        b"item,count\nprocessed,12\n".to_vec(),
        0,
    )];
    ctx.reply("集計が完了しました。").await?;
    ctx.followup_files_ephemeral("", &files, AttachmentLimits::default())
        .await
}
```

ボタンと選択メニューでは `defer_update` で操作を受け付けた時点から、同じメソッドでファイル付きの追加メッセージを送れます。ボタンやメニューが付いた元の投稿はそのままです。

## 追加返信をあとから編集・削除する

初回応答を済ませ、`ctx.followups()?` から追加返信を送ると、作成したメッセージを `FollowupMessage` として受け取れます。ID と Discord が返したメッセージを `id()` と `message()` で参照し、`edit` や `delete` でその追加返信を操作できます。

```rust
use disnana_akari::{Result, SlashContext};

async fn report(ctx: &mut SlashContext<'_>) -> Result<()> {
    ctx.reply_ephemeral("結果を取りまとめています。").await?;
    let mut followup = ctx.followups()?.send("暫定結果です。", true).await?;
    followup.edit("結果を更新しました。").await
}
```

本文、従来の UI、Components V2、ファイル付き本文は、それぞれ `send`、`send_ui`、`send_v2`、`send_files` で送れます。従来の UI を置き換えるときは `edit_ui`、Components V2 の追加返信を更新するときは `edit_v2` を使います。購入ボタンは `send_ui` と `send_v2` の両方で送れます。公開範囲は送信時に指定します。ハンドルは元の Interaction を借りるため、ハンドラー内で使います。 [Discord の追加返信 API](https://docs.discord.com/developers/interactions/receiving-and-responding#create-followup-message)

追加返信のハンドルの `Debug` と Akari が返す通信エラーには、Interaction トークンを含めません。Twilight 0.18 の HTTP トレーシングは、トークンを含むリクエストパスを記録する場合があります。ログ出力は [ログの設定](logging.md) の例のように Akari に絞ってください。

## 初回応答の失敗を調べる

`Bot::on_info` の `InfoEvent::InteractionResponseFailed` は、初回応答の HTTP リクエストが失敗したときに届きます。JSON と添付付きのどちらでも、Interaction ID、HTTP ステータス、Discord の数値エラーコード、キュー待ち、REST 待ちを確認できます。トークンとレスポンス本文は含みません。

たとえば、Discord のコード `10062` は Interaction を見つけられなかったことを示します。Bot が受信してからの経過時間を合わせて見れば、Discord が求める3秒の期限と照らし合わせられます。[Discord の応答期限](https://docs.discord.com/developers/interactions/receiving-and-responding#interaction-callback) · [Discord のエラー番号](https://docs.discord.com/developers/topics/opcodes-and-status-codes#json)
