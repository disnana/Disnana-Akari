# 0.3 から 0.4 への移行

## 依存関係を更新する

`Cargo.toml` の Akari の依存を `disnana-akari = "0.4"` に変え、Bot のプロジェクトで `cargo check` とテストを実行してください。最小対応 Rust は 1.89 です。CI では公開済みの `v0.3.0` と公開 API を比較しています。

既存コマンドの登録や Intent の設定は引き続き使えます。たとえば、メンバーを Gateway で照会する機能を導入するときは、取得する情報に応じて Intent を追加します。 [Intent の選び方](../guide/intents.md)

## 新しい機能を取り入れる

- 権限判定には `ctx.access()`、管理操作には `ctx.manage()` を使えます。管理コマンドでは実行者を認可したうえで操作を呼びます。 [権限と管理操作](../guide/permissions.md)
- 複数シャード全体や初期メンバー取得を待ってから仕事を始めたい場合は `StartupReadiness` を使います。シャードごとの `Ready` は引き続き接続単位の通知です。 [Akari の動き方](../guide/how-it-works.md)
- 対話画面の一連の操作を期限付きで受け付けるには `UiSessions` を使います。長い処理に続いて投票を返す場合は、公開の `defer()` の後で `reply_poll` を呼びます。 [ボタンと画面の状態](../guide/buttons.md) · [投票で応答する](../guide/responding.md#投票で応答する)
- Webhook を使った投稿・編集も、メンション通知を抑止する API で実行できます。トークンを含むパスを避けるため、ログの購読対象は Akari に絞ってください。 [Webhook の操作](../guide/discord-rest.md#webhook-を使ってメッセージを送る) · [ログの設定](../guide/logging.md)
