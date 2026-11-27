# 応答のしかた

スラッシュコマンド、ボタン、選択メニュー、フォームのような **Interaction** には、Discord が決めたルールがあります。この章を読むと、「どのメソッドをいつ呼ぶか」が分かります。

## 3 つのルール

1. **最初の応答は 3 秒以内。** 間に合わないと、Discord に「アプリケーションが応答しませんでした」と表示されます。
2. **最初の応答は 1 回だけ。** 2 回目以降は、`followup`（追加のメッセージ）を使います。
3. **応答の期限は 15 分。** 最初の応答から 15 分たつと、続きのメッセージは送れません。

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

**公開範囲は最初の応答で決まります。** 公開の `defer` のあとに `reply_ephemeral` を呼ぶと、エラーになります。本人にだけ見せたいときは、最初から `defer_ephemeral` を使ってください。

## 使えるメソッドの一覧

| やりたいこと | スラッシュ | ボタン・選択 | フォーム |
| --- | --- | --- | --- |
| すぐ返信 | `reply` | `reply_ephemeral` | `reply` / `reply_ephemeral` |
| 本人にだけ返信 | `reply_ephemeral` | `reply_ephemeral` | `reply_ephemeral` |
| 処理中にする | `defer` / `defer_ephemeral` | `defer_update` | `defer` / `defer_ephemeral` |
| 元のメッセージを更新 | – | `update_message` / `update_v2` | – |
| 追加のメッセージ | `followup` / `followup_ephemeral` | `followup` / `followup_ephemeral` | `followup` / `followup_ephemeral` |
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
