# Components V2

従来の「本文＋Embed＋ボタン」とは別の、新しいメッセージの組み立て方です。テキスト、区切り線、ボタン、選択メニュー、色付きのコンテナーを、好きな順に並べられます。

## 基本

```rust
use disnana_akari::ui::{Button, ComponentsV2};
use disnana_akari::{Result, SlashContext};

async fn panel(ctx: &mut SlashContext<'_>) -> Result<()> {
    let message = ComponentsV2::new()
        .text("## お知らせ\nメンテナンスは **今夜 0 時** からです。")
        .separator()
        .buttons(vec![Button::primary("akari:demo:ack", "了解")]);
    ctx.reply_v2(&message).await
}
```

| メソッド | 動作 |
| --- | --- |
| `.text(markdown)` | Markdown を使えるテキストを追加します。 |
| `.separator()` | 横線の区切りを追加します。 |
| `.buttons(vec![...])` | ボタンの行を追加します。 |
| `.select(menu)` | 文字列の選択メニューを追加します。 |
| `.user_select(..)` / `.role_select(..)` / `.channel_select(..)` / `.mentionable_select(..)` | [選択メニュー](select-menus.md)の各種類を追加します。 |
| `.container(color, 中身)` | 複数の部品を、色付きの枠でまとめます。 |
| `.component(raw)` | Twilight の部品を直接追加します。 |

## コンテナーでまとめる

```rust
use disnana_akari::ui::{Button, ComponentsV2};

fn card() -> ComponentsV2 {
    let inside = ComponentsV2::new()
        .text("### 今週のイベント")
        .text("土曜 21:00 に開催します。")
        .separator()
        .buttons(vec![Button::success("akari:demo:join", "参加する")]);

    ComponentsV2::new().container(0x5865F2, inside)
}
```

## 送り方

| 場所 | メソッド |
| --- | --- |
| テキストコマンド | `ctx.reply_v2(&message)` |
| スラッシュコマンド | `ctx.reply_v2(&message)` / `ctx.reply_v2_ephemeral(&message)` |
| ハイブリッドコマンド | `ctx.reply_v2(&message)` |
| ボタン・選択メニュー（元のメッセージを更新） | `ctx.update_v2(&message)` |

## 制限

- **本文と Embed は併用できません。** 文章は `.text()` で入れます。従来の本文や Embed が必要なら、[`UiMessage`](buttons.md) を使います。
- テキストは、メッセージ全体で 4,000 文字までです。
- 部品は、入れ子を含めて 40 個までです。
- `custom_id` は、メッセージ全体（コンテナーの中を含む）で重複できません。
- 既に Components V2 になっているメッセージは、`update_v2` で更新します。

送信前に、これらの制限が検証されます。違反しているときは、Discord へ送らず `Err` を返します。
