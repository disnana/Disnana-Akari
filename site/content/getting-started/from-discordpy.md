# discord.py との対応表

discord.py を使ったことがある人向けに、よく使う操作の対応をまとめました。完全に同じ動きではないので、細かい違いは各ガイドを見てください。

## 考え方の違い

| | discord.py | Akari |
| --- | --- | --- |
| コマンドの登録 | `@bot.command()` などのデコレーター | `commands.register("名前", 関数)` |
| 実行時の情報 | `ctx` / `interaction` | 種類ごとのコンテキスト（`CommandContext`、`SlashContext` など） |
| エラー | 例外を投げる | `Result` を返す（`?` で伝える） |
| 設定の置き場所 | `Bot(...)` の引数 | `Bot` のメソッドチェーン（`.with_events(...)` など） |

## コマンドとイベント

| やりたいこと | discord.py | Akari |
| --- | --- | --- |
| テキストコマンド | `@bot.command()` | [`Commands::register`](../guide/text-commands.md) |
| 別名を付ける | `@bot.command(aliases=[...])` | [`Commands::register_with_aliases`](../guide/text-commands.md#コマンドに別名を付ける) |
| 複数のプレフィックス | `Bot(command_prefix=[...])` | [`Commands::with_prefixes`](../guide/text-commands.md#コマンドに別名を付ける) |
| 引数を読む | 関数の引数 | `ctx.args()`（残りの本文を丸ごと受け取る） |
| スラッシュコマンド | `@bot.tree.command()` | [`SlashCommands::register`](../guide/slash-commands.md) |
| 両対応のコマンド | `@bot.hybrid_command()` | [`register_hybrid`](../guide/hybrid-commands.md) |
| `on_ready` | `async def on_ready()` | [`events.on_ready(関数)`](../guide/events.md) |
| `on_message` | `async def on_message(m)` | `events.on_message(関数)` |
| 編集・削除を受け取る | `on_raw_message_edit`、`on_raw_message_delete`、`on_raw_bulk_message_delete` | [`Events` の raw ハンドラー](../guide/events.md#編集・削除を受け取る) |
| リアクションを受け取る | `on_raw_reaction_add` など | [`Events` の raw リアクション](../guide/events.md#リアクションの追加・削除を受け取る) |
| クールダウン | `@commands.cooldown(1, 5)` | [`Cooldown`](../guide/cooldowns.md) |
| ステータスの変更 | `await bot.change_presence(...)` | [`PresenceHandle::set`](../guide/presence.md)（待たずに戻り、最新の値を送ります） |
| Cog（機能のまとまり） | `class MyCog(commands.Cog)` | [`Module`](../guide/modules.md)（登録のしかたをまとめるだけの trait） |
| 定期処理 | `discord.ext.tasks.loop` | [Tokio のタイマーと Ready イベント](../guide/tasks.md) |
| 権限チェック | `@commands.has_permissions(...)` | [`ctx.require_permissions(..)?`](../guide/permissions.md)（呼んだときだけ確認） |
| サブコマンド | `app_commands.Group` | [`slash.group(..)`](../guide/slash-commands.md) |
| オートコンプリート | `@cmd.autocomplete(...)` | [`slash.autocomplete(..)`](../guide/slash-commands.md) |

## 返信する

| やりたいこと | discord.py | Akari |
| --- | --- | --- |
| メッセージに返信 | `await ctx.reply("...")` | `ctx.reply("...").await?` |
| スラッシュの最初の応答 | `await interaction.response.send_message("...")` | `ctx.reply("...").await?` |
| 本人にだけ見える応答 | `send_message(..., ephemeral=True)` | `ctx.reply_ephemeral("...").await?` |
| 処理中にする | `await interaction.response.defer()` | `ctx.defer().await?`（本人のみは `defer_ephemeral`） |
| 追加のメッセージ | `await interaction.followup.send("...")` | `ctx.followup("...").await?` |
| ボタンのメッセージを更新 | `interaction.response.edit_message(...)` | `ctx.update_message(&message).await?` |
| フォームを開く | `interaction.response.send_modal(modal)` | `ctx.show_modal(&form).await?` |

詳しくは [応答のしかた](../guide/responding.md) を見てください。

## UI 部品

| やりたいこと | discord.py | Akari |
| --- | --- | --- |
| Embed | `discord.Embed(title=...)` | [`ui::Embed::new(title)`](../guide/buttons.md) |
| ボタン | `discord.ui.Button` + `View` | `ui::Button::primary(custom_id, label)` |
| ボタンの処理 | `button.callback` | `events.on_button(custom_id, 関数)` |
| 選択メニュー | `discord.ui.Select` | [`ui::StringSelect`](../guide/select-menus.md) / `events.on_select` |
| ユーザー・ロール選択 | `discord.ui.UserSelect` など | `UserSelect` / `RoleSelect` / `ChannelSelect` / `MentionableSelect` |
| モーダル | `discord.ui.Modal` | [`ui::Modal`](../guide/modals.md) / `events.on_modal` |
| Components V2 | `discord.ui.LayoutView` | [`ui::ComponentsV2`](../guide/components-v2.md) |

## 大きな違い

- **ボタンに `View` はありません。** ボタンは `custom_id`（文字列）で識別し、`events.on_button` に関数を登録します。メッセージごとのオブジェクトを持たないので、Bot を再起動しても、同じ `custom_id` のボタンは反応し続けます。
- **ボタンの押下は誰でもできます。** discord.py の `interaction_check` のような仕組みは自動では付きません。操作できる人を限定したいときは、[ボタンの操作者を確認する](../guide/buttons.md#ボタンの操作者を確認する) を見てください。
- **コマンドは並行して実行されます。** 遅いコマンドがあっても、ほかのコマンドは止まりません。[Akari の動き方](../guide/how-it-works.md)
- **スラッシュコマンドは必ず応答します。** 応答せずに終わると、Akari が「処理できませんでした」と返します（文言は変えられます）。
- **Intent は自動で追加されません。** discord.py の `intents=` と同じように、`add_intents` で指定します。[Intent の設定](../guide/intents.md)
- **エラーは `Bot::on_error` で受け取ります。** discord.py の `on_error` / `on_command_error` にあたります。ログは、Rust の標準の `tracing` で出します（discord.py の `logging` にあたります）。Akari は、標準出力にも標準エラー出力にも書きません。[エラーと自動で送る文言](../guide/errors.md)
