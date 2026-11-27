# トラブルシューティング

| 症状 | 確認すること |
| --- | --- |
| `playground/.env の DISCORD_TOKEN に Bot トークンを設定してください` | ファイルを UTF-8 で保存し、`DISCORD_TOKEN=` の右辺に Bot トークンを入力します。 |
| `DISCORD_TOKEN に Discord Bot トークンを設定してください` | `cargo run` と同じシェルで、Discord Developer Portal の **Bot** タブから取得したトークンを設定します。 |
| Gateway がコード `4004` で閉じる | Bot トークンが有効か確認します。トークンを再発行した場合は環境変数も更新します。 |
| Gateway がコード `4014` で閉じる | **Message Content Intent** が有効か、アプリに使用が許可されているかを確認します。理由は `Bot::on_error` に報告されます（設定していなければ標準エラー出力に出ます）。 |
| 起動時に `Error::MissingIntents`（「Intent … が必要です」）が返る | テキストコマンドやメッセージのハンドラーには、`GUILD_MESSAGES` と `MESSAGE_CONTENT` が必要です。`Bot::add_intents(Intents::GUILD_MESSAGES \| Intents::MESSAGE_CONTENT)` で追加します。Akari は Intent を自動では追加しません。 |
| ハンドラーでアプリ固有のエラーに `?` を使うと型が合わない | `.map_err(Error::handler)?` で原因を包みます。既に `Box<dyn std::error::Error + Send + Sync>` なら、そのまま `?` で変換できます。 |
| 接続済みだが `!ping` に反応しない | テスト用サーバーのチャンネルで、人間のアカウントから半角で `!ping` と送ります。Bot にチャンネルが見えているか確認します。`Bot::add_intents` で `GUILD_MESSAGES` と `MESSAGE_CONTENT` を追加しているか確認します。 |
| メッセージ処理が権限エラーになる | **View Channel**、**Send Messages**、**Read Message History** を確認します。スレッドで試す場合は **Send Messages in Threads** も確認します。 |
| スラッシュコマンドを登録できない | `DISCORD_GUILD_ID` にサーバー ID を指定します。そのサーバーにトークンと同じ Bot アプリがインストールされていることを確認し、Bot を再起動します。 |
| `/ping` が候補に出ない | `DISCORD_GUILD_ID` で指定したサーバーを開き、チャンネルでアプリケーションコマンドを使えることを確認します。 |
| 消した `/` コマンドが候補に残る | 既定では既存コマンドが保持されます。Playground がそのサーバーでの Bot の全コマンドを管理する場合は、クイックスタートの `DISCORD_REPLACE_GUILD_COMMANDS` で一括同期できます。 |
| 「現在混雑しています」と返る | スラッシュコマンドやボタン操作の同時実行枠か受信キューが埋まっています。少し待って同じ操作を試します。テキストコマンドは、既定では返信せず、`Bot::on_error` に報告します。 |
| 失敗したのに、何も表示されない | エラーは `Bot::on_error` に報告されます。設定していなければ、標準エラー出力に `[akari] ...` と出ます。テキストコマンドの失敗は、既定では返信しません。`Messages::text_command_failed` で、返信する文言を設定できます。 |
| 初回応答にかかる時間を確認したい | `Bot::on_info` で `InfoEvent::InteractionResponded` の `elapsed` と `rest_elapsed` を受け取れます。初回応答が受け付けられた操作を計測します。 |
| Discord に「アプリケーションが応答しませんでした」と表示される | Bot が起動中で、同名の別アプリを選んでいないことを確認します。初回応答は受信後 3 秒以内です。時間がかかる可能性のあるスラッシュコマンドでは、その処理を始める前にハンドラーから `ctx.defer().await?` を呼びます。 |
| `playground/Cargo.toml` が見つからない | リポジトリのルートに移動します。新規クローンの場合はクイックスタートに沿って実験用プロジェクトを作ります。 |

[クイックスタート](quickstart.md) · [Discord の Gateway 終了コード](https://docs.discord.com/developers/topics/opcodes-and-status-codes#gateway-gateway-close-event-codes) · [Discord のスラッシュコマンド仕様](https://docs.discord.com/developers/interactions/application-commands)
