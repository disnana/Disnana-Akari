---
layout: home
title: Disnana Akari

hero:
  name: Disnana Akari
  text: Rust 製の Discord Bot フレームワーク
  tagline: Twilight の上に作られています。テキストコマンド、スラッシュコマンド、ボタンやフォームを、同じ書き方の async fn で扱えます。
  actions:
    - theme: brand
      text: はじめる
      link: /getting-started/install
    - theme: alt
      text: Akari とは
      link: /introduction
    - theme: alt
      text: GitHub
      link: https://github.com/disnana/Disnana-Akari

features:
  - title: テキストとスラッシュ
    details: "!ping も /ping も、関数を登録するだけです。1 つの関数を両方に登録することもできます。"
    link: /guide/text-commands
    linkText: テキストコマンド
  - title: Embed、ボタン、フォーム
    details: Embed、ボタン、選択メニュー、入力フォーム、Components V2 を、型の付いたビルダーで組み立てます。
    link: /guide/buttons
    linkText: Embed とボタン
  - title: 3 秒ルールへの対応
    details: 時間のかかる処理は defer で先に受け付け、終わってから結果を返せます。
    link: /guide/responding
    linkText: 応答のしかた
  - title: 勝手なことをしない
    details: 標準出力には何も書かず、Intent も自動では追加しません。ハンドラーのエラーは、Bot::on_error で一件ずつ受け取れます。
    link: /guide/errors
    linkText: エラーと自動で送る文言
---

## 最小の例

`!ping` に `Pong!` と返す Bot です。

```rust
use disnana_akari::model::gateway::Intents;
use disnana_akari::{Bot, CommandContext, Commands, Result};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut commands = Commands::new("!")?;
    commands.register("ping", ping)?;
    Bot::from_env()?
        .add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)
        .run_commands(commands)
        .await
}

async fn ping(ctx: CommandContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}
```

```sh
cargo add disnana-akari@0.3
cargo add tokio --features macros,rt
```

テキストコマンドを試すには、Discord の Developer Portal でも **Message Content Intent** を有効にします。[Discord で Bot を用意する](/getting-started/discord-setup)

## 読み進め方

1. [インストール](/getting-started/install) → [Discord で Bot を用意する](/getting-started/discord-setup) → [はじめての Bot](/getting-started/first-bot)
2. Rust が初めてなら [Rust が初めての人へ](/getting-started/rust-basics)、discord.py の経験があるなら [対応表](/getting-started/from-discordpy)
3. 作りたい機能を [ガイド](/guide/text-commands) から選ぶ
4. 型やメソッドの一覧は [API リファレンス](/reference/api)
