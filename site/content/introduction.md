# Disnana Akari とは

**Disnana Akari**（以下 Akari）は、Rust で Discord Bot を書くためのフレームワークです。[Twilight](https://twilight.rs/) の上に作られていて、コマンドを `async fn` として登録するだけで動かせます。

```rust
use disnana_akari::model::gateway::Intents;
use disnana_akari::{Bot, CommandContext, Commands, Result};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut commands = Commands::new("!")?;
    commands.register("ping", ping)?;
    Bot::from_env()?
        // テキストコマンドは本文を読むので、この Intent が必要です
        .add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)
        .run_commands(commands)
        .await
}

async fn ping(ctx: CommandContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}
```

これだけで、Discord で `!ping` と送ると Bot が `Pong!` と返信します。`Intents` の行は、Akari が勝手に足さないので、自分で書きます（[Intent の設定](guide/intents.md)）。

## できること

| 機能 | 説明 |
| --- | --- |
| [テキストコマンド](guide/text-commands.md) | `!ping` のように、プレフィックス付きのメッセージで呼び出します。 |
| [スラッシュコマンド](guide/slash-commands.md) | `/ping` のように、Discord のメニューから呼び出します。サブコマンド、入力中の候補（オートコンプリート）、型付きの引数、すべてのサーバーへの登録（グローバル）にも対応しています。 |
| [ハイブリッドコマンド](guide/hybrid-commands.md) | 1 つの関数を `!echo` と `/echo` の両方に登録します。 |
| [イベント](guide/events.md) | 接続完了やメッセージの受信に反応します。 |
| [Embed とボタン](guide/buttons.md) | 見栄えのよい返信を作り、ボタンのクリックを受け取ります。 |
| [選択メニュー](guide/select-menus.md) | 文字列、ユーザー、ロール、チャンネルを選ばせます。 |
| [入力フォーム](guide/modals.md) | モーダルで文章を入力してもらいます。 |
| [Components V2](guide/components-v2.md) | テキスト、区切り線、コンテナーを組み合わせたレイアウトを作ります。 |
| [クールダウン](guide/cooldowns.md) | ユーザーごとに連打を制限します。 |
| [権限の確認](guide/permissions.md) | 「BAN の権限を持つ人だけ」のように、実行できる人を絞ります。 |
| [モジュール](guide/modules.md) | 関連するコマンドやイベントを、ひとまとまりにして登録します（discord.py の Cog にあたります）。 |
| [ステータスとアクティビティ](guide/presence.md) | Bot のオンライン状態と「〇〇をプレイ中」を設定します。 |
| [ログ](guide/logging.md) | `tracing` で、エラーや接続の様子をレベルつきで記録します。 |

## 特徴

- **勝手なことをしません。** 標準出力にも標準エラー出力にも書かず、Intent も自動では追加しません。ハンドラーのエラーは、`Bot::on_error` で一件ずつ受け取れます。[エラーと自動で送る文言](guide/errors.md)
- **遅いコマンドがあっても止まりません。** コマンドは並行して実行され、混雑したときは `on_error` に報告します（スラッシュコマンドとボタンには、本人にだけ「現在混雑しています」と返します）。[仕組み](guide/how-it-works.md)
- **Discord の 3 秒ルールに対応しています。** 時間のかかる処理は `defer` で先に受け付け、あとで結果を返せます。[応答のしかた](guide/responding.md)
- **入力の検証が付いています。** Embed の文字数やボタン ID の重複など、Discord が拒否する内容は送信前にエラーになります。
- **Twilight の型をそのまま使えます。** 足りない機能は Twilight の API で補えます。[Twilight を直接使う](guide/twilight.md)

## 動作環境

- Rust 1.89 以上
- [Tokio](https://tokio.rs/) ランタイム
- Discord の Bot トークン

> [!NOTE]
> 現在のバージョンは 0.2 系です。公開 API は今後の更新で変わることがあります。変更点は [変更履歴](reference/changelog.md) に記載します。

## 現時点の制限

Akari は、まず「自分のテスト用サーバーで Bot を動かす」ことに重点を置いています。

- シャードは 1 つです。大規模な Bot 向けの分割には対応していません。
- 同じ Bot に、グローバルとサーバー限定のスラッシュコマンドを同時には登録できません。
- テキストコマンドとメッセージの受信は、サーバー内のメッセージが対象です（ダイレクトメッセージは対象外です）。

## 読み進め方

1. はじめての人は、[インストール](getting-started/install.md) から [はじめての Bot](getting-started/first-bot.md) まで順に進めてください。
2. Rust が初めてなら [Rust が初めての人へ](getting-started/rust-basics.md)、discord.py の経験があるなら [対応表](getting-started/from-discordpy.md) が近道です。
3. 作りたい機能は [ガイド](guide/text-commands.md) から選べます。
4. 型やメソッドの一覧は [API リファレンス](reference/api.md) を見てください。
