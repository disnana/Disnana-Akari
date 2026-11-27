# 変更履歴

## Unreleased

- **ハンドラーが、任意のエラー型を返せるようになりました。** コマンド、スラッシュコマンド、ハイブリッドコマンド、イベント、ボタン、選択メニュー、フォーム、`Bot::run` のハンドラーは、`Result<(), E>`（`E: Into<Box<dyn Error + Send + Sync>>`）を返せます。`io::Error` や `ParseIntError` などに、そのまま `?` を使えます。Akari の `Error` は、包み直さずに通ります。クロージャが `Ok(())` で終わるときは、戻り値の型の指定が必要です。
- `Error::msg` を追加しました（メッセージだけのアプリ固有のエラー）。`Result<T>` は `Result<T, E = Error>` になりました。
- **エラーの連鎖で、メッセージが二重に表示されなくなりました。** `Error` の `Http`、`GatewayReceive`、`*Validation` は、表示と原因を元のエラーへそのまま渡します。`Error::Handler` は `HandlerError` を持つようになり（`downcast_ref`、`inner`、`into_inner` で元のエラーを取り出せます）、`CommandRegistration` の表示から原因を外しました（原因は `source()` で取得します）。
- `ErrorReport` を `{report}` で表示すると、原因の連鎖も `: 原因` の形で続けて表示します。標準エラー出力の既定の表示にも、本当の理由が含まれます。
- ドキュメントに、Twilight の型を公開 API に含める方針（安定性）を明記しました。
- **グローバルコマンドに対応しました。** `SlashCommands::global()` で、すべてのサーバーと DM 向けに登録できます。**`replace_guild_commands` は `replace_commands` に名前を変えました**（サーバー、グローバルのどちらにも使えます）。`InfoEvent::SlashCommandsSynced` の `guild_id` は `Option` になりました（グローバルは `None`）。
- 引数の型変換を追加しました。`SlashCommands::{integer_option, number_option, boolean_option}` で型付きの引数を作り、`SlashContext::{integer, number, boolean}` で受け取れます。`SlashContext::parse::<T>` と `HybridContext::parse::<T>` は、文字列の引数を `FromStr` で変換します（未入力は `None`、変換の失敗は `Some(Err(..))`）。
- `Bot::limits` と `Limits` を追加しました。同時に処理する数（テキストコマンド、Interaction、混雑の通知）と、待ち行列の大きさを変えられます。既定は、これまでどおりの 8、8、8、64 です。0 を指定すると、起動時に `Error::InvalidLimits` を返します。
- `Context::reply_with` と `CommandContext::reply_with` を追加しました。通知する相手（返信元の著者、ユーザー、ロール）を `AllowedMentions` で指定して返信できます。`reply` は、これまでどおり何も通知しません。
- `Bot::include_bots` を追加しました。他の Bot やウェブフックの投稿も、コマンドとメッセージのハンドラーへ渡せます（既定は、これまでどおり無視します。この Bot 自身の投稿は、常に渡しません）。
- **文字列だった `Error` のバリアントを、型付きに置き換えました。** `InvalidRegistration`、`InvalidSelection`、`InvalidResponseState`、`CommandSync`、`QueueFull(&str)` をやめ、`Registration(RegistrationError)`、`Selection(SelectionError)`、`ResponseState(ResponseStateError)`、`CommandSync(CommandSyncError)`、`QueueFull(QueueKind)` にしました。メッセージの文言ではなく、バリアントで判別できます。

勝手な動作を減らし、エラーと文言を呼び出し側が決められるようにしました。**互換性のない変更を含みます。**

- `Bot::on_info` を追加しました。Gateway のセッション、スラッシュコマンドの同期、Interaction の初回応答の計測値をオプトインで受け取れます。
- **`Result<T>` と `ErrorReport::error` が型付きの `Error` を使うようになりました。** Intent の不足や Discord API の失敗は列挙型の各バリアントで判別できます。アプリ固有のエラーは `Error::handler` で包めます。
- `CommandSource` と `Mentionable` は `#[non_exhaustive]` になりました。外部クレートの `match` には `_` の分岐を付けてください。
- `Bot` や各コンテキストなどに `Debug` を追加しました。`Bot` のトークンは `<redacted>` と表示されます。
- **Intent を自動で追加しなくなりました。** これまでは、テキストコマンドやメッセージのハンドラーを登録すると、特権 Intent の `MESSAGE_CONTENT` も黙って要求していました。今後は `Bot::add_intents`（または `with_intents`）で指定します。足りないときは、Discord に接続する前に、足りない Intent と対処を示すエラーを返します。要求する Intent は `Bot::intents()` で確認できます。
- **ライブラリが標準出力に書かなくなりました。** 接続時の「Discord に接続しました」を削除しました。接続を知りたい場合は `Events::on_ready` に書きます。
- **`Bot::on_error` を追加しました。** ハンドラーが返したエラー、Gateway の切断と受信エラー、処理枠の満杯でイベントを捨てたこと、自動応答の失敗を、`ErrorReport`（場所、名前、内容）で一件ずつ受け取れます。設定しない場合は、これまでどおり標準エラー出力に書きます（形式は `[akari] ...` に変わりました）。Gateway が接続を閉じたときは、理由を含めて報告します（例: コード 4014 は特権 Intent の案内）。
- **`Bot::with_messages` と `Messages` を追加しました。** Akari が利用者へ自動で送る文言を、変更したり、送らないようにしたりできます。**テキストコマンドの失敗と混雑の返信は、既定で送らなくなりました**（以前は、失敗時に「コマンドを処理できませんでした。」を元の投稿へ公開で返信していました）。スラッシュコマンド、ボタン、選択メニュー、フォームの文言は、Discord の応答期限のために、既定を残しています。
- **`rustls` の暗号プロバイダーを、ライブラリが登録しなくなりました。** `rustls` が `ring` を自動で選びます。アプリで別のプロバイダーも有効にする場合は、起動前にアプリ側で登録してください。

## 0.1.1

- バージョンタグの push から、GitHub Actions と crates.io の Trusted Publishing を使って公開できるようにしました。タグとバージョンを照合し、CI と RustSec 監査が通過してから公開します。
- 日本語の公開手順に、次のバージョンの準備とタグの作成方法を追加しました。
- crates.io への公開後に GitHub Release を作成し、前版からのコミット、マージされた PR、コントリビューターを掲載します。配布アーカイブには SHA-256 チェックサムと署名付き来歴証明を添付します。

## 0.1.0

- Twilight 上で Bot を起動し、プレフィックス付きテキストコマンド、サーバー向けスラッシュコマンド、両方で関数を共有するハイブリッドコマンドを登録できます。
- Gateway の受信とコマンドの実行を分けました。イベントハンドラーの登録、Gateway RTT と処理時間の確認、ユーザーごとのクールダウンにも対応しました。
- Discord への初回返信、遅延応答、追加メッセージと、明示的な設定によるサーバーのスラッシュコマンドの一括同期に対応しました。
- Embed、ボタン、Components V2、選択メニュー、モーダルを追加しました。ボタン操作から元のメッセージを更新でき、選択メニューでは文字列、ユーザー、ロール、チャンネルを扱えます。Mentionable Select ではユーザーとロールを同じメニューから選べ、選択結果を型付きで受け取れます。
- ボタンやセレクトメニューの `resolve_invoker_id()` で、操作データに含まれない返信先の投稿者を Discord REST API から確認できます。
- 最小対応バージョンは Rust 1.89 です。GitHub Actions で整形、Clippy、Linux・Windows のテスト、配布パッケージのビルド、RustSec による依存関係の監査を実行します。
