# トラブルシューティング

| 症状 | 確認すること |
| --- | --- |
| `playground/.env の DISCORD_TOKEN に Bot トークンを設定してください` | ファイルを UTF-8 で保存し、`DISCORD_TOKEN=` の右辺に Bot トークンを入力します。 |
| `DISCORD_TOKEN に Discord Bot トークンを設定してください` | `cargo run` と同じシェルで、Discord Developer Portal の **Bot** タブから取得したトークンを設定します。 |
| Gateway がコード `4004` で閉じる | Bot トークンが有効か確認します。トークンを再発行した場合は環境変数も更新します。 |
| Gateway がコード `4014` で閉じる | **Message Content Intent** が有効か、アプリに使用が許可されているかを確認します。 |
| 接続済みだが `!ping` に反応しない | テスト用サーバーのチャンネルで、人間のアカウントから半角で `!ping` と送ります。Bot にチャンネルが見えているか確認します。`with_intents` を使う場合は `GUILD_MESSAGES` と `MESSAGE_CONTENT` を含めます。追加だけなら `add_intents` を使えます。 |
| メッセージ処理が権限エラーになる | **View Channel**、**Send Messages**、**Read Message History** を確認します。スレッドで試す場合は **Send Messages in Threads** も確認します。 |
| スラッシュコマンドを登録できない | `DISCORD_GUILD_ID` にサーバー ID を指定します。そのサーバーにトークンと同じ Bot アプリがインストールされていることを確認し、Bot を再起動します。 |
| `/ping` が候補に出ない | `DISCORD_GUILD_ID` で指定したサーバーを開き、チャンネルでアプリケーションコマンドを使えることを確認します。 |
| 消した `/` コマンドが候補に残る | 既定では既存コマンドが保持されます。Playground がそのサーバーでの Bot の全コマンドを管理する場合は、クイックスタートの `DISCORD_REPLACE_GUILD_COMMANDS` で一括同期できます。 |
| 「現在混雑しています」と返る | スラッシュコマンドの同時実行枠か受信キューが埋まっています。少し待って同じ操作を試します。 |
| Discord に「アプリケーションが応答しませんでした」と表示される | Bot が起動中で、同名の別アプリを選んでいないことを確認します。初回応答は受信後 3 秒以内です。時間のかかる処理の前に `ctx.defer().await?` を呼びます。 |
| `playground/Cargo.toml` が見つからない | リポジトリのルートに移動します。新規クローンの場合はクイックスタートに沿って実験用プロジェクトを作ります。 |

[クイックスタート](quickstart.md) · [Discord の Gateway 終了コード](https://docs.discord.com/developers/topics/opcodes-and-status-codes#gateway-gateway-close-event-codes) · [Discord のスラッシュコマンド仕様](https://docs.discord.com/developers/interactions/application-commands)
