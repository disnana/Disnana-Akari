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

## 何も応答しなかったら

Akari が、種類ごとに補ってくれます。

| 種類 | ハンドラーが成功して応答なし | ハンドラーがエラー |
| --- | --- | --- |
| スラッシュコマンド | 「コマンドを処理できませんでした。」（本人にだけ） | 同左。`defer` 済みなら「考え中」を置き換える |
| ボタン・選択メニュー | 何も変えずに受け付ける（クリックが失敗扱いにならない） | 「…を処理できませんでした」を本人にだけ返す |
| フォーム | 「入力を受け付けました。」 | 「入力内容を処理できませんでした。」 |

> [!TIP]
> エラーの原因を利用者に説明したいときは、`Err` を返さずに、自分で `reply_ephemeral` で理由を伝えて `Ok(())` で終わります。
