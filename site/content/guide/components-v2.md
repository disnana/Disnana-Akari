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

## テキストの横にボタンや画像を置く

```rust
use disnana_akari::ui::{Button, ComponentsV2, Media};

fn help_panel() -> ComponentsV2 {
    ComponentsV2::new()
        .section_with_button(["# サポート", "操作方法を開きます。"], Button::primary("help", "開く"))
        .section_with_thumbnail(
            ["# プレビュー"],
            Media::new("https://example.com/preview.webp").description("プレビュー画像"),
        )
        .media_gallery([Media::new("https://example.com/gallery.webp").spoiler(true)])
}
```

セクションにはテキストを 1～3 個、ギャラリーにはメディアを 1～10 件置けます。`Media::description` は代替テキストです。`attachment://ファイル名` を参照する場合は、参照先のファイルを送信時にアップロードします。

## 罫線とコンテナーの見た目を調整する

`separator_with_spacing(SeparatorSpacingSize::Large, false)` は広い余白だけを挿入します。罫線も表示する場合は第2引数を `true` にします。`container_with_options(None, true, contents)` は、強調色を指定せず、クリックすると中身を表示するコンテナーを作ります。

## 添付ファイルを部品として表示する

`FileDisplay` の名前と、同じメッセージに添付するファイル名を一致させます。ファイル名は拡張子まで指定します。

```rust
use disnana_akari::{Result, SlashContext, model::http::attachment::Attachment};
use disnana_akari::ui::{ComponentsV2, FileDisplay};

async fn send_report(ctx: &mut SlashContext<'_>) -> Result<()> {
    let name = "report.txt";
    let panel = ComponentsV2::new()
        .text("出力ファイル")
        .file(FileDisplay::new(name))
        .attachment(Attachment::from_bytes(name.into(), b"total: 42\n".to_vec(), 0));
    ctx.reply_v2(&panel).await
}
```

複数のファイルをアップロードする場合は、ファイルごとに異なる添付 ID とファイル名を指定します。`FileDisplay` は添付を参照する部品です。外部サイトの URL は渡せません。[Discord の File 仕様](https://docs.discord.com/developers/components/reference#file)

## 送り方

| 場所 | メソッド |
| --- | --- |
| テキストコマンド | `ctx.reply_v2(&message)` |
| スラッシュコマンド | `ctx.reply_v2(&message)` / `ctx.reply_v2_ephemeral(&message)` |
| ハイブリッドコマンド | `ctx.reply_v2(&message)` |
| ボタン・選択メニュー（元のメッセージを更新） | `ctx.update_v2(&message)` |

## 制限

`ComponentsV2::validate()` を呼ぶと、送信前に部品配置、本文、アップロードするファイル数などを確認できます。アップロードは1メッセージに最大10件で、更新で添付を追加するときは残すファイルも含めて数えます。Interaction への返信では、その呼び出しで Discord が通知した上限と、各ファイルのサイズも照合します。[Discord のメッセージ添付の制限](https://docs.discord.com/developers/topics/opcodes-and-status-codes#json-json-error-codes)

- **本文と Embed は併用できません。** 文章は `.text()` で入れます。従来の本文や Embed が必要なら、[`UiMessage`](buttons.md) を使います。
- テキストは、メッセージ全体で 4,000 文字までです。
- 部品は、入れ子を含めて 40 個までです。
- `custom_id` は、メッセージ全体（コンテナーの中を含む）で重複できません。
- 既に Components V2 になっているメッセージは、`update_v2` で更新します。

送信前に、これらの制限が検証されます。違反しているときは、Discord へ送らず `Err` を返します。
