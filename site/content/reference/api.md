# API リファレンス

すべての型とメソッドは、rustdoc で調べられます。

<a href="/Disnana-Akari/api/disnana_akari/index.html" target="_blank" rel="noopener"><strong>rustdoc を開く →</strong></a>

rustdoc には、次の内容があります。

- 公開されているすべての型、メソッド、関数のシグネチャ
- 型の検索（画面上部の検索ボックス）
- ソースコードへのリンク

## 主な型

| 型 | 役割 | ガイド |
| --- | --- | --- |
| `Bot` | Bot の設定と起動 | [はじめての Bot](../getting-started/first-bot.md) |
| `Commands` / `CommandContext` | テキストコマンド | [テキストコマンド](../guide/text-commands.md) |
| `SlashCommands` / `SlashContext` | スラッシュコマンド | [スラッシュコマンド](../guide/slash-commands.md) |
| `register_hybrid` / `HybridContext` | テキストとスラッシュの共通化 | [ハイブリッドコマンド](../guide/hybrid-commands.md) |
| `Events` / `Ready` | イベントのハンドラー登録 | [イベントを受け取る](../guide/events.md) |
| `ButtonContext` / `SelectContext` / `ModalContext` | ボタン、選択メニュー、フォームの操作 | [Embed とボタン](../guide/buttons.md) |
| `ui::{UiMessage, Embed, Button, ...}` | メッセージの部品 | [Embed とボタン](../guide/buttons.md) |
| `ui::ComponentsV2` | Components V2 のメッセージ | [Components V2](../guide/components-v2.md) |
| `Cooldown` | ユーザーごとの実行間隔 | [クールダウン](../guide/cooldowns.md) |
| `Error` / `ErrorReport` | 型付きエラーと発生箇所の通知 | [エラーと自動で送る文言](../guide/errors.md) |
| `Timing` | 受信から処理までの測定値 | [Akari の動き方](../guide/how-it-works.md) |

## 測定値（`Timing`）

| メソッド | 測定範囲 |
| --- | --- |
| `gateway_rtt()` | 直近のハートビートを送ってから、Discord の応答を受け取るまでの往復時間。応答を受け取る前は `None`。 |
| `queue_wait()` | Akari がイベントを受信してから、処理が始まるまでの待ち時間。 |
| `handler_elapsed()` | 処理を始めてから、呼び出した時点までの経過時間。 |

## 型のメモ

- `Result<T>` は `std::result::Result<T, disnana_akari::Error>` の別名です。
- `disnana_akari::model` は `twilight_model` の再エクスポート、`disnana_akari::http` は `twilight_http` の再エクスポートです。

> [!NOTE]
> このリファレンスは、公開時に rustdoc から自動で生成されます。ローカルで見るには、リポジトリのルートで `cargo doc --no-deps --open` を実行してください。
