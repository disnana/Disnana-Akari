# よくある質問とトラブルシューティング

## 起動できない

| 症状 | 確認すること |
| --- | --- |
| `DISCORD_TOKEN に Discord Bot トークンを設定してください` | `cargo run` と同じターミナルで、環境変数 `DISCORD_TOKEN` を設定したか確認します。[手順](../getting-started/discord-setup.md#トークンを渡す) |
| Gateway がコード `4004` で閉じる | トークンが正しいか確認します。**Reset Token** で作り直した場合は、環境変数も更新します。 |
| 起動時に「Intent … が必要です」と出る | テキストコマンドやメッセージのハンドラーには、`GUILD_MESSAGES` と `MESSAGE_CONTENT` が必要です。`add_intents` で追加します。[Intent の設定](../guide/intents.md) |
| Gateway がコード `4014` で閉じる | Developer Portal で **Message Content Intent** を有効にしたか確認します。理由は `Bot::on_error` に報告されます。 |
| スラッシュコマンドを登録できない | Bot を、そのサーバーに `applications.commands` スコープ付きで招待したか、サーバー ID が正しいか確認します。 |

## 反応しない

| 症状 | 確認すること |
| --- | --- |
| `!ping` に反応しない | 人間のアカウントから、半角で `!ping` と送ります。Bot にそのチャンネルが見えているか確認します。`with_intents` を使っているなら、`GUILD_MESSAGES` と `MESSAGE_CONTENT` を含めます。 |
| 反応するが、返信が出ない | **View Channel**、**Send Messages**、**Read Message History** の権限を確認します。スレッドでは **Send Messages in Threads** も必要です。 |
| `/ping` が候補に出ない | 指定したサーバーを開いているか、そのチャンネルでアプリのコマンドが使えるかを確認します。 |
| 消したはずの `/` コマンドが候補に残る | 既定では、既存のコマンドが残ります。[一括同期](../guide/slash-commands.md#登録済みのコマンドを整理する)を使います。 |
| Embed が表示されない | テキストコマンドの返信では、Bot に **Embed Links** の権限が必要です。 |

## 応答まわり

| 症状 | 確認すること |
| --- | --- |
| 「アプリケーションが応答しませんでした」と出る | 3 秒以内に最初の応答を送れていません。時間のかかる処理の前に `ctx.defer().await?` を呼びます。[応答のしかた](../guide/responding.md) |
| 「現在混雑しています」と返る | 同時に処理できる数か、待てる数がいっぱいです。少し待って、もう一度試します。[Akari の動き方](../guide/how-it-works.md) |
| 「コマンドを処理できませんでした。」と返る | スラッシュコマンドが `Err` を返したか、何も返信せずに終わりました。詳細は `Bot::on_error` に報告されます（未設定なら標準エラー出力）。 |
| テキストコマンドが失敗しても、何も返信されない | 既定では返信しません。エラーは `Bot::on_error` に報告されます。返信したいときは `Messages::text_command_failed` を設定します。[エラーと自動で送る文言](../guide/errors.md) |
| `reply_ephemeral` がエラーになる | 公開の `defer` のあとは、本人にだけ見せる応答に変えられません。最初から `defer_ephemeral` を使います。 |

## よくある質問

### グローバルなスラッシュコマンドは作れますか？

作れます。`SlashCommands::for_guild` の代わりに `SlashCommands::global()` を使います。Discord 側の反映に時間がかかることがあるので、開発中はサーバー指定、公開するときにグローバルにするのがおすすめです（[スラッシュコマンド](../guide/slash-commands.md)の「すべてのサーバーで使う」）。

### ダイレクトメッセージで動かせますか？

テキストコマンドとメッセージの受信は、サーバー内のメッセージが対象です。

### 複数のサーバーで使えますか？

テキストコマンドは、Bot が参加しているどのサーバーでも動きます。スラッシュコマンドは、登録したサーバーで使えます。

### データベースなど、状態はどこに持てばいいですか？

`async move` のクロージャで、`Arc` で包んだ値（`Arc<Mutex<...>>` や、データベースの接続プール）を持たせます。[テキストコマンド](../guide/text-commands.md#コマンドの間で状態を共有する)を参照してください。

### Bot のトークンが漏れたかもしれません

すぐに Developer Portal で **Reset Token** を行い、古いトークンを無効にします。Git の履歴にコミットしてしまった場合も、同様にトークンを再発行してください。

### 最新のバージョンはどこで確認できますか？

[crates.io](https://crates.io/crates/disnana-akari) と、[変更履歴](changelog.md)で確認できます。

### バグを見つけました

[GitHub の Issues](https://github.com/disnana/Disnana-Akari/issues) に報告してください。脆弱性は、公開の Issue にせず、リポジトリの SECURITY.md の手順で報告してください。
