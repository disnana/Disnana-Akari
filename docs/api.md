# API リファレンス

## `Bot`

| メソッド | 動作 |
| --- | --- |
| `Bot::new(token) -> Result<Bot>` | トークンの前後の空白を除去して Bot を作ります。値が空の場合はエラーを返します。 |
| `Bot::from_env() -> Result<Bot>` | 環境変数 `DISCORD_TOKEN` から Bot を作ります。 |
| `bot.with_events(events) -> Bot` | Gateway イベントに応じて呼び出す関数の一覧を設定します。 |
| `bot.with_presence(handle) -> Bot` | 起動時と実行中の Discord 上のステータスを共有ハンドルから設定します。 |
| `bot.with_slash_commands(commands) -> Bot` | スラッシュコマンドを指定サーバーで有効にします。 |
| `bot.with_intents(intents) -> Bot` | 要求する Gateway Intent を、渡された `Intents` の集合にします。既定は空です。 |
| `bot.add_intents(intents) -> Bot` | 要求する Intent に、渡された `Intents` を追加します。 |
| `bot.intents() -> Intents` | 起動時に Discord へ要求する Intent を返します。 |
| `bot.on_info(hook) -> Bot` | 実行時の情報を受け取る関数を設定します。[実行時の情報](#実行時の情報) |
| `bot.on_error(hook) -> Bot` | エラーを一件ずつ受け取る関数を設定します。[エラーと自動で送る文言](#エラーと自動で送る文言) |
| `bot.with_messages(messages) -> Bot` | Akari が利用者へ自動で送る文言を設定します。 |
| `bot.start().await -> Result<()>` | 設定したイベントとスラッシュコマンドで Bot を起動します。 |
| `bot.with_shutdown() -> (Bot, ShutdownHandle)` | `shutdown.request()` で停止を要求できます。要求後は、Gateway の切断を最大2秒、受付済みの処理を最大5秒、並行して待ち、起動メソッドが正常に終了します。 |
| `bot.with_recommended_shards() -> Bot` | Discord の推奨シャード数とセッション開始枠を起動時に調べ、同じプロセスで全シャードを起動します。通常は単一接続です。 |
| `bot.with_voice_gateway() -> (Bot, VoiceGateway)` | ボイスチャンネルの参加・移動・退出を担当シャードへ要求します。`GUILD_VOICE_STATES` が必要です。 |
| `bot.with_member_cache(capacity) -> (Bot, MemberCacheHandle)` | 保存済みメンバーを `handle.get(guild, user)` または `handle.guild_members(guild)` で参照できます。 |
| `bot.cache_members(capacity)` | キャッシュ付きのメンバー更新・退出通知が使う最大保存件数です。`GUILDS` と `GUILD_MEMBERS` が必要です。 |
| `bot.cache_messages(capacity)` | キャッシュ付きの編集・削除イベントが使うメッセージの最大保存件数を指定します。既定値と `0` は無効です。 |
| `bot.with_gateway_status() -> (Bot, GatewayStatusHandle)` | `status.shards()` で現在のシャード一覧を読み、`status.subscribe()` で変化を購読します。 |
| `bot.run_commands(commands).await -> Result<()>` | 上記に加えて、テキストコマンドを実行します。 |
| `bot.run(on_message).await -> Result<()>` | テキストコマンドの代わりに、受信メッセージを自前の `async fn on_message(ctx: Context<'_>) -> Result<()>` で処理します。 |

起動メソッドは Tokio ランタイム内で呼び出します。

### Bot のステータスとアクティビティ

`PresenceHandle::new(Presence::new(Status::Online))` を作り、クローンを `bot.with_presence(handle.clone())` に渡します。実行中はハンドラーなどから `handle.set(Presence::new(Status::Idle))` を呼びます。`Presence::with_activity` には Twilight の `MinimalActivity` を渡せます。アクティビティを消すときは `Presence::new(status)` だけで作った値を設定します。

現在のアクティビティを保ってオンライン状態だけ変える場合は `handle.set_status(status)`、オンライン状態を保ってアクティビティだけ変える場合は `handle.set_activity(Some(activity))` を使います。`handle.set_activity(None)` でアクティビティだけ消せます。`Status::Offline` を指定すると、Discord には `Status::Invisible` を送ります。

送信キューに入れる前の更新は最新の値にまとめ、キューへの追加間隔を5秒以上空けます。Gateway への送信時刻は接続状況と Twilight の送信処理に依存します。`set` は Discord での表示完了を待ちません。接続が回復したときは最後に設定した値を再びキューに入れます。[管理者向けのステータス変更サンプル](../examples/presence/main.rs)

### Intent

Akari は Intent を **自動では追加しません**。要求する Intent は、`with_intents` と `add_intents` で指定したものだけです。`with_intents` で基本の集合を決め、`add_intents` で足します。両方を指定した場合は、呼ぶ順序に関係なく和集合になります。`Intents` は `disnana_akari::model::gateway::Intents` から参照できます。

- テキストコマンド、`Events::on_message`、`Bot::run` のメッセージ処理を使うには、`GUILD_MESSAGES` と `MESSAGE_CONTENT` が必要です。足りない場合は、Discord に接続する前に、足りない Intent と対処を示すエラーを返します。
- メッセージの raw イベントは、サーバーでは `GUILD_MESSAGES`、DM の編集・単独削除では `DIRECT_MESSAGES` を指定します。一括削除を登録する場合は `GUILD_MESSAGES` が必要です。編集後の本文などを受け取れるかは Discord の `MESSAGE_CONTENT` Intent とその適用条件に従います。
- `MESSAGE_CONTENT` は特権 Intent です。Developer Portal の Bot ページでも有効にします。許可されていないと、Gateway がコード `4014` で接続を閉じます。このときの理由は、`on_error` に報告します。
- スラッシュコマンド、ボタンや選択メニューの操作、フォーム、`on_ready` だけを使う場合は、Intent は不要です。

```rust,no_run
use disnana_akari::model::gateway::Intents;
use disnana_akari::{Bot, Commands, Result};

async fn run(commands: Commands) -> Result<()> {
    Bot::from_env()?
        .add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)
        .run_commands(commands)
        .await
}
```

### TLS（暗号プロバイダー）

Akari はプロセス全体の暗号プロバイダー（`rustls` の `CryptoProvider`）を登録しません。依存の `rustls` は `ring` だけを有効にしているので、`rustls` が最初の使用時に自動で選びます。アプリで別のプロバイダー（`aws-lc-rs` など）も有効にする場合は、起動前に、アプリ側で `CryptoProvider::install_default` を呼んでください。

Bot と Webhook が送ったメッセージは除外します。復旧可能な Gateway の切断は Twilight が処理し、イベントストリームの終了やスラッシュコマンドの登録失敗は起動メソッドからエラーとして返ります。

### ハンドラーの実行順

Gateway の受信、コマンド、イベントは非同期で並行して進みます。処理ごとに次の枠とキューを使います。

| 処理 | 同時実行枠 | 受信キュー | 混雑時 |
| --- | --- | --- | --- |
| `Bot::run_commands` のテキストコマンド | 8 件 | 64 件 | `on_error` に報告します。`Messages::text_command_busy` を設定した場合だけ、元の投稿に返信します。 |
| スラッシュコマンド、引数補完、登録済みのボタン、選択メニュー、フォーム送信 | 合計 8 件 | 合計 64 件 | `on_error` に報告します。補完は空の候補を返します。その他の操作には `Messages::interaction_busy`（既定あり）を本人にだけ返します。 |
| `on_ready`、`on_message`、メッセージの raw イベントと `Bot::run` の自前ハンドラー | 1 件 | 64 件 | 新しく届いたイベントを破棄し、`on_error` に報告します。 |

テキストコマンド用のキューには、登録名に一致したメッセージだけが入ります。`on_ready` と `on_message` は受信順に処理し、一件のイベントに対する関数は登録順に呼びます。フォーム送信を含むコンポーネント操作は、スラッシュコマンドと同じ枠で並行して処理します。`Bot::run` の自前ハンドラーは、そのメッセージに対する `Events::on_message` より先に実行します。`Bot::run_commands` のコマンドと `on_message` は独立して進むため、どちらが先に完了するかは処理時間によって変わります。

混雑の返信には、テキストとスラッシュ・ボタンで別々の処理枠を使います。それぞれ同時に 8 件、キューは 64 件までです。数は `Bot::limits(Limits::default().text_workers(16))` のように変えられます（`text_workers`、`interaction_workers`、`notice_workers`、`queue_capacity`。0 は起動時にエラーです）。混雑の返信用のキューが満杯の場合や、返信に失敗した場合も、`on_error` に報告します。Gateway のイベントストリームが終わると、処理中と保留中のイベントも中止します。

ハンドラーが I/O の完了を `.await` している間は Gateway の受信を続けられます。同期 I/O や長い計算は、同じタスク内の他の処理も止めるため、Tokio の `spawn_blocking` などで切り分けます。

## `Commands` と `CommandContext<'_>`

| メソッド | 動作 |
| --- | --- |
| `Commands::new(prefix) -> Result<Commands>` | `!` などのプレフィックスを指定します。空文字列と空白を含む値はエラーです。 |
| `Commands::with_prefix_resolver(resolver) -> Commands` | 投稿を受け取る非同期関数が返す `CommandPrefixes` で判定します。候補の検証は `CommandPrefixes::new(&[&str])`、判定の無効化は `CommandPrefixes::default()` を使います。 |
| `commands.add_check(check)` | 登録済みテキストコマンドに適用する共通の `CommandCheck` を追加します。 |
| `Commands::with_prefixes(prefixes) -> Result<Commands>` | 複数のプレフィックスで同じコマンド一覧を使います。一致する候補が複数ある場合は最も長いものを選びます。 |
| `commands.register(name, handler) -> Result<()>` | プレフィックスを省いた名前で非同期関数を登録します。空の名前、空白がある名前、重複した名前はエラーです。 |
| `commands.register_with_aliases(name, aliases, handler) -> Result<()>` | 正式名と別名を一括で登録します。どの名前からも同じ関数を呼び、1 件でも名前が無効なら登録全体が失敗します。 |
| `commands.group(name)` / `commands.group_with_aliases(name, aliases)` | テキストコマンドを階層にまとめます。`TextCommandGroup` へサブコマンド、子グループ、グループ名だけで呼んだ場合の `fallback` を登録できます。 |
| `commands.dispatch(ctx).await -> Result<()>` | 該当するコマンドを実行し、ハンドラーが返したエラーを呼び出し元に返します。`Bot::run` の独自処理にも組み込めます。 |
| `ctx.command_name() -> &str` / `ctx.invoked_with() -> &str` | 登録した正式名と、ユーザーが入力した名前をそれぞれ取得します。 |
| `ctx.prefix() -> &str` | 実際に使われたプレフィックスを取得します。 |
| `ctx.qualified_name() -> &str` | グループ名を含む正式なコマンド経路を、空白で区切った文字列で返します。 |
| `ctx.args() -> &str` | コマンド名に続く本文を借ります。前後の空白を除去し、引数なしの場合は空文字列を返します。 |
| `ctx.arguments() -> Arguments<'_>` | 空白区切りで引数を順に読みます。引用符やかぎ括弧の中に空白を含められます。閉じ忘れなどは `ArgumentError` で返します。 |
| `ctx.context() -> &Context<'_>` | 元のメッセージを扱うコンテキストを借ります。 |
| `ctx.timing() -> Timing` | このテキストコマンド呼び出しの測定値を取得します。 |
| `ctx.reply(content).await -> Result<()>` | コマンドのメッセージに返信し、返信先の投稿者や本文中のメンションへの通知を抑止します。 |
| `ctx.reply_with(content, &mentions).await -> Result<()>` | 通知する相手を `AllowedMentions` で指定して返信します（`reply` は何も通知しません）。メッセージ返信の `Context` と `CommandContext` で使えます。 |
| `ctx.require_permissions(perms).await -> Result<()>` | 送り主がすべての権限を持つか確認します。足りないと `Error::Check(CheckFailure::MissingPermissions(不足分))` を返します。`ctx.permissions().await`、`ctx.require_guild()` も使えます。テキストコマンドでは Discord から取得して計算します。`Bot::cache_permissions(ttl)` で保存期間を指定でき、`Intents::GUILDS` を有効にするとロールやチャンネルの変更時に保存を破棄します。 |
| `ctx.reply_ui(&message).await -> Result<()>` | `UiMessage` を元のメッセージに返信します。 |
| `ctx.reply_v2(&message).await -> Result<()>` | `ComponentsV2` を元のメッセージに返信します。 |

名前は大文字と小文字を区別します。最初の空白までがコマンド名です。`! ping` のようにプレフィックスの直後が空白の投稿、未登録の名前、通常の投稿は無視されます。

`Bot::run_commands` で登録済みのコマンドの実行に失敗した場合は、エラーを `on_error` に報告します。元の投稿への返信は、既定では **しません**。返信したい場合は `Messages::text_command_failed` に文言を設定します。`commands.dispatch` を直接使う場合は、呼び出し元でエラーを受け取って返信やログを扱います。

## `SlashCommands` と `SlashContext<'_>`

`slash.register_user_menu(name, handler)` と `slash.register_message_menu(name, handler)` はユーザー・メッセージのコンテキストメニューにコマンドを登録します。ハンドラーへの第 2 引数は `UserMenuTarget<'_>` または `&model::channel::Message` です。どちらも `SlashContext` で実行者を確認し、応答できます。対象情報が Discord の通知にない場合は `Error::CommandTargetUnavailable` を報告します。

`slash.set_command_settings(kind, name, settings)` で登録済みのコマンドに `CommandSettings` を付けます。グローバルの利用場所とインストール先、翻訳名・翻訳説明、年齢制限を設定できます。`SlashContext::locale` と `guild_locale` で Discord が通知したロケールを取得できます。

| メソッド | 動作 |
| --- | --- |
| `SlashCommands::for_guild(guild_id) -> SlashCommands` | Discord のサーバー ID を指定して空のコマンド一覧を作ります。ID の型は `Id<GuildMarker>` です。 |
| `Registry::new(&mut commands, slash, &mut events).add(&module)?` | `Module` を実装した型の `register(&self, &mut Registry)` を呼び、テキストコマンド、スラッシュコマンド、イベントをまとめて登録します。 |
| `commands.autocomplete(path, option, handler) -> Result<()>` | 引数に、入力中の候補を返す関数を付けます。`path` はコマンド名（サブコマンドなら経路）、関数は `AutocompleteContext`（入力中の文字列は `focused()`、その引数の名前は `focused_name()`、候補を返すのは `respond(choices)`）を受け取ります。候補は `SlashCommands::{string_choice, integer_choice, number_choice}` で作ります。 |
| `commands.group(name, description) -> Result<CommandGroup>` | サブコマンドをまとめるコマンドを作ります。`group.subcommand(name, description, options, handler)` と、`group.group(name, description)`（さらに 1 段）で登録します。 |
| `SlashCommands::global() -> SlashCommands` | アプリ全体（すべてのサーバーと DM）向けに空のコマンド一覧を作ります。Discord 側の反映に時間がかかることがあります。 |
| `commands.replace_commands() -> SlashCommands` | 起動時に、このアプリの同じ範囲（サーバー、またはグローバル）の全コマンドを、登録した一覧に一括で置き換えます。 |
| `commands.register(name, description, options, handler) -> Result<()>` | 表示名、説明、引数の定義、呼び出す非同期関数を登録します。Discord の名前や説明の制約、重複名を検証します。 |
| `SlashCommands::string_option(name, description, required) -> CommandOption` | 文字列引数を定義します。`required` が `true` なら Discord の入力欄で必須になります。 |
| `ctx.option(name) -> Option<&CommandOptionValue>` | 指定した名前の引数を Twilight の値として借ります。 |
| `ctx.string(name) -> Option<&str>` | 指定した名前の文字列を借ります。引数がない場合や他の型の場合は `None` を返します。 |
| `ctx.author_id() -> Option<Id<UserMarker>>` | 呼び出したユーザーの ID を取得します。 |
| `ctx.channel_id() -> Option<Id<ChannelMarker>>` | コマンドが実行されたチャンネルの ID を取得します。 |
| `ctx.timing() -> Timing` | このスラッシュコマンド呼び出しの測定値を取得します。 |
| `ctx.defer().await -> Result<()>` | チャンネルに表示される処理中の初回応答を送ります。 |
| `ctx.defer_ephemeral().await -> Result<()>` | 呼び出したユーザーだけに表示される処理中の初回応答を送ります。 |
| `ctx.reply(content).await -> Result<()>` | チャンネルに初回応答を送ります。`defer` または `defer_ephemeral` の後は、公開範囲を維持して処理中の表示を本文に置き換えます。 |
| `ctx.reply_ui(&message).await -> Result<()>` | `UiMessage` を初回応答として返します。遅延応答の後は、公開範囲を維持して元の応答を編集します。 |
| `ctx.reply_ui_ephemeral(&message).await -> Result<()>` | `UiMessage` を本人にだけ見える初回応答として返します。`defer_ephemeral` の後は、処理中の表示を置き換えます。 |
| `ctx.reply_v2(&message).await -> Result<()>` | `ComponentsV2` を初回応答として返します。遅延応答の後は、公開範囲を維持して元の応答を編集します。 |
| `ctx.reply_v2_ephemeral(&message).await -> Result<()>` | `ComponentsV2` を本人にだけ見える初回応答として返します。`defer_ephemeral` の後は、処理中の表示を置き換えます。 |
| `ctx.show_modal(&form).await -> Result<()>` | コマンドの初回応答として入力フォームを開きます。 |
| `ctx.reply_ephemeral(content).await -> Result<()>` | 呼び出したユーザーだけに初回応答を送ります。`defer_ephemeral` の後は、その処理中の表示を本文に置き換えます。 |
| `ctx.followup(content).await -> Result<()>` | 最初の本文を送った後に、チャンネルに追加のメッセージを送ります。 |
| `ctx.followup_ephemeral(content).await -> Result<()>` | 最初の本文を送った後に、呼び出したユーザーだけに追加のメッセージを送ります。 |
| `ctx.followup_files(content, files, limits).await -> Result<()>` | 元の応答を完成させた後、添付付きの追加メッセージをチャンネルへ送ります。 |
| `ctx.followup_files_ephemeral(content, files, limits).await -> Result<()>` | 元の応答を完成させた後、添付付きの追加メッセージを呼び出したユーザーだけに送ります。 |
| `ctx.original_response().await -> Result<Message>` | 元の応答メッセージを取得します。非公開の応答も取得できます。 |
| `ctx.edit_original_response(content)` / `ctx.edit_original_ui(&message)` / `ctx.edit_original_v2(&message)` | 元の応答を編集します。遅延応答を完成させることもでき、いずれも公開範囲と添付を維持します。 |
| `ctx.delete_original_response().await -> Result<()>` | 元の応答メッセージを削除します。遅延応答だけを削除した場合は、追加応答を送れません。 |

### 特定の権限を持つ人にコマンドを表示する

`SlashCommands::for_guild` で `register_with_permissions` に `Permissions::MANAGE_GUILD` などを渡すと、Discord のコマンド一覧での既定の表示条件と、Akari の実行前チェックに同じ権限を使います。管理者専用にする場合は `Permissions::ADMINISTRATOR` を渡します。サーバー側で一覧の表示条件が上書きされても、ハンドラーに処理を渡す前の権限チェックは残ります。

サブコマンドをまとめて制限するには `group_with_permissions` を使います。最上位の表示条件に加え、配下のサブコマンドと入力補完にも同じ権限を適用します。実行時の拒否は `Messages::check_failed` で通知され、入力補完では空の候補を返します。

表示条件だけを設定し、実行時の確認を `ctx.require_permissions(..)?` で自分で行う場合は `default_member_permissions` を使います。権限付きとして登録したコマンドの表示条件は、登録時の指定で決まります。[Discord のコマンド権限](https://docs.discord.com/developers/docs/interactions/slash-commands#permissions)

### サーバーに登録するコマンドを同期する

既定の起動では、同名のスラッシュコマンドを追加または更新し、ほかの既存コマンドを保持します。Akari に登録していないサーバー限定のスラッシュコマンドを受信した場合は、`on_error` に報告し、`Messages::slash_command_unavailable`（既定は「このコマンドは現在利用できません。」）を本人にだけ返します。

`replace_commands()` を指定すると、登録一覧に含まれないコマンドが対象サーバーから削除されます。削除の範囲は同じアプリ・同じサーバーの全アプリケーションコマンドで、ユーザーやメッセージを右クリックして実行するコマンドも対象です。空の一覧で一括同期すると対象の全コマンドが削除されます。[Discord のサーバー単位の一括上書き](https://docs.discord.com/developers/interactions/application-commands#bulk-overwrite-guild-application-commands)

### 時間がかかる処理に応答する

関数の形は `async fn handler(ctx: &mut SlashContext<'_>) -> Result<()>` です。Discord は Interaction の受信後 3 秒以内に初回応答を求めます。外部サービスへの問い合わせなど、期限に間に合わない可能性がある処理の前に、ハンドラーから `ctx.defer().await?` または `ctx.defer_ephemeral().await?` を明示的に呼びます。処理後に `ctx.reply(...).await?` で処理中の表示を本文に置き換えます。公開範囲は `defer` または `defer_ephemeral` の時点で決まります。公開の `defer` の後に `reply_ephemeral` を呼ぶとエラーになります。`followup` と `followup_ephemeral` は最初の本文を送った後に呼びます。Discord の Interaction トークンの有効期間は 15 分です。[Discord の Interaction 応答仕様](https://docs.discord.com/developers/interactions/receiving-and-responding)

公開の `defer` の後に `reply_ui_ephemeral` を呼ぶと、公開範囲を変更できないためエラーになります。ハンドラーが最初の本文を送れずに終了した場合は、Akari がエラー応答を試みます。初回応答が済んでいない場合は本人にだけ表示し、処理中の応答がある場合はその表示をエラー文に置き換えます。返信本文中のメンション通知は抑止します。

権限チェックに失敗した場合の `Messages::check_failed` は、公開の `defer()` を済ませていても本人にだけ送ります。この場合、公開の待機表示は固定文言「処理を終了しました。」に変わります。待機表示も非公開にするには `defer_ephemeral()` を使います。

## `register_hybrid` と `HybridContext<'_>`

`register_typed_hybrid` は、定義した `CommandOption` を使ってテキストとスラッシュの両方を検証し、ハンドラーの第 2 引数に `HybridArguments<'_>` を渡します。`string`、`integer`、`number`、`boolean`、`user_id`、`role_id`、`channel_id` で値を取得できます。テキストでは定義順に入力し、省略可能な引数は末尾に置きます。必須引数の不足や型・範囲の違反は `Error::HybridArgument` になります。

`register_hybrid_group` は `HybridCommandGroup` に登録した経路をテキストとスラッシュの両方に追加します。いずれかの定義でエラーがあれば、グループの追加全体を中止します。`fallback(slash_name, description, handler)` は、テキストではグループ名だけ、スラッシュでは指定したサブコマンド名で呼び出せます。

`register_hybrid(&mut commands, slash, name, description, options, handler)` は、テキストコマンドとスラッシュコマンドに同じ関数を登録します。二つ目の引数に `Some(&mut slash_commands)` を渡すと両方に登録し、`None` を渡すとテキストコマンドだけに登録します。`options` はスラッシュ側の入力欄の定義です。登録先に同名コマンドがある場合や、定義が登録先の形式に合わない場合は、追加前にエラーを返します。

ハンドラーは `async fn handler(ctx: &mut HybridContext<'_>) -> Result<()>` の形で定義します。

| メソッド | 動作 |
| --- | --- |
| `ctx.source() -> CommandSource` | テキストから呼ばれた場合は `CommandSource::Text`、スラッシュからなら `CommandSource::Slash` を返します。 |
| `ctx.author_id() -> Option<Id<UserMarker>>` | 呼び出したユーザーの ID を返します。取得できない場合は `None` を返します。 |
| `ctx.args() -> Option<&str>` | テキスト側ではコマンド名に続く本文を `Some` で返します。スラッシュ側では `None` を返します。 |
| `ctx.option(name) -> Option<&CommandOptionValue>` | スラッシュ側の名前付き引数を借ります。テキスト側や該当する引数がない場合は `None` を返します。 |
| `ctx.string(name) -> Option<&str>` | スラッシュ側の文字列引数を借ります。テキスト側、引数なし、他の型の場合は `None` を返します。 |
| `ctx.text_arg(name) -> Option<&str>` | スラッシュでは同名の文字列引数、テキストでは最初に定義したトップレベルの文字列引数に対応する残りの本文を借ります。値が空か空白だけの場合は `None` です。 |
| `ctx.check_cooldown(&cooldown).await -> Result<bool>` | 実行できる場合はユーザーごとの利用を記録して `true` を返します。待ち時間が残っている場合はその時間を知らせ、`false` を返します。スラッシュでの待ち時間の通知は本人にだけ表示します。 |
| `ctx.timing() -> Timing` | 呼び出し元に応じた測定値を取得します。 |
| `ctx.reply(content).await -> Result<()>` | テキストなら元の投稿に返信し、スラッシュなら公開の初回応答を送ります。 |
| `ctx.reply_ui(&message).await -> Result<()>` | `UiMessage` をテキストコマンドまたはスラッシュコマンドへの返信に使います。 |
| `ctx.reply_v2(&message).await -> Result<()>` | `ComponentsV2` をテキストコマンドまたはスラッシュコマンドへの返信に使います。 |

テキスト側の `text_arg` は、コマンド名の後ろ全体を最初のトップレベルの文字列引数に割り当てます。たとえば `count`（整数）と `text`（文字列）を定義した場合、`!cmd 3 hello` の `text_arg("text")` は `3 hello` 全体を返します。`/cmd count:3 text:hello` では `option("count")` が整数の `3` を返し、`text_arg("text")` は `hello` を返します。テキスト側で整数などの型付き引数が必要な場合は、ハンドラーで `args()` を分割・変換します。

クールダウンは応答や重い処理の前に確認します。`if !ctx.check_cooldown(&cooldown).await? { return Ok(()); }` とすると、残り時間を返信した場合にハンドラーを終了できます。ユーザー ID を取得できない場合や返信に失敗した場合はエラーを返します。

## `Cooldown`

`Cooldown::new(interval)` に `std::time::Duration` を渡すと、ユーザーごとの実行間隔を管理できます。`check(user_id)` が `Ok(())` を返した時点で実行が記録され、同じユーザーが間隔を空けずに呼ぶと `Err(残り時間)` が返ります。拒否された呼び出しは待ち時間を延ばしません。

状態は Bot プロセス内に保存されます。別のコマンドや呼び出し方に個別の制限を設けるには、別の `Cooldown` を作ります。テキストとスラッシュで同じ `Cooldown` を使うと、待ち時間も共有できます。[Playground の適用例](tutorial.md)

`Cooldown::<K>::per_key(rate, interval)` は、同じキー `K` で最初に通過してから `interval` の間に `rate` 回まで通します。`rate` には `NonZeroU32` を指定します。`check(key)` で判定し、チャンネル ID や複数の ID のタプルをキーにできます。すべての呼び出しで制限を共有する場合は `Cooldown::<()>` に `()` を渡します。

### 同時に実行できる件数を制限する

`ConcurrencyLimit::<K>::new(maximum)` で同じキーあたりの実行枠を作ります。上限は `NonZeroU16` です。`try_acquire(key)` は待たずに枠を取り、満杯時は `None` を返します。`acquire(key).await` は同じキーの待機列に入ります。取得した `ConcurrencyPermit` は処理が終わるまで変数で保持し、スコープを抜けると枠が戻ります。

## `Timing`

| メソッド | 測定範囲 |
| --- | --- |
| `gateway_rtt() -> Option<Duration>` | イベントを受信した時点で確認できた、Bot と Discord Gateway 間の直近のハートビートを送ってから Discord の応答を受け取るまでの往復時間です。応答が得られるまでは `None` です。 |
| `queue_wait() -> Duration` | Akari がイベントを受信してから、処理する側が内部のキューから取り出すまでの時間です。 |
| `handler_elapsed() -> Duration` | このコマンドの処理を始めてから、経過時間を取得するまでの時間です。 |

`Duration` は Rust 標準ライブラリの時間の長さを表す型です。サンプルは各値をミリ秒で表示し、処理時間は返信を送る直前に取得します。

## `Events`

| メソッド | 動作 |
| --- | --- |
| `Events::new() -> Events` | 空のイベントハンドラー一覧を作ります。 |
| `events.on_ready(handler)` | Gateway が新規セッションを確立したときに受け取る `&Ready` を渡します。 |
| `events.on_message(handler)` | 人が送った新着メッセージの `&Message` を渡します。通常の投稿とテキストコマンドの両方を受け取ります。 |
| `events.on_member_update(handler)` / `on_member_remove(handler)` | 保存済みのメンバーの更新前後・退出前を `CachedMemberUpdate` / `CachedMemberRemove` として受け取ります。 |
| `events.on_message_edit(handler)` | 保存済みのメッセージの更新前後を `&CachedMessageEdit` で渡します。 |
| `events.on_message_delete(handler)` / `events.on_bulk_message_delete(handler)` | 保存済みの単独削除を `&Message`、保存済みの一括削除を `&[Arc<Message>]` で渡します。 |
| `events.on_raw_message_edit(handler)` | 編集や Embed の更新など、Discord が送った部分更新を `&MessageUpdate` で渡します。 |
| `events.on_raw_message_delete(handler)` | 単独削除のチャンネル・メッセージ ID を `&MessageDelete` で渡します。 |
| `events.on_raw_bulk_message_delete(handler)` | サーバーでの一括削除を `&MessageDeleteBulk` で渡します。`ids` で削除された ID 一覧を取得できます。 |
| `events.on_raw_reaction_add(handler)` / `events.on_raw_reaction_remove(handler)` | リアクションの追加と単独削除を `&ReactionAdd` / `&ReactionRemove` で渡します。 |
| `events.on_raw_reaction_clear(handler)` / `events.on_raw_reaction_clear_emoji(handler)` | すべてのリアクションの削除、または指定した絵文字のリアクションの削除を `&ReactionRemoveAll` / `&ReactionRemoveEmoji` で渡します。 |
| `events.on_raw_poll_vote_add(handler)` / `events.on_raw_poll_vote_remove(handler)` | 投票の追加・取り消しを `&MessagePollVoteAdd` / `&MessagePollVoteRemove` で渡します。 |
| `events.on_raw_typing(handler)` | 入力開始を `&TypingStart` で渡します。 |
| `events.on_gateway(handler)` | ハンドラーが受け取る `GatewayPayload` 型に応じて、サーバー、チャンネル、ロール、メンバー、スレッド、ピン、プレゼンス、音声状態の通知を受け取ります。必要な Intent は型と受信範囲によって異なります。 |
| `events.on_button(custom_id, handler) -> Result<()>` | 指定した識別子に完全一致するボタンが押されたときに `&mut ButtonContext<'_>` を渡します。識別子は 1～100 文字で、Bot の登録一覧全体で、同じ識別子に関数を一つ登録できます。 |
| `events.on_select(custom_id, handler) -> Result<()>` | 指定した識別子の文字列選択メニューが操作されたときに `&mut SelectContext<'_>` を渡します。 |
| `events.on_user_select(custom_id, handler) -> Result<()>` | ユーザー選択時に `&mut UserSelectContext<'_>` を渡します。 |
| `events.on_role_select(custom_id, handler) -> Result<()>` | ロール選択時に `&mut RoleSelectContext<'_>` を渡します。 |
| `events.on_channel_select(custom_id, handler) -> Result<()>` | チャンネル選択時に `&mut ChannelSelectContext<'_>` を渡します。 |
| `events.on_mentionable_select(custom_id, handler) -> Result<()>` | ユーザーとロールを候補に持つメニューの選択時に `&mut MentionableSelectContext<'_>` を渡します。 |
| `events.on_dynamic_button(namespace, handler)` / `on_dynamic_select` / `on_dynamic_modal` | 機能名で操作を登録し、`ctx.dynamic_id()` で対象の識別子を取得します。ユーザー・ロール・チャンネル・メンション選択にも対応します。 |
| `events.on_modal(custom_id, handler) -> Result<()>` | 指定した識別子のフォームが送信されたときに `&mut ModalContext<'_>` を渡します。 |

`on_ready`、`on_message`、メッセージの raw イベントには複数の関数を登録できます。各関数が返したエラーは `on_error` に報告され、次のハンドラーへ進みます。コマンドとイベントの順序は [ハンドラーの実行順](#ハンドラーの実行順) を参照してください。

メッセージの raw イベントは受信順に処理し、新着メッセージと同じキューを使います。以前のメッセージを受信していなくても届き、Bot や Webhook の投稿にも発生します。編集前や削除時の内容を使いたい場合は `cache_messages` とキャッシュ付きイベントを使います。部分更新の `content` が `None` の場合、本文が変更されたかどうかはその通知だけでは判断できません。

キャッシュ付きのメッセージイベントを登録した場合、`cache_messages` に正の上限と、対象に合うメッセージ Intent を設定します。一括削除には `GUILD_MESSAGES` が必要です。

リアクションは `GUILD_MESSAGE_REACTIONS` / `DIRECT_MESSAGE_REACTIONS`、投票は `GUILD_MESSAGE_POLLS` / `DIRECT_MESSAGE_POLLS`、入力開始は `GUILD_MESSAGE_TYPING` / `DIRECT_MESSAGE_TYPING` を、受信する範囲に応じて指定します。

管理、予定、Webhook、招待、Entitlement、Subscription などの型付き通知も `Events::on_gateway` で登録できます。ペイロードの型は `model::gateway::payload::incoming` にあります。

## `UiMessage` と `ButtonContext<'_>`

`ui::DynamicId::new(namespace, value)` で動的ルート用の `custom_id` を組み立て、ボタン・セレクト・モーダルの既存ビルダーに渡します。通常の `on_*` に完全一致する ID が登録済みなら、そのハンドラーを呼びます。`on_dynamic_*` のハンドラーは `ctx.dynamic_id()` で機能名と値を確認できます。以前のメッセージを処理するために、起動時に同じ機能名を登録してください。

`disnana_akari::ui::{UiMessage, Embed, Button}` を使って、本文・埋め込み・ボタンを組み立てます。完成した値を `ctx.reply_ui(&message).await` に渡します。

| ビルダー | 動作 |
| --- | --- |
| `UiMessage::new().content(text)` | メッセージ本文を指定します。 |
| `message.embed(embed)` | 埋め込みを一つ追加します。 |
| `message.buttons(vec![button])` | ボタンを一行追加します。一行は最大五つで、メッセージには最大五行を追加できます。 |
| `message.select(select)` | 文字列選択メニューを一行追加します。 |
| `Embed::new(title).description(text).color(rgb)` | 埋め込みの見出しと説明、`0x5865F2` のような RGB の色を指定します。 |
| `embed.field(name, value, inline)` | 項目を一つ追加します。`inline` が `true` の項目は、表示幅に応じて横に並びます。 |
| `Button::primary(custom_id, label)` | Discord に表示するラベルと、クリック時に使う識別子を指定します。 |
| `Button::secondary(custom_id, label)` | 控えめに表示する操作ボタンを作ります。 |
| `Button::success(custom_id, label)` | 確認や完了などに使う緑色の操作ボタンを作ります。 |
| `Button::danger(custom_id, label)` | 削除などに使う赤色の操作ボタンを作ります。 |
| `Button::link(url, label)` | クリックで URL を開くボタンを作ります。Discord からクリックのイベントは届きません。 |
| `button.emoji(emoji)` | `model::channel::message::EmojiReactionType` で Unicode 絵文字やカスタム絵文字を付けます。 |
| `button.disabled(true)` | 押せない状態でボタンを表示します。 |

本文は最大 2,000 文字、埋め込みは最大 10 個で合計 6,000 文字までです。一つのメッセージ内で同じボタン識別子を複数使うと、送信時にエラーが返ります。テキストコマンドの返信に埋め込みを付ける場合は、Bot のチャンネル権限 **Embed Links** を確認します。[Discord のチャンネル権限](https://docs.discord.com/developers/topics/permissions#permissions-bitwise-permission-flags)

操作ボタン、リンクボタン、絵文字は `UiMessage::buttons` と `ComponentsV2::buttons` の両方で使えます。リンクボタンには 512 文字以内の絶対 HTTP または HTTPS URL を指定します。URL はメッセージの送信時に検証します。

`Events::on_button` で登録した関数は、サーバーやメッセージを問わず `custom_id` が完全一致すると呼ばれます。メッセージを閲覧できるユーザーがボタンを押せるため、投稿者や管理者に限定する操作では `guild_id()` と `message()` で対象を確認し、`author_id()` で操作者を特定して権限を確認します。

| メソッド | 動作 |
| --- | --- |
| `ctx.custom_id() -> &str` | ボタンの識別子を取得します。 |
| `ctx.author_id() -> Option<Id<UserMarker>>` | ボタンを押したユーザーの ID を取得します。 |
| `ctx.invoker_id() -> Option<Id<UserMarker>>` | 元のメッセージの操作メタデータに記録された実行者 ID を取得します。記録がなく、Bot の返信が参照したユーザー投稿を照合できる場合は、その投稿者 ID を返します。どちらも確認できない場合は `None` です。 |
| `ctx.resolve_invoker_id().await -> Result<Option<Id<UserMarker>>>` | 返信先の投稿が操作データに含まれていない場合、同じチャンネルの返信先を Discord REST API から取得して照合します。 |
| `ctx.guild_id() -> Option<Id<GuildMarker>>` | ボタンが押されたサーバーの ID を取得します。 |
| `ctx.message() -> Option<&Message>` | ボタンが付いた元のメッセージを借ります。 |
| `ctx.timing() -> Timing` | ボタン操作の受信時点と処理開始時点をもとにした測定値を取得します。 |
| `ctx.reply_ephemeral(content).await -> Result<()>` | ボタンを押した本人にだけ見える初回応答を送ります。 |
| `ctx.update_message(&message).await -> Result<()>` | ボタンの元メッセージを `UiMessage` で更新します。その場で初回応答として更新するか、`defer_update` 後に更新できます。 |
| `ctx.update_v2(&message).await -> Result<()>` | Components V2 の元メッセージを `ComponentsV2` で更新します。 |
| `ctx.show_modal(&form).await -> Result<()>` | このクリックの初回応答として入力フォームを開きます。 |
| `ctx.defer_update().await -> Result<()>` | 追加メッセージを表示せず、操作を先に受け付けます。処理後に `update_message` を呼べます。 |
| `ctx.acknowledge().await -> Result<()>` | `defer_update` と同じ確認応答を送ります。 |
| `ctx.followup(content).await -> Result<()>` | 初回応答の後、チャンネルに追加のメッセージを送ります。 |
| `ctx.followup_ephemeral(content).await -> Result<()>` | 初回応答の後、ボタンを押した本人にだけ追加のメッセージを送ります。 |
| `ctx.followup_files(content, files, limits).await -> Result<()>` | 初回応答や `defer_update` の後、添付付きの追加メッセージをチャンネルへ送ります。 |
| `ctx.followup_files_ephemeral(content, files, limits).await -> Result<()>` | 初回応答や `defer_update` の後、添付付きの追加メッセージをボタンを押した本人だけに送ります。 |

`update_message` は本文、埋め込み、ボタンの行をまとめて置き換えます。次の表示にも残したい埋め込みやボタンは、新しい `UiMessage` に含めます。更新は一回のクリックにつき一度です。時間のかかる処理では、先に `defer_update` を呼んで 3 秒以内に初回応答を送り、処理の完了後に `update_message` で表示を変更します。

発行者本人に操作を限定する場合は、`invoker_id()` と現在の `author_id()` をハンドラーで照合します。両方の ID が取得できた場合に比較し、操作の対象や必要な権限も確認します。選択メニューも同じ方法で確認できます。

テキストコマンドへの返信では、Discord が返信先の投稿を操作データから省略する場合があります。`resolve_invoker_id().await` を使う場合は、先に `defer_update().await?` で操作を受け付けておくと、REST の完了を待ってから結果を返せます。返信先が削除されていれば `Ok(None)` を返し、履歴の閲覧権限不足などで取得が失敗した場合は `Err` を返します。[Discord のメッセージ仕様](https://docs.discord.com/developers/resources/message#message-object)

```rust,no_run
use disnana_akari::{ButtonContext, Result};

async fn confirm(ctx: &mut ButtonContext<'_>) -> Result<()> {
    // Discord の初回応答期限内に操作を受け付けます。
    ctx.defer_update().await?;

    // 操作者と元の投稿者の ID が両方判明した場合だけ比較します。
    let owner = ctx.resolve_invoker_id().await?;
    if !matches!((ctx.author_id(), owner), (Some(author), Some(owner)) if author == owner) {
        return ctx.followup_ephemeral("元の投稿者だけが操作できます。").await;
    }

    ctx.followup_ephemeral("操作を受け付けました。").await
}
```

ハンドラーが初回応答を送らずに成功した場合は、Akari が確認応答を試みます。初回応答の前にエラーを返した場合は、本人にだけ見える初回応答を試みます。`defer_update` 後にハンドラーが失敗した場合は、元のメッセージを保ったまま、本人にだけ追加でエラーを知らせます。[埋め込みとボタンのチュートリアル](tutorial.md)

## `ComponentsV2` と入力フォーム

`disnana_akari::ui::ComponentsV2` は、`text`、`separator`、`buttons`、`select` などでメッセージを組み立てます。`reply_v2` は送信時に Discord の `IS_COMPONENTS_V2` フラグを付けます。Components V2 の文章は `text(...)` で追加し、従来の本文や埋め込みを付ける場合は `UiMessage` を使います。既存の Components V2 メッセージを更新する場合は `update_v2` を使います。この更新は初回応答として送るか、`defer_update` の後に送れます。[Discord の Components V2](https://docs.discord.com/developers/components/overview)

| ビルダー | 動作 |
| --- | --- |
| `v2.section_with_button(texts, button)` / `v2.section_with_thumbnail(texts, media)` | 1～3 個のテキスト部品と、その横に表示するボタンまたはサムネイルを追加します。 |
| `v2.media_gallery(items)` / `Media::new(url)` | 1～10 件のメディアを並べます。`description` で代替テキスト、`spoiler` で初期表示を設定します。 |
| `SelectOption::new(label, value)` | メニューに表示するラベルと、選択時に返す値を設定します。`description` で選択肢の説明を設定できます。 |
| `StringSelect::new(custom_id, options)` | 選択メニューを作ります。`placeholder` で選択する前の表示、`min_values` と `max_values` で選べる個数、`disabled` で無効状態を指定できます。 |
| `Modal::new(custom_id, title)` | 入力フォームを作ります。 |
| `form.text_input(TextInput::new(custom_id, label))` | フォームに入力欄を追加します。`paragraph` で複数行入力、`required` で必須かどうか、`min_length` と `max_length` で文字数、`value` で初期値を設定できます。 |
| `form.file_upload(FileUpload::new(custom_id, label))` | フォームにファイル添付欄を追加します。`description`、`min_values`、`max_values`、`required` を指定できます。 |
| `form.checkbox(Checkbox::new(custom_id, label))` | 単一のチェックボックスを追加します。`description`、`selected_by_default` を指定できます。 |
| `form.select(label, menu, required)` | 文字列セレクトを追加します。対象の型に応じて `user_select`、`role_select`、`channel_select`、`mentionable_select` も選べます。 |
| `form.checkbox_group(CheckboxGroup::new(custom_id, label, options))` | 複数選択できるチェックボックスを追加します。選択結果は `ctx.checkbox_group(custom_id)` で借ります。 |
| `form.radio_group(RadioGroup::new(custom_id, label, options))` | 2〜10 個から 1 個選ぶ入力欄を追加します。`ctx.radio_group(custom_id)` は値、未選択、欄なしを区別します。 |

選択メニューの既定値は一項目です。最小選択数は 0～25、最大選択数は 1～25 で、最小値は最大値以下、最大値は用意した選択肢の数以下にします。入力文字数の最小値は 0～4,000、最大値は 1～4,000 です。空でない初期値は設定した文字数の範囲に収めます。入力欄を空で始めたい場合は、初期値を省略するか空文字列にします。

文字列の選択肢は `SelectOption::selected_by_default(true)`、ユーザーなどの対象選択は `default_values(ids)` で初期選択を指定できます。`MentionableSelect` の初期選択には `Mentionable::User(id)` または `Mentionable::Role(id)` を使います。

`SelectContext` には、メニューの `custom_id()` と選択結果の `values() -> &[String]` が渡ります。`author_id()`、`guild_id()`、`message()`、`invoker_id()` で操作者や元のメッセージを確認でき、ボタンと同じように `reply_ephemeral`、`defer_update`、`update_message`、`update_v2`、`show_modal`、`followup_files`、`followup_files_ephemeral` を呼べます。

### ユーザー、ロール、チャンネルを選択する

候補を Discord が用意するメニューは `UserSelect::new(custom_id)`、`RoleSelect::new(custom_id)`、`ChannelSelect::new(custom_id)` で作ります。`UiMessage` と `ComponentsV2` に `user_select`、`role_select`、`channel_select` でそれぞれ追加できます。各ビルダーは `placeholder`、`min_values`、`max_values`、`disabled` を持ちます。`ChannelSelect::channel_types(Vec<ChannelType>)` でチャンネルの種類を絞れます。

同じメニューからユーザーとロールを選ぶ場合は、`MentionableSelect::new(custom_id)` で作り、`UiMessage` または `ComponentsV2` に `mentionable_select` で追加します。選択した項目は Discord が送った順で `Mentionable::User(Id<UserMarker>)` または `Mentionable::Role(Id<RoleMarker>)` として受け取ります。

| コンテキスト | `values()` の型 |
| --- | --- |
| `UserSelectContext<'_>` | `&[Id<UserMarker>]` |
| `RoleSelectContext<'_>` | `&[Id<RoleMarker>]` |
| `ChannelSelectContext<'_>` | `&[Id<ChannelMarker>]` |
| `MentionableSelectContext<'_>` | `&[Mentionable]` |

`UserSelectContext`、`RoleSelectContext`、`ChannelSelectContext` は、共通の `EntitySelectContext<'_, M>` の別名で、選択対象の種類（`M` は `UserMarker` など）だけが違います。メソッドは共通です。

`values()` で対象ごとに型の付いた ID を取得できます。Discord が選択対象の情報を同梱した場合は、`resolved()` で `&InteractionDataResolved` を取得できます。操作への本人宛ての返信には `reply_ephemeral` を使います。[Playground の選択メニュー実装](../examples/ping/entities.rs)

ユーザーとロールを混ぜたメニューでは、選択された各 ID の種類を Discord の `resolved.users` と `resolved.roles` で判定します。選択があるときに `resolved` が欠けている場合や、ID の種類が一意に決まらない場合は、ハンドラーを呼ばず本人へのエラー応答を試みます。選択数が 0 件なら、`resolved` がなくても空の `values()` を受け取れます。

### 入力フォームを開く

`show_modal` はスラッシュコマンド、ボタン、選択メニューの初回応答として呼びます。フォームを送信すると、フォームの識別子に対応する `on_modal` の関数が呼ばれます。`ModalContext::text_input(custom_id)` は、入力欄の識別子に対応する値を `Option<&str>` で返します。`custom_id()` はフォームの識別子を返し、`author_id()` と `guild_id()` は送信者とサーバーの情報を返します。

初回返信か defer の後に、`original_response` でフォーム送信への元の応答を取得できます。`edit_original_response`、`edit_original_ui`、`edit_original_v2` で元の応答を編集し、`delete_original_response` で削除できます。

添付ファイルは `ctx.file_upload(custom_id)` で型付き ID を受け取り、`ctx.attachment(id)` で Discord が送ったファイル名やサイズなどのメタデータを取得します。ファイル本体のダウンロードはアプリから明示的に行います。

`ctx.checkbox(custom_id)` はチェックされた欄を `Some(true)`、チェックされていない欄を `Some(false)` で返します。欄がない場合は `None` です。

セレクトの選択結果は `ctx.string_select(custom_id)` や、対象の型に対応するメソッドで取得できます。`None` は欄がないか型が違うことを示します。選択がなかった欄は空の選択結果を返します。`mentionable_select` は選択をユーザーまたはロールに解決できない場合にエラーを返します。

### フォーム送信に応答する

| メソッド | 動作 |
| --- | --- |
| `ctx.reply(content).await -> Result<()>` | チャンネルに返信します。遅延応答の後は、公開範囲を維持して処理中の表示を置き換えます。 |
| `ctx.reply_ephemeral(content).await -> Result<()>` | 本人にだけ見える初回応答を送ります。`defer_ephemeral` の後は、処理中の表示を置き換えます。 |
| `ctx.defer().await -> Result<()>` | 最初に処理中の応答をチャンネルに送ります。 |
| `ctx.defer_ephemeral().await -> Result<()>` | 最初に本人にだけ見える処理中の応答を送ります。 |
| `ctx.followup(content).await -> Result<()>` | 最初の本文を送った後、チャンネルに追加のメッセージを送ります。 |
| `ctx.followup_ephemeral(content).await -> Result<()>` | 最初の本文を送った後、本人にだけ追加のメッセージを送ります。 |
| `ctx.followup_files(content, files, limits).await -> Result<()>` | フォーム送信への元の応答を完成させた後、添付付きの追加メッセージをチャンネルへ送ります。 |
| `ctx.followup_files_ephemeral(content, files, limits).await -> Result<()>` | フォーム送信への元の応答を完成させた後、添付付きの追加メッセージを本人だけに送ります。 |

時間がかかる処理では、3 秒の初回応答期限までに `defer` または `defer_ephemeral` を明示的に呼び、完了時に `reply` を呼びます。公開の `defer` の後に `reply_ephemeral` を呼ぶとエラーになります。ハンドラーが本文を送らずに成功した場合は、Akari が `Messages::modal_accepted`（既定は「入力を受け付けました。」）を送ります。初回応答前なら本人にだけ表示し、遅延応答済みならその表示を置き換えます。本文を送る前にハンドラーが失敗した場合は、`Messages::modal_failed` を同じ経路で送ります。どちらの文言も `None` にすると、Akari は何も送りません。その場合、応答が済んでいない操作は、Discord 上で失敗と表示されます。[Components V2 のチュートリアル](tutorial.md) · [Discord の Interaction 応答仕様](https://docs.discord.com/developers/interactions/receiving-and-responding)

```rust,no_run
use disnana_akari::{ModalContext, Result};

async fn submit(ctx: &mut ModalContext<'_>) -> Result<()> {
    ctx.defer_ephemeral().await?;
    // ここでフォーム送信後の処理を待ちます。
    ctx.reply("入力を受け付けました。").await?;
    ctx.followup_ephemeral("受け取り番号は 1234 です。").await
}
```

## 実行時の情報

`Bot::on_info` を設定すると、`InfoEvent` を同期的に受け取れます。既定では情報の通知はありません。関数の中では短い処理を行い、必要ならログや計測システムに渡してください。

| バリアント | 通知されるとき |
| --- | --- |
| `GatewayReady` | Gateway で新しいセッションが準備できたとき |
| `GatewayResumed` | 既存の Gateway セッションを再開したとき |
| `SlashCommandsSynced` | Discord への同期が成功したとき。サーバー ID（グローバルは `None`）と今回同期したコマンド数を渡します |
| `InteractionResponded` | Akari 経由の初回応答を Discord が受け付けたとき。遅延応答、フォーム表示、混雑時の初回返信も含みます |

`InteractionResponded` の `elapsed` は、Akari が Gateway イベントを受信してから REST 応答を受け取るまでの時間です。`rest_elapsed` はそのうち初回応答の REST リクエストにかかった時間で、レート制限による待ち時間を含みます。後続の更新や追加メッセージ、`ctx.http()` から直接送った応答は計測の対象外です。

```rust
use disnana_akari::{Bot, InfoEvent, Result};

fn build() -> Result<Bot> {
    Ok(Bot::from_env()?.on_info(|event| {
        if let InfoEvent::InteractionResponded { elapsed, rest_elapsed, .. } = event {
            eprintln!("初回応答 {elapsed:?}（REST {rest_elapsed:?}）");
        }
    }))
}
```

## エラーと自動で送る文言

### エラーを受け取る

`Bot::on_error` に関数を渡すと、Akari の内部で起きたエラーを一件ずつ受け取れます。

```rust,no_run
use disnana_akari::{Bot, Result};

fn build() -> Result<Bot> {
    Ok(Bot::from_env()?.on_error(|report| {
        // report.source: エラーが起きた場所、report.name: コマンド名や custom_id、report.error: 内容
        eprintln!("[bot] {report}");
    }))
}
```

| 項目 | 内容 |
| --- | --- |
| `report.source` | `ErrorSource`。`TextCommand`、`SlashCommand`、`Button`、`Select`、`Modal`、`Message`、`Event`、`Gateway`、`Response`（自動応答の失敗）、`QueueFull`（処理枠が満杯）など。今後増える可能性があります（`#[non_exhaustive]`）。 |
| `report.name` | コマンド名、`custom_id`、イベント名など、原因を絞り込む手がかり。分からないときは `None`。 |
| `report.error` | `&Error`。ハンドラーが返した型付きエラーをそのまま渡します。 |

ログへ出力する場合は `report` を整形すると、Gateway のデコードエラーから受信ペイロードが除かれます。

- Akari は、標準出力にも標準エラー出力にも書きません。エラーは `tracing` のイベント（`error` と `warn`、`target` は `disnana_akari`）として発行します。出力先は、アプリが `tracing` の購読先で決めます。この関数は、ログとは別に、コードで反応するためのもので、設定してもログは変わりません。
- `{report}` で表示すると、`場所 `名前`: エラー: 原因: 原因…` の形で、**原因の連鎖も**続けて表示します（たとえば `Http` では、本当の理由は原因の側にあります）。
- 関数は処理の途中で同期的に呼ばれます。短く保ち、`panic` しないでください。重い処理は、チャンネルへ送るなどして、別のタスクで行います。
- Gateway が接続を閉じたときは、理由を含めて報告します。たとえばコード `4014` では、特権 Intent を有効にするよう案内します。
- Akari は `tracing` や `log` を使いません。必要なら、`on_error` の中で、お使いのロガーへ渡してください。

### 自動で送る文言

Akari は、Discord の決まりを満たすために、いくつかの場面で利用者へメッセージを送ります。`Bot::with_messages` で、文言を変えたり、送らないようにしたりできます。

```rust,no_run
use disnana_akari::{Bot, Messages, Result};

fn build() -> Result<Bot> {
    let mut messages = Messages::default();
    messages.interaction_busy = Some("Only a moment, please.".to_owned());
    messages.text_command_failed = Some("Something went wrong.".to_owned()); // 既定は送らない
    messages.modal_accepted = None; // 何も送らない
    Ok(Bot::from_env()?.with_messages(messages))
}
```

| 項目 | 送る場面 | 既定 |
| --- | --- | --- |
| `text_command_failed` | テキストコマンドの関数がエラーを返したとき（元の投稿へ返信） | 送らない |
| `text_command_busy` | テキストコマンドの処理枠が満杯のとき（元の投稿へ返信） | 送らない |
| `interaction_busy` | スラッシュ、ボタン、選択メニュー、フォームの処理枠が満杯のとき（本人にだけ） | 「現在混雑しています。…」 |
| `slash_command_failed` | スラッシュコマンドの関数がエラーを返したか、応答しなかったとき | 「コマンドを処理できませんでした。」 |
| `slash_command_unavailable` | Akari に登録していないスラッシュコマンドが実行されたとき（本人にだけ） | 「このコマンドは現在利用できません。」 |
| `check_failed` | `require_*` の確認に通らず、ハンドラーが `CheckFailure` を返したとき（スラッシュでは本人にだけ、テキストでは元の投稿への返信） | 「このコマンドを使う権限がありません。」 |
| `bot_check_failed` | Bot 自身に必要な権限が足りず、`CheckFailure` の `BotMissingPermissions` で拒否したとき（送り方は `check_failed` と同じ） | 「Bot に必要な権限が足りないため、実行できません。サーバーの管理者に連絡してください。」 |
| `hierarchy_failed` | ロールの階層で操作できず、`CheckFailure` の `Hierarchy` で拒否したとき（送り方は `check_failed` と同じ） | 「ロールの階層により、この相手は操作できません。」 |
| `button_failed` | ボタンの関数がエラーを返したとき（本人にだけ） | 「ボタンを処理できませんでした。」 |
| `select_failed` | 選択メニューの関数がエラーを返したとき、または選択内容を読み取れなかったとき（本人にだけ） | 「選択内容を処理できませんでした。」 |
| `modal_failed` | フォームの関数がエラーを返したとき（本人にだけ） | 「入力内容を処理できませんでした。」 |
| `modal_accepted` | フォームの関数が、成功したのに応答しなかったとき（本人にだけ） | 「入力を受け付けました。」 |

- テキストコマンドには、応答の決まりがないので、既定では何も送りません。
- スラッシュコマンド、ボタン、選択メニュー、フォームは、Discord が受信後 3 秒以内の応答を求めます。そのため、これらには既定の文言があります。`None` にすると、応答が済んでいない操作は Discord 上で失敗と表示されます。
- 文言を `None` にしても、エラー自体は `on_error` に報告します。
- 成功した（エラーを返さなかった）ボタンや選択メニューの操作は、メッセージを出さずに受け付けます。

## `Context<'_>` と `Result<T>`

`Context` は `Bot::run` に渡すメッセージ処理用の型です。`ctx.message()` で元の `&Message` を借りられます。`ctx.reply(content).await`、`ctx.reply_ui(&message).await`、`ctx.reply_v2(&message).await` は元のメッセージへの返信を送ります。`ctx.timing()` で受信から処理までの測定値を取得できます。

`Bot::run` が返す Future は `Send` ではありません。実行中のタスクで直接 `.await` するか、Tokio の `LocalSet` を使ってください。

`Result<T, E = Error>` は `std::result::Result<T, E>` の別名で、エラー型の既定は Akari の `Error` です。`Error::MissingIntents` は不足している Intent を保持し、`Error::Http` や `Error::CommandRegistration` は Twilight の原因を保持します。`Error`、`CommandSource`、`Mentionable` は `#[non_exhaustive]` です。外部のクレートで `match` を書く場合は `_` の分岐を付けてください。

### ハンドラーが返せるエラー

コマンド、スラッシュコマンド、イベント、ボタン、選択メニュー、フォーム、`Bot::run` のハンドラーは、`Result<(), E>` を返せます。`E` は `Into<Box<dyn std::error::Error + Send + Sync>>` を満たす型なら何でもよく、標準のエラー（`io::Error`、`ParseIntError` など）、自作のエラー型、`Box<dyn Error + Send + Sync>` に、そのまま `?` を使えます。

```rust
use disnana_akari::CommandContext;

// 標準のエラー（ParseIntError）と、Akari のエラー（reply の失敗）の両方に ? を使える
async fn double(ctx: CommandContext<'_>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let n: i64 = ctx.args().parse()?;
    ctx.reply(&(n * 2).to_string()).await?;
    Ok(())
}
```

- Akari が返したエラー（`ctx.reply(...).await?` など）は、`Result<()>`（`Result<(), Error>`）を返すハンドラーなら、そのまま `?` で伝播し、`Error` のバリアントも保たれます。標準のエラーと Akari のエラーの両方に `?` を使いたいときは、`Box<dyn Error + Send + Sync>` を返すと便利です。ハンドラーが特定の型（`io::Error` など）を返す場合、Akari の `Error` からの変換はないので、`ctx.reply(...).await` の結果は自分で扱ってください。
- 標準のエラーなどは、Akari が `Error::Handler` で包んで `on_error` に報告します。`Error::Handler` が持つ `HandlerError` から、`downcast_ref::<T>()`、`inner()`、`into_inner()` で元のエラーを取り出せます。表示と原因の連鎖は、元のエラーと同じです（メッセージが二重に表示されません）。
- メッセージだけのエラーは `Error::msg("使い方: !echo 文章")` で作れます。`Error::handler(e)` は、任意のエラーを `Error::Handler` に包みます。
- **クロージャ**は、末尾が関数呼び出しなら、そのまま渡せます。`Ok(())` で終わるときは、戻り値の型を書きます: `async move |ctx: CommandContext<'_>| -> Result<()> { ...; Ok(()) }`。書かないと、エラー型を決められずに `type annotations needed` になります。

## ハイブリッドの添付とメンション

`SlashCommands::attachment_option` と `mentionable_option` を型付きハイブリッドの登録にも使えます。`HybridArguments::attachment` は Discord が渡した添付メタデータを借り、`mentionable` はユーザーかロールを型付きで返します。テキストの添付は投稿に添付した順に読み、メンションはユーザー用かロール用の書式を指定します。

## サブコマンドと選択肢の翻訳

`CommandOptionLocalizations::apply(option)` で引数に名前と説明の翻訳を付けます。`CommandGroup::set_child_localizations` と `HybridCommandGroup::set_child_localizations` は、元の名前を指定して子サブコマンドに翻訳を付けます。`SlashCommands::localize_choice` では固定の選択肢の表示名を翻訳できます。

## 音声状態を変更する

`voice.set_state(guild_id, VoiceStateChange::join(channel_id)).await` で参加または移動を、`VoiceStateChange::leave()` で退出を担当シャードへ要求します。`self_mute`、`self_deaf` も設定できます。呼び出しの成功は、シャードが送信キューへの追加を受理した時点を示します。

`join(guild_id, channel_id, timeout)` と `leave(guild_id, timeout)` は Discord の確認通知まで待ちます。`VoiceConnection::status()` と `changed().await` は音声サーバーの割り当てと切断を確認するための API です。

音声接続の準備では、Bot 自身の `VoiceStateUpdate` と同じサーバーの `VoiceServerUpdate` を受け取ります。`VoiceServerUpdate` に含まれるトークンは機密情報です。

## 通常の添付返信

`Context`、`CommandContext`、`SlashContext`、`HybridContext` の `reply_files` で本文と添付を送れます。添付だけを送る場合は空の本文を指定します。`SlashContext::reply_files_ephemeral` は本人向けで、`defer_ephemeral` 後の待機表示も完了できます。

## 添付付きの追加メッセージ

`SlashContext`、`ModalContext`、`ButtonContext` と各選択メニューのコンテキストでは、公開の `followup_files(&self, content: &str, files: &[Attachment], limits: AttachmentLimits)` と、本人だけに送る `followup_files_ephemeral` を使えます。`Attachment` は `model::http::attachment::Attachment`、`AttachmentLimits` は `validation::AttachmentLimits` です。添付だけを送るときは本文に空文字を指定します。本文と添付が両方空の場合はエラーになります。

スラッシュコマンドとフォーム送信では、遅延応答の待機表示を `reply` などで完成させた後に送れます。ボタンと選択メニューでは、`defer_update` で操作を受け付けた後にすぐ送れます。公開範囲は追加メッセージごとに選べます。送信前に本文、添付の件数・名前・ID・サイズを検証し、Discord が Interaction に通知した1ファイルのサイズ上限と `AttachmentLimits` が適用されます。[Discord の追加メッセージ API](https://docs.discord.com/developers/interactions/receiving-and-responding#create-followup-message)

### テキストコマンドの実行フック

`Commands::set_before_invoke` と `set_after_invoke` は、チェックと型付き引数の変換が通ったテキスト実行の前後に呼ばれます。実行後フックにはハンドラーのエラーが渡ります。ハイブリッドのテキスト実行にも適用します。

## 送信前の検証

`validation::message_content`、`attachment_count`、`attachment_sizes` で、本文の文字数、保持分を含む添付件数、Discord が通知した1ファイルのバイト上限を確認します。本文は上限超過を検出した時点で走査を打ち切ります。 `AttachmentLimits` では、保持分を含む件数、1 ファイルのサイズ、新規送信分の合計サイズにアプリ独自の上限を設定できます。値を `None` にすると、その項目のアプリ独自の検査を外します。`ComponentsV2::attachment_limits` で返信と編集に適用できます。`UiMessage::validate`、`ComponentsV2::validate`、`Modal::validate` は各ビルダーの構成を検証します。

## テキストコマンドのメタデータとヘルプ

`CommandMetadata` で説明、使い方、標準ヘルプの表示有無を設定できます。`Commands::set_metadata` は正式名または別名から指定し、`command_info(path)` と `command_infos()` で登録情報を参照できます。`Commands::register_help(name)` で一覧と詳細を表示するテキストコマンドを追加します。

## そのほかの公開型

ほかの節で触れていない公開型です。いずれも、対応する機能の戻り値やイベントとして受け取ります。

| 型 | 内容 |
| --- | --- |
| `CommandInfo` | 登録済みテキストコマンドの正式名、別名、説明です。`children()`、`is_group()`、`is_hidden()`、`is_invokable()` でグループや非表示の状態を確認できます。 |
| `CommandCheckSource` | `CommandCheckContext::source()` が返す、チェックの呼び出し元です。 |
| `HybridArgumentError` | 型付きハイブリッドコマンドが受け取った引数のエラーです。 |
| `GatewayShardStatus` | 一つのシャードの状態と、ハートビートの計測値です。 |
| `GatewayStatusUpdates` | Gateway の状態が変化したときに、最新の一覧を受け取ります。 |
| `VoiceConnectionInfo` | 音声処理ライブラリに渡す一時的な接続情報です。ログや保存先に出さないでください。 |
| `VoiceConnectionStatus` | 音声サーバーの割り当て状況です。 |
| `VoiceGatewayError` | 音声操作を完了できなかった理由です。 |
| `SoundboardSound`、`SoundboardSoundId`、`SoundboardSoundMarker`、`SoundboardSounds`、`VoiceEffectAnimation` | Discord のサウンドボード音とその一覧、再生時の演出です。 |
| `GuildSoundboardSoundCreate`、`GuildSoundboardSoundUpdate`、`GuildSoundboardSoundDelete`、`GuildSoundboardSoundsUpdate` | サウンドボード音の追加、更新、削除、一括更新の通知です。 |

## 権限の確認（Access）

`ctx.access()` が返す `Access` で、実行者、Bot、ほかのメンバーの権限を調べ、確認します。`can`、`missing`、`bot_can`、`bot_missing` は調べるだけで、`require_permissions`、`require_bot_permissions`、`require_guild_owner`、`require_role`、`require_any_role`、`require_can_moderate` は、満たさなければ `CheckFailure` で失敗します。`snapshot` は、実行者と Bot の権限と所有者かどうかを、`PermissionSnapshot` にまとめて返します。

`Needs` は、コマンドの実行に必要な権限を宣言する型です。`Needs::new().author(..).bot(..)` を、`Commands::require` と `SlashCommands::require` に渡すと、ハンドラーの前に、実行者、Bot の順に確認します。宣言は、指定したコマンドとその配下だけに適用し、登録されていない名前は、登録の時点で失敗します。独自のチェックでは、`CommandCheckContext::access()` で `Access` を使えます。

権限の上書きとロールの権限は、`Access` の `overwrite_of`、`set_overwrite`、`delete_overwrite`、`role_permissions`、`set_role_permissions` で読み書きします。`Overwrite` は、`allow`、`deny`、`inherit` で権限ごとの状態を指定する型で、`state` が `Tri`（`Allow`、`Deny`、`Inherit`）を返します。対象は `OverwriteTarget`（`Role` か `Member`）で指定します。書き込みは、送る前に Bot の権限とロールの位置を確認しますが、実行した人への確認は、呼ぶ側の責任です。

`ctx.manage()` が返す `Manage` で、`kick_member`、`ban_member`、`unban_member`、`timeout_member`、`set_nickname`、`add_role`、`remove_role`、`move_member`、`set_voice_mute`、`set_voice_deafen` を呼べます。送る前に Bot の権限とロールの階層を確認し、実行した人への確認は呼ぶ側の責任です。チャンネルは `create_text_channel`（`NewTextChannel`）、`create_voice_channel`（`NewVoiceChannel`）、`create_category`（`NewCategory`）で作成し、`edit_text_channel`（`TextChannelEdit`）、`edit_voice_channel`（`VoiceChannelEdit`）で編集し、`delete_channel` で削除します。ロールは `create_role`（`NewRole`）、`edit_role`（`RoleEdit`）、`delete_role` です。編集は指定した項目だけを変更し、何も指定しないと `InvalidArgument::NothingToChange` で失敗します。

タイムアウトは `MAX_TIMEOUT`（28 日）、BAN と一緒に削除するメッセージは `MAX_BAN_MESSAGE_DELETE`（7 日）までで、超えると `InvalidArgument` で、通信せずに失敗します。ロールの階層で断られた理由は、`CheckFailure::Hierarchy` が持つ `HierarchyDenied` で分かります。

## 権限やフラグを名前で扱う

`use disnana_akari::prelude::*;` で、`PermissionsExt`、`PermissionPresets`、`MessageFlagsExt`、`UserFlagsExt`、`SystemChannelFlagsExt`、`ChannelFlagsExt`、`Label` を取り込みます。Twilight の型に、名前で調べるメソッド（`perms.manage_messages()`）、`names()`、`labels()`、`unknown_bits()` を足します。`Permissions::text()`、`voice()`、`moderator()` は、目的ごとの権限のまとまりです。`Label::label()` は、`ChannelType`、`VerificationLevel`、`DefaultMessageNotificationLevel`、`ExplicitContentFilter`、`MfaLevel`、`NSFWLevel`、`PremiumTier` を日本語の名前にします。トレイトは封印されていて、ほかの型には実装できません。

## コマンドの共通チェック

`CommandCheck::new` には `async fn(CommandCheckContext<'_>) -> Result<()>` または同等のクロージャを渡します。同じチェックを `Commands::add_check` と `SlashCommands::add_check` の両方に渡せます。`command_path()` はグループを含めた正式なコマンド名を親から順に返します。`source()`、`author_id()`、`guild_id()`、`channel_id()` で呼び出し元を確認できます。

`require_guild()`、`require_permissions(...).await` で既存の権限確認と同じエラーが返ります。独自の条件での拒否には `CheckFailure::Rejected` を使います。いずれかのチェックが失敗した場合、以降のチェックとハンドラーを実行しません。

## Twilight の型と REST クライアントを使う

`disnana_akari::model` は Twilight のモデル型、`disnana_akari::http` は Twilight の HTTP API です。`Context`、`CommandContext`、`SlashContext`、`HybridContext`、`ButtonContext`、`SelectContext`、`ModalContext` の `http()` は、Bot と同じ `&http::Client` を返します。Akari の返信と同じ接続やレート制限の処理を利用できます。

```rust,no_run
use disnana_akari::{Context, Result, model::channel::Channel};

async fn fetch_channel(ctx: Context<'_>) -> Result<Channel> {
    let response = ctx.http().channel(ctx.message().channel_id).await?;
    let channel = response.model().await?;
    Ok(channel)
}
```

スラッシュコマンドとボタン操作の応答は、`reply`、`defer`、`update_message` など対応するコンテキストのメソッドで送ります。これらのメソッドは、初回応答や更新が済んだかどうかを Akari に記録します。[Twilight HTTP の API](https://docs.rs/twilight-http/0.18.0/twilight_http/)

## 安定性（Twilight の型）

Akari は Twilight 0.18 の上に作られていて、公開 API に Twilight の型が出ています。Twilight は 0.x なので、Twilight の更新に合わせて Akari も、互換性のない変更を入れることがあります。

| 場所 | Twilight の型 |
| --- | --- |
| `disnana_akari::model`、`disnana_akari::http` | `twilight_model`、`twilight_http` の再エクスポート |
| `Error` の `Http`、`HttpBody`、`GatewayReceive`、`*Validation`、`CommandRegistration` | Twilight のエラー型 |
| `Context` などの `message()`、`http()`、`Intents`、`Ready`、スラッシュコマンドの `CommandOption` | Twilight の型 |
| `InfoEvent` の `InteractionType`、`InteractionResponseType` | Twilight の型 |

- Twilight の型を使うときは、`twilight-*` を自分の依存に加えず、`disnana_akari::model` と `disnana_akari::http` を使ってください。版がずれて、型が一致しなくなることを防げます。
- Akari が Twilight の互換性のない版（0.19 など）に上げるときは、Akari の 0.x のマイナーバージョンを上げ、変更履歴に書きます。
- Akari 自身の列挙型（`Error`、`ErrorSource`、`InfoEvent`、`CommandSource`、`Mentionable`）は `#[non_exhaustive]` です。バリアントが増えても、`_` の分岐があれば壊れません。

## 使用例

```rust,no_run
use std::time::Duration;

use disnana_akari::model::gateway::Intents;
use disnana_akari::{
    Bot, ButtonContext, CommandContext, Commands, Cooldown, Error, Events, HybridContext, Ready, Result,
    SlashCommands, SlashContext, register_hybrid, ui::{Button, Embed, UiMessage},
};

const CONFIRM: &str = "akari:example:confirm";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut commands = Commands::new("!")?;
    commands.register("ping", ping)?;

    let guild_id = std::env::var("DISCORD_GUILD_ID")
        .map_err(Error::handler)?
        .parse()
        .map_err(Error::handler)?;
    let mut slash = SlashCommands::for_guild(guild_id);
    slash.register("ping", "Bot の応答を確認します", vec![], slash_ping)?;
    slash.register("ui", "埋め込みとボタンを表示します", vec![], show)?;
    register_hybrid(
        &mut commands,
        Some(&mut slash),
        "echo",
        "入力した文章を返します",
        vec![SlashCommands::string_option("text", "返す文章", true)],
        echo,
    )?;
    let cooldown = Cooldown::new(Duration::from_secs(1));
    register_hybrid(
        &mut commands,
        Some(&mut slash),
        "latency",
        "Bot のキュー待ちを確認します",
        vec![],
        async move |ctx| latency(ctx, &cooldown).await,
    )?;

    let mut events = Events::new();
    events.on_ready(ready);
    events.on_button(CONFIRM, confirm)?;

    Bot::from_env()?
        .add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)
        .with_events(events)
        .with_slash_commands(slash)
        .run_commands(commands)
        .await
}

async fn ping(ctx: CommandContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}

async fn slash_ping(ctx: &mut SlashContext<'_>) -> Result<()> {
    ctx.reply("Pong!").await
}

async fn show(ctx: &mut SlashContext<'_>) -> Result<()> {
    let message = UiMessage::new()
        .embed(Embed::new("確認").description("下のボタンを押してください。"))
        .buttons(vec![Button::primary(CONFIRM, "確認する")]);
    ctx.reply_ui(&message).await
}

async fn confirm(ctx: &mut ButtonContext<'_>) -> Result<()> {
    let message = UiMessage::new().content("確認済みです。");
    ctx.update_message(&message).await?;
    ctx.followup_ephemeral("確認しました。").await
}

async fn echo(ctx: &mut HybridContext<'_>) -> Result<()> {
    let Some(text) = ctx.text_arg("text") else {
        return ctx.reply("返したい文章を入力してください。").await;
    };
    ctx.reply(text).await
}

async fn latency(ctx: &mut HybridContext<'_>, cooldown: &Cooldown) -> Result<()> {
    if !ctx.check_cooldown(cooldown).await? {
        return Ok(());
    }

    let wait = ctx.timing().queue_wait();
    ctx.reply(&format!("キュー待ち {:.3} ms", wait.as_secs_f64() * 1_000.0))
        .await
}

async fn ready(_event: &Ready) -> Result<()> {
    println!("Bot の準備ができました");
    Ok(())
}
```
