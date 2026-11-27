# 変更履歴

## 0.2.0

勝手な動作を減らし、エラーと文言を呼び出し側が決められるようにしました。あわせて、権限の確認、サブコマンド、オートコンプリート、グローバルコマンド、モジュールなど、コマンドまわりの機能を追加しました。**互換性のない変更を含みます。** 移行の手引きは、ガイドの「0.1 から 0.2 への移行」を見てください。

### 互換性のない変更

- **標準エラー出力への既定の出力をやめて、`tracing` のログに変えました。** 0.1 は、`Bot::on_error` を設定しないと、エラーを `[akari] ...` の形で標準エラー出力に書いていました。0.2 は、標準出力にも標準エラー出力にも書きません。エラーと実行時の情報を、`tracing` のイベント（`target` は `disnana_akari`）として発行するので、見たいときは、アプリの起動の最初に `tracing_subscriber::fmt::init();` などで購読先を設定してください。`on_error` と `on_info` は、これまでどおり使えます。依存に `tracing` を追加しました（Twilight の依存にすでに入っていたもので、依存の総数は増えません）。
- **Intent を自動で追加しなくなりました。** これまでは、テキストコマンドやメッセージのハンドラーを登録すると、特権 Intent の `MESSAGE_CONTENT` も黙って要求していました。今後は `Bot::add_intents`（または `with_intents`）で指定します。足りないときは、Discord に接続する前に、足りない Intent と対処を示すエラーを返します。要求する Intent は `Bot::intents()` で確認できます。
- **ライブラリが標準出力に書かなくなりました。** 接続時の「Discord に接続しました」を削除しました。接続を知りたい場合は `Events::on_ready` に書きます。
- **`Bot::with_messages` と `Messages` を追加しました。** Akari が利用者へ自動で送る文言を、変更したり、送らないようにしたりできます。**テキストコマンドの失敗と混雑の返信は、既定で送らなくなりました**（以前は、失敗時に「コマンドを処理できませんでした。」を元の投稿へ公開で返信していました）。スラッシュコマンド、ボタン、選択メニュー、フォームの文言は、Discord の応答期限のために、既定を残しています。
- **`rustls` の暗号プロバイダーを、ライブラリが登録しなくなりました。** `rustls` が `ring` を自動で選びます。アプリで別のプロバイダーも有効にする場合は、起動前にアプリ側で登録してください。
- `CommandSource` と `Mentionable` は `#[non_exhaustive]` になりました。外部クレートの `match` には `_` の分岐を付けてください。
- **`Result<T>` と `ErrorReport::error` が型付きの `Error` を使うようになりました。** Intent の不足や Discord API の失敗は列挙型の各バリアントで判別できます。アプリ固有のエラーは `Error::handler` で包めます。
- **文字列だった `Error` のバリアントを、型付きに置き換えました。** `InvalidRegistration`、`InvalidSelection`、`InvalidResponseState`、`CommandSync`、`QueueFull(&str)` をやめ、`Registration(RegistrationError)`、`Selection(SelectionError)`、`ResponseState(ResponseStateError)`、`CommandSync(CommandSyncError)`、`QueueFull(QueueKind)` にしました。メッセージの文言ではなく、バリアントで判別できます。
- **エラーの連鎖で、メッセージが二重に表示されなくなりました。** `Error` の `Http`、`GatewayReceive`、`*Validation` は、表示と原因を元のエラーへそのまま渡します。`Error::Handler` は `HandlerError` を持つようになり（`downcast_ref`、`inner`、`into_inner` で元のエラーを取り出せます）、`CommandRegistration` の表示から原因を外しました（原因は `source()` で取得します）。
- **ハンドラーが、任意のエラー型を返せるようになりました。** コマンド、スラッシュコマンド、ハイブリッドコマンド、イベント、ボタン、選択メニュー、フォーム、`Bot::run` のハンドラーは、`Result<(), E>`（`E: Into<Box<dyn Error + Send + Sync>>`）を返せます。`io::Error` や `ParseIntError` などに、そのまま `?` を使えます。Akari の `Error` は、包み直さずに通ります。クロージャが `Ok(())` で終わるときは、戻り値の型の指定が必要です。
- **`SlashCommands::replace_guild_commands` は `replace_commands` に名前を変えました。** サーバーとグローバルのどちらにも使えます。`InfoEvent::SlashCommandsSynced` の `guild_id` は `Option` になりました（グローバルは `None`）。
- 依存に `twilight-util`（権限の計算だけ）を追加しました。

### 追加

- **Bot のステータスとアクティビティを設定できるようにしました。** `PresenceHandle` と `Bot::with_presence` で、起動時の設定と、実行中の変更（`set`、`set_status`、`set_activity`）ができます。送信キューに入れる前の更新は最新の値にまとめ、追加の間隔を 5 秒以上空けます。再接続したときは、最後の値を再びキューに入れます。`Status::Offline` は `Invisible` として送ります。
- **権限付きのギルドコマンドを追加しました。** `SlashCommands::register_with_permissions` と `group_with_permissions` が、Discord の一覧での表示条件と、ハンドラーを呼ぶ前の権限チェックを、同じ権限でまとめて設定します。足りないときは、ハンドラーを呼ばずに `Messages::check_failed` を本人にだけ返し、入力補完は空の候補を返します。グローバル登録では使えません。
- **ログを `tracing` で発行します。** `error`（ハンドラーのエラー、Gateway の切断）、`warn`（処理枠の満杯、自動応答の失敗）、`info`（Gateway の準備と再開、コマンドの同期）、`debug`（Interaction への初回応答と時間）です。
- **`Bot::on_error` を追加しました。** ハンドラーが返したエラー、Gateway の切断と受信エラー、処理枠の満杯でイベントを捨てたこと、自動応答の失敗を、`ErrorReport`（場所、名前、内容）で一件ずつ受け取れます。Gateway が接続を閉じたときは、理由を含めて報告します（例: コード 4014 は特権 Intent の案内）。
- `Bot::on_info` を追加しました。Gateway のセッション、スラッシュコマンドの同期、Interaction の初回応答の計測値をオプトインで受け取れます。
- **モジュールを追加しました。** `Module` trait と `Registry` で、関連するコマンドやイベントの登録を、ひとまとまりにできます（discord.py の Cog にあたりますが、継承もマクロもありません）。
- **オートコンプリートに対応しました。** `SlashCommands::autocomplete(path, option, handler)` で、引数に入力中の候補を返す関数を付けられます（`AutocompleteContext::{focused, respond}`、候補は `string_choice` などで作ります）。付けた引数は、Akari が自動で `autocomplete` を有効にします。
- **サブコマンドに対応しました。** `SlashCommands::group` と `CommandGroup::{subcommand, group}` で、`/config set` や `/config advanced reset` を登録できます（コマンド → グループ → サブコマンドの最大 3 段）。`SlashContext::option` などはサブコマンドの引数も読み、`SlashContext::command_path` で呼ばれた経路を取得できます。`on_error` に渡る名前は `config advanced reset` の形になります。
- **テキストコマンドとハイブリッドの権限確認を追加しました。** `Context` と `CommandContext`、`HybridContext` の `require_permissions(..).await?`、`permissions().await`、`require_guild()` です。メッセージには権限が付かないため、サーバーとチャンネルを Discord から取得し、Twilight 公式の `twilight-util`（権限計算のみ）で計算します。`Bot::cache_permissions(ttl)` で取得した情報を保存できます（既定は保存しません）。依存に `twilight-util` を追加しました。
- **スラッシュコマンドの権限確認を追加しました。** `SlashContext::{require_permissions, require_guild, permissions, guild_id}` で、呼び出した人の権限を確認できます（Discord が計算した値を使います）。失敗は `Error::Check(CheckFailure)` で、既定では `Messages::check_failed` を本人にだけ返します。`SlashCommands::default_member_permissions` で、権限のない人からコマンドを隠せます。
- **グローバルコマンドに対応しました。** `SlashCommands::global()` で、すべてのサーバーと DM 向けに登録できます。**`replace_guild_commands` は `replace_commands` に名前を変えました**（サーバー、グローバルのどちらにも使えます）。`InfoEvent::SlashCommandsSynced` の `guild_id` は `Option` になりました（グローバルは `None`）。
- 引数の型変換を追加しました。`SlashCommands::{integer_option, number_option, boolean_option}` で型付きの引数を作り、`SlashContext::{integer, number, boolean}` で受け取れます。`SlashContext::parse::<T>` と `HybridContext::parse::<T>` は、文字列の引数を `FromStr` で変換します（未入力は `None`、変換の失敗は `Some(Err(..))`）。
- `Bot::limits` と `Limits` を追加しました。同時に処理する数（テキストコマンド、Interaction、混雑の通知）と、待ち行列の大きさを変えられます。既定は、これまでどおりの 8、8、8、64 です。0 を指定すると、起動時に `Error::InvalidLimits` を返します。
- `Context::reply_with` と `CommandContext::reply_with` を追加しました。通知する相手（返信元の著者、ユーザー、ロール）を `AllowedMentions` で指定して返信できます。`reply` は、これまでどおり何も通知しません。
- `Bot::include_bots` を追加しました。他の Bot やウェブフックの投稿も、コマンドとメッセージのハンドラーへ渡せます（既定は、これまでどおり無視します。この Bot 自身の投稿は、常に渡しません）。
- `Error::msg` を追加しました（メッセージだけのアプリ固有のエラー）。`Result<T>` は `Result<T, E = Error>` になりました。

### 変更・改善

- `GUILDS` Intent と権限キャッシュを有効にすると、Gateway の変更イベントで該当するキャッシュを破棄します。同じデータへの同時取得は REST リクエスト1件にまとめます。
- テキストコマンドでは、スレッドに投稿できるかを添付や埋め込みなどの権限に反映します。メッセージに有効なタイムアウトの期限がある場合は、一般メンバーの権限を閲覧と履歴の読み取りに絞ります。
- 公開の遅延応答を送ったスラッシュコマンドでも、権限チェックの失敗文言を実行者にだけ送るようにしました。
- `ErrorReport` を `{report}` で表示すると、原因の連鎖も `: 原因` の形で続けて表示します。`tracing` のログにも、本当の理由が含まれます。
- ドキュメントに、Twilight の型を公開 API に含める方針（安定性）を明記しました。
- `Bot` や各コンテキストなどに `Debug` を追加しました。`Bot` のトークンは `<redacted>` と表示されます。

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
