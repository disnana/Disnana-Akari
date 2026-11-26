# API リファレンス

## `Bot`

| メソッド | 動作 |
| --- | --- |
| `Bot::new(token) -> Result<Bot>` | トークンの前後の空白を除去して Bot を作ります。値が空の場合はエラーを返します。 |
| `Bot::from_env() -> Result<Bot>` | 環境変数 `DISCORD_TOKEN` から Bot を作ります。 |
| `bot.with_events(events) -> Bot` | Gateway イベントに応じて呼び出す関数の一覧を設定します。 |
| `bot.with_slash_commands(commands) -> Bot` | スラッシュコマンドを指定サーバーで有効にします。 |
| `bot.with_intents(intents) -> Bot` | 要求する Gateway Intent を、渡された `Intents` の集合にします。既定は空です。 |
| `bot.add_intents(intents) -> Bot` | 要求する Intent に、渡された `Intents` を追加します。 |
| `bot.intents() -> Intents` | 起動時に Discord へ要求する Intent を返します。 |
| `bot.on_error(hook) -> Bot` | エラーを一件ずつ受け取る関数を設定します。[エラーと自動で送る文言](#エラーと自動で送る文言) |
| `bot.with_messages(messages) -> Bot` | Akari が利用者へ自動で送る文言を設定します。 |
| `bot.start().await -> Result<()>` | 設定したイベントとスラッシュコマンドで Bot を起動します。 |
| `bot.run_commands(commands).await -> Result<()>` | 上記に加えて、テキストコマンドを実行します。 |
| `bot.run(on_message).await -> Result<()>` | テキストコマンドの代わりに、受信メッセージを自前の `async fn on_message(ctx: Context<'_>) -> Result<()>` で処理します。 |

起動メソッドは Tokio ランタイム内で呼び出します。

### Intent

Akari は Intent を **自動では追加しません**。要求する Intent は、`with_intents` と `add_intents` で指定したものだけです。`with_intents` で基本の集合を決め、`add_intents` で足します。両方を指定した場合は、呼ぶ順序に関係なく和集合になります。`Intents` は `disnana_akari::model::gateway::Intents` から参照できます。

- テキストコマンド、`Events::on_message`、`Bot::run` のメッセージ処理を使うには、`GUILD_MESSAGES` と `MESSAGE_CONTENT` が必要です。足りない場合は、Discord に接続する前に、足りない Intent と対処を示すエラーを返します。
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
| スラッシュコマンドと登録済みのボタン、選択メニュー、フォーム送信 | 合計 8 件 | 合計 64 件 | `on_error` に報告し、`Messages::interaction_busy`（既定あり）を本人にだけ返します。 |
| `on_ready`、`on_message` と `Bot::run` の自前ハンドラー | 1 件 | 64 件 | 新しく届いたイベントを破棄し、`on_error` に報告します。 |

テキストコマンド用のキューには、登録名に一致したメッセージだけが入ります。`on_ready` と `on_message` は受信順に処理し、一件のイベントに対する関数は登録順に呼びます。フォーム送信を含むコンポーネント操作は、スラッシュコマンドと同じ枠で並行して処理します。`Bot::run` の自前ハンドラーは、そのメッセージに対する `Events::on_message` より先に実行します。`Bot::run_commands` のコマンドと `on_message` は独立して進むため、どちらが先に完了するかは処理時間によって変わります。

混雑の返信には、テキストとスラッシュ・ボタンで別々の処理枠を使います。それぞれ同時に 8 件、キューは 64 件までです。混雑の返信用のキューが満杯の場合や、返信に失敗した場合も、`on_error` に報告します。Gateway のイベントストリームが終わると、処理中と保留中のイベントも中止します。

ハンドラーが I/O の完了を `.await` している間は Gateway の受信を続けられます。同期 I/O や長い計算は、同じタスク内の他の処理も止めるため、Tokio の `spawn_blocking` などで切り分けます。

## `Commands` と `CommandContext<'_>`

| メソッド | 動作 |
| --- | --- |
| `Commands::new(prefix) -> Result<Commands>` | `!` などのプレフィックスを指定します。空文字列と空白を含む値はエラーです。 |
| `commands.register(name, handler) -> Result<()>` | プレフィックスを省いた名前で非同期関数を登録します。空の名前、空白がある名前、重複した名前はエラーです。 |
| `commands.dispatch(ctx).await -> Result<()>` | 該当するコマンドを実行し、ハンドラーが返したエラーを呼び出し元に返します。`Bot::run` の独自処理にも組み込めます。 |
| `ctx.args() -> &str` | コマンド名に続く本文を借ります。前後の空白を除去し、引数なしの場合は空文字列を返します。 |
| `ctx.context() -> &Context<'_>` | 元のメッセージを扱うコンテキストを借ります。 |
| `ctx.timing() -> Timing` | このテキストコマンド呼び出しの測定値を取得します。 |
| `ctx.reply(content).await -> Result<()>` | コマンドのメッセージに返信し、返信先の投稿者や本文中のメンションへの通知を抑止します。 |
| `ctx.reply_ui(&message).await -> Result<()>` | `UiMessage` を元のメッセージに返信します。 |
| `ctx.reply_v2(&message).await -> Result<()>` | `ComponentsV2` を元のメッセージに返信します。 |

名前は大文字と小文字を区別します。最初の空白までがコマンド名です。`! ping` のようにプレフィックスの直後が空白の投稿、未登録の名前、通常の投稿は無視されます。

`Bot::run_commands` で登録済みのコマンドの実行に失敗した場合は、エラーを `on_error` に報告します。元の投稿への返信は、既定では **しません**。返信したい場合は `Messages::text_command_failed` に文言を設定します。`commands.dispatch` を直接使う場合は、呼び出し元でエラーを受け取って返信やログを扱います。

## `SlashCommands` と `SlashContext<'_>`

| メソッド | 動作 |
| --- | --- |
| `SlashCommands::for_guild(guild_id) -> SlashCommands` | Discord のサーバー ID を指定して空のコマンド一覧を作ります。ID の型は `Id<GuildMarker>` です。 |
| `commands.replace_guild_commands() -> SlashCommands` | 起動時に、このアプリのサーバー内の全コマンドを登録した一覧に一括で置き換えます。 |
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

### サーバーに登録するコマンドを同期する

既定の起動では、同名のスラッシュコマンドを追加または更新し、ほかの既存コマンドを保持します。Akari に登録していないサーバー限定のスラッシュコマンドを受信した場合は、`on_error` に報告し、`Messages::slash_command_unavailable`（既定は「このコマンドは現在利用できません。」）を本人にだけ返します。

`replace_guild_commands()` を指定すると、登録一覧に含まれないコマンドが対象サーバーから削除されます。削除の範囲は同じアプリ・同じサーバーの全アプリケーションコマンドで、ユーザーやメッセージを右クリックして実行するコマンドも対象です。空の一覧で一括同期すると対象の全コマンドが削除されます。[Discord のサーバー単位の一括上書き](https://docs.discord.com/developers/interactions/application-commands#bulk-overwrite-guild-application-commands)

### 時間がかかる処理に応答する

関数の形は `async fn handler(ctx: &mut SlashContext<'_>) -> Result<()>` です。Discord は Interaction の受信後 3 秒以内に初回応答を求めます。外部サービスへの問い合わせなど、期限に間に合わない可能性がある処理の前に、ハンドラーから `ctx.defer().await?` または `ctx.defer_ephemeral().await?` を明示的に呼びます。処理後に `ctx.reply(...).await?` で処理中の表示を本文に置き換えます。公開範囲は `defer` または `defer_ephemeral` の時点で決まります。公開の `defer` の後に `reply_ephemeral` を呼ぶとエラーになります。`followup` と `followup_ephemeral` は最初の本文を送った後に呼びます。Discord の Interaction トークンの有効期間は 15 分です。[Discord の Interaction 応答仕様](https://docs.discord.com/developers/interactions/receiving-and-responding)

公開の `defer` の後に `reply_ui_ephemeral` を呼ぶと、公開範囲を変更できないためエラーになります。ハンドラーが最初の本文を送れずに終了した場合は、Akari がエラー応答を試みます。初回応答が済んでいない場合は本人にだけ表示し、処理中の応答がある場合はその表示をエラー文に置き換えます。返信本文中のメンション通知は抑止します。

## `register_hybrid` と `HybridContext<'_>`

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
| `events.on_button(custom_id, handler) -> Result<()>` | 指定した識別子に完全一致するボタンが押されたときに `&mut ButtonContext<'_>` を渡します。識別子は 1～100 文字で、Bot の登録一覧全体で、同じ識別子に関数を一つ登録できます。 |
| `events.on_select(custom_id, handler) -> Result<()>` | 指定した識別子の文字列選択メニューが操作されたときに `&mut SelectContext<'_>` を渡します。 |
| `events.on_user_select(custom_id, handler) -> Result<()>` | ユーザー選択時に `&mut UserSelectContext<'_>` を渡します。 |
| `events.on_role_select(custom_id, handler) -> Result<()>` | ロール選択時に `&mut RoleSelectContext<'_>` を渡します。 |
| `events.on_channel_select(custom_id, handler) -> Result<()>` | チャンネル選択時に `&mut ChannelSelectContext<'_>` を渡します。 |
| `events.on_mentionable_select(custom_id, handler) -> Result<()>` | ユーザーとロールを候補に持つメニューの選択時に `&mut MentionableSelectContext<'_>` を渡します。 |
| `events.on_modal(custom_id, handler) -> Result<()>` | 指定した識別子のフォームが送信されたときに `&mut ModalContext<'_>` を渡します。 |

`on_ready` と `on_message` には複数の関数を登録できます。各関数が返したエラーは `on_error` に報告され、次のハンドラーへ進みます。コマンドとイベントの順序は [ハンドラーの実行順](#ハンドラーの実行順) を参照してください。

## `UiMessage` と `ButtonContext<'_>`

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
| `SelectOption::new(label, value)` | メニューに表示するラベルと、選択時に返す値を設定します。`description` で選択肢の説明を設定できます。 |
| `StringSelect::new(custom_id, options)` | 選択メニューを作ります。`placeholder` で選択する前の表示、`min_values` と `max_values` で選べる個数、`disabled` で無効状態を指定できます。 |
| `Modal::new(custom_id, title)` | 入力フォームを作ります。 |
| `form.text_input(TextInput::new(custom_id, label))` | フォームに入力欄を追加します。`paragraph` で複数行入力、`required` で必須かどうか、`min_length` と `max_length` で文字数、`value` で初期値を設定できます。 |

選択メニューの既定値は一項目です。最小選択数は 0～25、最大選択数は 1～25 で、最小値は最大値以下、最大値は用意した選択肢の数以下にします。入力文字数の最小値は 0～4,000、最大値は 1～4,000 です。空でない初期値は設定した文字数の範囲に収めます。入力欄を空で始めたい場合は、初期値を省略するか空文字列にします。

`SelectContext` には、メニューの `custom_id()` と選択結果の `values() -> &[String]` が渡ります。`author_id()`、`guild_id()`、`message()`、`invoker_id()` で操作者や元のメッセージを確認でき、ボタンと同じように `reply_ephemeral`、`defer_update`、`update_message`、`update_v2`、`show_modal` を呼べます。

### ユーザー、ロール、チャンネルを選択する

候補を Discord が用意するメニューは `UserSelect::new(custom_id)`、`RoleSelect::new(custom_id)`、`ChannelSelect::new(custom_id)` で作ります。`UiMessage` と `ComponentsV2` に `user_select`、`role_select`、`channel_select` でそれぞれ追加できます。各ビルダーは `placeholder`、`min_values`、`max_values`、`disabled` を持ちます。`ChannelSelect::channel_types(Vec<ChannelType>)` でチャンネルの種類を絞れます。

同じメニューからユーザーとロールを選ぶ場合は、`MentionableSelect::new(custom_id)` で作り、`UiMessage` または `ComponentsV2` に `mentionable_select` で追加します。選択した項目は Discord が送った順で `Mentionable::User(Id<UserMarker>)` または `Mentionable::Role(Id<RoleMarker>)` として受け取ります。

| コンテキスト | `values()` の型 |
| --- | --- |
| `UserSelectContext<'_>` | `&[Id<UserMarker>]` |
| `RoleSelectContext<'_>` | `&[Id<RoleMarker>]` |
| `ChannelSelectContext<'_>` | `&[Id<ChannelMarker>]` |
| `MentionableSelectContext<'_>` | `&[Mentionable]` |

`values()` で対象ごとに型の付いた ID を取得できます。Discord が選択対象の情報を同梱した場合は、`resolved()` で `&InteractionDataResolved` を取得できます。操作への本人宛ての返信には `reply_ephemeral` を使います。[Playground の選択メニュー実装](../examples/ping/entities.rs)

ユーザーとロールを混ぜたメニューでは、選択された各 ID の種類を Discord の `resolved.users` と `resolved.roles` で判定します。選択があるときに `resolved` が欠けている場合や、ID の種類が一意に決まらない場合は、ハンドラーを呼ばず本人へのエラー応答を試みます。選択数が 0 件なら、`resolved` がなくても空の `values()` を受け取れます。

### 入力フォームを開く

`show_modal` はスラッシュコマンド、ボタン、選択メニューの初回応答として呼びます。フォームを送信すると、フォームの識別子に対応する `on_modal` の関数が呼ばれます。`ModalContext::text_input(custom_id)` は、入力欄の識別子に対応する値を `Option<&str>` で返します。`custom_id()` はフォームの識別子を返し、`author_id()` と `guild_id()` は送信者とサーバーの情報を返します。

### フォーム送信に応答する

| メソッド | 動作 |
| --- | --- |
| `ctx.reply(content).await -> Result<()>` | チャンネルに返信します。遅延応答の後は、公開範囲を維持して処理中の表示を置き換えます。 |
| `ctx.reply_ephemeral(content).await -> Result<()>` | 本人にだけ見える初回応答を送ります。`defer_ephemeral` の後は、処理中の表示を置き換えます。 |
| `ctx.defer().await -> Result<()>` | 最初に処理中の応答をチャンネルに送ります。 |
| `ctx.defer_ephemeral().await -> Result<()>` | 最初に本人にだけ見える処理中の応答を送ります。 |
| `ctx.followup(content).await -> Result<()>` | 最初の本文を送った後、チャンネルに追加のメッセージを送ります。 |
| `ctx.followup_ephemeral(content).await -> Result<()>` | 最初の本文を送った後、本人にだけ追加のメッセージを送ります。 |

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
| `report.error` | エラーの内容。ハンドラーが返したエラーは、そのまま渡します。 |

- 関数を設定すると、Akari は標準エラー出力へは書きません。設定しない場合は、`[akari] ...` の形で標準エラー出力に書きます。
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

`Result<T>` は `std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>` の別名です。関数内で `?` を使うと、失敗時にそのエラーを呼び出し元へ返します。

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

## 使用例

```rust,no_run
use std::time::Duration;

use disnana_akari::model::gateway::Intents;
use disnana_akari::{
    Bot, ButtonContext, CommandContext, Commands, Cooldown, Events, HybridContext, Ready, Result,
    SlashCommands, SlashContext, register_hybrid, ui::{Button, Embed, UiMessage},
};

const CONFIRM: &str = "akari:example:confirm";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut commands = Commands::new("!")?;
    commands.register("ping", ping)?;

    let guild_id = std::env::var("DISCORD_GUILD_ID")?.parse()?;
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
