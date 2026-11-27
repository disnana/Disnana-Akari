# 次の版以降の設計メモ

discord.py と比べて不便な点を洗い出し、Akari の方針（シンプル、規則的、高速、安定、安全）を守ったまま埋める順番と設計をまとめます。**ここに書いた API はまだ実装していません。** 実装するときに、この文書と食い違う点があれば、先にこの文書を直します。

## 守る方針

- **バージョンは 0.3.x の中で上げます。** 追加だけの変更なので、0.3.1、0.3.2 と進めます。0.4.0 は、互換性を壊す変更が必要になるまで出しません（バージョンの変化を速くしすぎないためです）。
- 公開 API は追加だけにします。`cargo-semver-checks` で既存の API が壊れていないかを確かめます。
- 勝手に確認や処理をしません。呼んだとき、または登録で宣言したときだけ動きます。
- 状態を持つ機能は opt-in にして、上限、タイムアウト、後始末を必須にします。
- 取得できない、判断できないときは、許可せずに失敗します。
- 公開した型は文書に載せます（載せないと CI が落ちます）。例のコードは、文書の検査でコンパイルします。
- 実機でしか確かめられない部分は「実機未確認」と書きます。

## 実装の順番

| 段階 | 内容 | d.py での対応 |
| --- | --- | --- |
| 0.3.1 | 権限の補助（下の「権限の補助」） | `has_permissions`、`bot_has_permissions`、`permissions_for`、`top_role` |
| 0.3.1 | 引数の型変換をテキスト、ハイブリッド、スラッシュで共通にする（`Greedy`、`Optional`、範囲、フラグを含む） | `Converter` |
| 0.3.1 | サーバー・チャンネル・ロールの opt-in キャッシュ。依存の枠（132/140）を見て、自前の小さな実装を先に検討する | `bot.get_channel`、`guild.get_role` |
| 0.3.1 | `wait_for`（メッセージ、リアクション） | `bot.wait_for` |
| 0.3.2 | `Member`、`Role`、`Channel`、`User` の変換（メンション、ID、名前）。キャッシュがなければ上限つきで REST を使う | `MemberConverter` など |
| 0.3.2 | `wait_for` のボタン、セレクト、モーダル。UI 補助（実行者だけ操作、タイムアウトで無効化） | `View.interaction_check`、`on_timeout` |
| 0.3.2 | 確認、ページ送りの例を `examples/` に置く（本体には入れない） | `ext.menus` |
| 0.3.3 | コマンド単位のエラー処理、メッセージの多言語化 | `@cmd.error`、`on_command_error` |
| 0.3.3 | 性能ベンチを CI に載せ、遅くなったら落とす | なし |

## 権限の補助

### 現状と不便な点

- `ctx.permissions()` と `ctx.require_permissions(..)` は、実行者の権限しか扱えません。
- チャンネルの上書きやロールの権限を、読んで変更する手段がありません（Twilight の型を直接組み立てます）。
- Bot 自身の権限、別のメンバーの権限、ロール階層、サーバーの所有者、ロールの所持は調べられません。
- 毎回 `.await?` と `contains` を書く必要があります。足りない権限を名前で取り出す方法がありません。

### 層 0: 数値やビットを名前で扱う

d.py は、Discord が数値で返す権限、フラグ、設定の種類を、名前で読み書きできる型にしています。Twilight は、これらをすでに型（ビットフラグと列挙型）にしているので、生の数値を書く必要はありません。足りないのは、d.py にある次の補助です。

- `perms.manage_messages` のような、名前のアクセサ
- `text()`、`voice()` などのプリセット
- 持っているものを名前で列挙する、不足分を調べる
- 人が読める名前（`MANAGE_MESSAGES` → 「メッセージの管理」）

#### 作り方（原則から決めたこと）

| 原則 | 決めたこと |
| --- | --- |
| シンプル | ラッパー型を作りません。Twilight の `Permissions` などに、拡張トレイトを足します。変換は不要で、`ctx.can(..)` が返す型と同じ型をそのまま使えます。 |
| 規則的 | 全部のフラグ型が、同じ名前のメソッドを持ちます。フラグ 1 つにつき名前を 1 か所だけ書く表から、アクセサ、日本語名、列挙を、マクロで生成します。 |
| 重複させない | 更新は、すでにある `insert`、`remove`、`set`（ビットフラグの標準）を使います。`update(**kwargs)` に当たる別の方法は足しません。 |
| 高速 | すべて `const` かインライン化できる関数です。割り当ては行わず、列挙はイテレーターで返します。通信は発生しません。 |
| 安定 | テストで、Twilight の全ビットが表に載っていることを確かめます。Twilight が新しい権限を足すと、表に無い状態を CI が検出して落ちます。表に無いビットは、表示では 16 進数で出し、黙って捨てません。 |
| わかりやすい | `use disnana_akari::prelude::*;` でトレイトを取り込みます。名前のアクセサは、エディターの補完で見つかります。 |

#### 対象

| 対象 | 追加するもの |
| --- | --- |
| `Permissions` | 名前のアクセサ、プリセット（`text`、`voice`、`moderator` など）、`names()`、`labels()`、日本語名 |
| `MessageFlags`、`UserFlags`、`SystemChannelFlags`、`ChannelFlags` | 名前のアクセサ、`names()`、日本語名 |
| `ChannelType`、`VerificationLevel`、通知レベル、コンテンツフィルター、MFA、NSFW レベル、ブーストの段階 | 日本語名（`label()`） |

<!-- no-check -->
```rust
use disnana_akari::prelude::*;

let perms = ctx.permissions().await?;
if perms.manage_messages() { /* ... */ }
for label in (Permissions::text() - perms).labels() {
    // 「メッセージの管理」のような日本語名で、不足分を列挙できる
}
```

`CheckFailure::MissingPermissions` のメッセージも、この日本語名で表示します（`Debug` の表示は変えません）。

### 層 1: 登録時に必要な権限を宣言する

<!-- no-check -->
```rust
slash.require("purge", Needs::author(Permissions::MANAGE_MESSAGES)
    .bot(Permissions::MANAGE_MESSAGES | Permissions::READ_MESSAGE_HISTORY))?;
```

- ハンドラーの前に、実行者と Bot の権限を確認します。足りなければ、不足している権限の名前つきで拒否します。
- 宣言は、スラッシュの `default_member_permissions` とヘルプの「必要な権限」にも使います。
- 宣言したコマンドだけが対象です。全コマンドの必要権限を、後から一覧にできます。

### 層 2: 準備済みの結果を名前で読む

<!-- no-check -->
```rust
let p = ctx.snapshot().await?;      // 1 回の取得で、実行者・Bot・チャンネルをまとめて準備する
if p.bot.manage_messages() { /* ... */ }
let missing = p.bot.missing(Permissions::EMBED_LINKS);
```

- `snapshot` は、実行者と Bot の権限、チャンネルの種類（スレッド、NSFW、スローモード）、実行者がサーバーの所有者かどうかを持ちます。
- 取得は 1 回です。層 1 の確認と共有するので、REST は増えません。
- 取得できないときは `Err` を返します。「持っていない」と偽って返しません。

### 層 3: 調べるだけの補助と、拒否する補助

<!-- no-check -->
```rust
if ctx.can(Permissions::MANAGE_MESSAGES).await? { /* ... */ }
if ctx.bot_can(Permissions::MANAGE_MESSAGES).await? { /* ... */ }
let perms = ctx.permissions_of(user_id).await?;          // d.py の permissions_for

ctx.require_bot_permissions(Permissions::BAN_MEMBERS).await?;
ctx.require_can_moderate(target_user_id).await?;          // ロール階層
ctx.require_guild_owner()?;
ctx.require_role(role_id).await?;
```

- 調べるもの（`can`、`missing`）と、満たさなければ失敗するもの（`require_*`）を分けます。
- どれも既存の `PermissionResolver` を使います（サーバーとチャンネルの上書き、スレッドは親、タイムアウトの制限）。同じ無効化の仕組みが働きます。

### 層 4: 既存の設定を読み、名前で変更する

d.py では、`Permissions` の `manage_messages` のような名前、`PermissionOverwrite`（許可、拒否、継承の 3 値）、`channel.set_permissions` で、現在の設定を読んで変更します。Akari でも、数字やビットを書かずに同じことができるようにします。

<!-- no-check -->
```rust
// 読む: チャンネルの上書き設定
let overwrite = ctx.overwrite_of(channel_id, Target::Role(role_id)).await?;   // Option<Overwrite>
overwrite.state(Permissions::SEND_MESSAGES);   // Tri::Allow / Tri::Deny / Tri::Inherit

// 変える: 1 つの対象の上書きを、許可、拒否、継承で指定する
ctx.set_overwrite(
    channel_id,
    Target::Role(role_id),
    Overwrite::new()
        .allow(Permissions::VIEW_CHANNEL)
        .deny(Permissions::SEND_MESSAGES)
        .inherit(Permissions::ADD_REACTIONS),
    "理由",
).await?;

// ロールの権限を読んで変える
let role = ctx.role_permissions(role_id).await?;
ctx.set_role_permissions(role_id, role.with(Permissions::MANAGE_MESSAGES), "理由").await?;
```

- **名前での読み取り**: Twilight の `Permissions` は、すでに `Permissions::MANAGE_MESSAGES` のような名前の定数を持ちます。足りないのは、日本語の表示名（エラーやヘルプ用）と、許可、拒否、継承の 3 値を扱う型です。これを `Overwrite` と `Tri` で補います。
- **1 つの対象だけを更新**: チャンネルの上書きは、Discord の「1 つの対象を更新する」エンドポイントを使います（Twilight の `update_channel_permission`）。一覧全体を読んで書き戻す方式ではないので、同時に別の変更があっても、他の対象の設定を巻き戻しません。
- **送る前の確認**: Discord の規則を、REST の前に確認します。Bot が持っていない権限を付与しようとした、Bot より上のロールを変更しようとした、`MANAGE_ROLES` や `MANAGE_CHANNELS` を持たない、のいずれかなら、リクエストを送らずに理由つきで失敗します。
- **ロールの権限**: 現在の値を読んで、指定した権限だけを足し引きします。値はいつ読んだものかをドキュメントに書き、同時に変更される可能性があることを明示します。
- **監査ログ**: 理由は必須の引数にして、文字数の上限（512 文字）を送る前に検証します。
- 変更系は、層 3 の `require_*` を通したあとに呼ぶ使い方を、ドキュメントの例にします。勝手には確認しません。

### ロール階層の規則

`require_can_moderate` は、次の規則を 1 つの純粋関数にします。

1. サーバーの所有者は、誰からも操作されません。
2. 実行者の最上位ロールが相手より高い場合だけ許可します（同位は不可）。
3. Bot の最上位ロールも、相手より高いことが必要です。
4. 自分自身と Bot 自身は操作できません。

失敗の理由は、`CheckFailure` に新しい種類として追加します。

### 実装の設計

現在のコードを調べて決めた、実装の方針です。

#### 入り口

- 新しい型 `Access<'a>` に、層 2〜4 の操作をまとめます。`Context`、`CommandContext`、`SlashContext`、`HybridContext` に `access()` を足します。
- よく使う `can`、`bot_can`、`require_bot_permissions`、`snapshot` だけは、各コンテキストにも同名のメソッドを置きます。ほかは `ctx.access().require_can_moderate(..)` のように呼びます。
- 型は、新しい `src/access.rs` に置きます。`src/permissions.rs` は計算と取得に専念させます。
- ボタン、セレクト、モーダルのコンテキストは、最初は対象外にします（後から同じ `Access` を足せる作りにします）。

#### 権限の取得元

| 呼び出し元 | 実行者の権限 | Bot の権限 | 他のメンバーの権限 |
| --- | --- | --- | --- |
| スラッシュ | Discord が送る `member.permissions`（REST なし） | Discord が送る `app_permissions`（REST なし） | REST（メンバー取得） |
| テキスト | 既存の計算（サーバーとチャンネルの取得） | REST（Bot のメンバー取得）と既存の計算 | REST（メンバー取得）と既存の計算 |

- 今の `PermissionResolver::resolve` は、メッセージの送り主に固定されています。これを、ユーザー ID、ロール、タイムアウトを受け取る形に一般化し、`resolve` はそれを呼ぶだけにします。
- Bot のユーザー ID は、最初に `current_user` で 1 回取得し、保存します。Bot のメンバー情報は、`guild_member` で取得します（`current_user_guild_member` は OAuth2 のトークンが必要で、Bot のトークンでは使えません）。
- サーバーの取得結果に、ロールの `position` を加えます（階層の判定に使います）。チャンネルの取得結果に、NSFW とスローモードを加えます（`snapshot` に使います）。どちらも、既存のキャッシュと無効化に載せるので、新しい状態は増えません。
- サーバー全体の権限（チャンネルの上書きを含まない）も、同じ計算から取り出せるようにします（ロールの権限変更の確認に使います）。

#### 層 1 の仕組み

- コマンドごとに `Needs` を保存し、全体のチェックの後、ハンドラーの前に確認します。スラッシュの補完にも適用します。
- 登録は、`Commands::require(name, needs)` と `SlashCommands::require(name, needs)` です。ハイブリッドは、両方に同じ `Needs` を渡す登録関数を用意します。
- スラッシュでは、実行者の権限を `default_member_permissions` にも反映します。ただし、すでに明示している場合は上書きしません。
- ヘルプには、`CommandInfo::required_permissions()` で表示できるようにします。

#### 新しい失敗の種類

`CheckFailure` は `#[non_exhaustive]` なので、種類を追加しても既存の `match` は壊れません。

- `BotMissingPermissions(Permissions)`
- `NotGuildOwner`
- `MissingRole`
- `Hierarchy(HierarchyDenied)`（対象が所有者、自分自身、Bot、実行者の階層が低い、Bot の階層が低い）

`Messages::check_failed` は、これらを日本語の権限名つきで、本人にだけ伝えます。`permission_names(Permissions)` を公開して、アプリも同じ名前を使えるようにします。

#### 追加の通信量

| 操作 | スラッシュ | テキスト |
| --- | --- | --- |
| 実行者、Bot の権限の確認 | 0 回 | 既存と同じ取得を共有し、Bot のメンバー取得が 1 回増える |
| 他のメンバーの権限 | 1 回 | 1 回 |
| `require_can_moderate` | 対象のメンバー取得 1 回 | 同左 |
| `snapshot` | 0 回（チャンネル情報はキャッシュの範囲） | 層 1 の確認と共有 |
| `set_overwrite`、`set_role_permissions` | 現在の値の確認と、書き込みの 2 回 | 同左 |

保存の期間を指定したときは、サーバーとチャンネルの取得を、既存のキャッシュと同じ枠で共有します。

#### 書き込みの確認（送る前）

- `set_overwrite`: Bot が `MANAGE_ROLES` を持つこと、許可または拒否する権限を、Bot 自身が持つこと（管理者は除く）。持たなければ、リクエストを送らずに失敗します。
- `set_role_permissions`: Bot が `MANAGE_ROLES` を持つこと、対象のロールが Bot の最上位ロールより低いこと、付与する権限を Bot がサーバー全体で持つこと。
- 理由は、書き込みのたびに必須で、512 文字を超えたら送る前に失敗します。
- 読み取りは、キャッシュを使わずに現在の値を取得します（読んで書き戻す方式の誤りを減らすためです）。

#### 決めておく規則

- 最上位ロールが同じ位置のときは、「高くない」として拒否します（安全側に倒します）。
- 管理者権限は、ロール階層の確認を免除しません（Discord の規則と同じです）。
- 取得に失敗した、あるいは情報が足りないときは、必ず失敗します。

### テスト

階層の関数は、所有者、同位、上位、下位、Bot が下位、自分自身、`@everyone` だけのメンバーを網羅します。権限の計算は、サーバー側の上書きとスレッドを含めて、既存のテストを拡張します。

### 性能の方針

無駄なく速くするための決まりです。通信回数は、テストで固定します。

1. **通信しないで済む場合は、通信しません。** スラッシュコマンドは、Discord が実行者の権限（`member.permissions`）と Bot の権限（`app_permissions`）を送ってきます。この経路は通信 0 回です。
2. **取得を共有します。** 1 回の呼び出しの中で、宣言した確認、`can`、`snapshot` は、同じ取得結果を使い回します。
3. **必要になった時に、必要なものだけ取得します。** Bot の権限は、`.bot(..)` を宣言したか、`bot_can` を呼んだときだけ取得します。階層用のメンバー取得は、`require_can_moderate` のときだけです。
4. **独立した取得は同時に実行します。** サーバー、チャンネル、Bot のメンバーを同時に取得し、待ちを最大の 1 回分にします（今の実装は順番に取得しています）。
5. **既存のキャッシュに載せます。** 保存の期間を指定したときは、Bot のメンバー情報も同じ枠に載せ、同じ無効化と上限（512 件）を使います。
6. **計算は、割り当てなしのビット演算だけです。** 名前の列挙はイテレーター、日本語名は静的な表です。
7. **安い確認から先に行います。** サーバー内か、宣言した権限が足りているかを先に調べ、失敗したら通信せずに終わります。
8. **書き込みの前に読み直しません。** 送る前の確認には、同じ呼び出しで取得済みの Bot の権限を使います。

| 経路 | キャッシュなし | キャッシュあり |
| --- | --- | --- |
| スラッシュで実行者と Bot の権限を確認 | 0 回 | 0 回 |
| テキストで実行者と Bot の権限を確認 | 同時に 3 回（待ちは 1 回分） | 0 回 |
| `require_can_moderate` | 対象のメンバー取得 1 回を加算 | 同左 |

テストでは、モックの REST を使い、上の回数を固定します。回数が増えたら CI が落ちます。権限の計算は、割り当てが 0 であることも確かめます。

### 層 5: 管理操作（作成・編集・削除）

d.py の `create_text_channel`、`channel.edit`、`guild.edit`、`role.edit`、`member.kick`、`thread.edit` などに当たる操作です。チャンネルや VC だけでなく、サーバー設定、フォーラム、スレッド、ロール、メンバー、絵文字まで、**同じ決まりで** 扱います。Rust らしく、**種類ごとの型で、成り立たない組み合わせを書けなくします。** d.py は、テキストチャンネルにビットレートを渡しても、実行時にしかエラーになりません。

#### 対象と段階

| 段階 | 対象 | 必要な Bot の権限 | 主な事前検証 |
| --- | --- | --- | --- |
| 1 | チャンネル（作成、編集、削除）、VC のメンバー操作 | `MANAGE_CHANNELS`、`MOVE_MEMBERS`、`MUTE_MEMBERS`、`DEAFEN_MEMBERS` | 名前、トピック、スローモード、人数の範囲 |
| 1 | ロール（作成、編集、削除、位置） | `MANAGE_ROLES` | 色、名前、権限は Bot が持つものだけ、Bot の最上位ロールより下 |
| 1 | メンバー（ニックネーム、ロールの付け外し、キック、BAN、タイムアウト） | `MANAGE_NICKNAMES`、`MANAGE_ROLES`、`KICK_MEMBERS`、`BAN_MEMBERS`、`MODERATE_MEMBERS` | ロール階層、タイムアウトは 28 日以内 |
| 2 | サーバー設定（名前、認証レベル、通知、コンテンツフィルター、システムチャンネル、AFK、ルール、言語、説明） | `MANAGE_GUILD` | 名前は 2〜100 文字、列挙は Twilight の型 |
| 2 | フォーラム（タグ、既定の並び順、レイアウト、反応の絵文字、ガイドライン）と投稿の作成 | `MANAGE_CHANNELS`、投稿は `SEND_MESSAGES` | タグは 20 個まで、タグ名 20 文字、投稿に付けるタグは 5 個まで |
| 2 | スレッド（作成、名前、アーカイブ、ロック、自動アーカイブ、メンバーの追加・削除） | `CREATE_PUBLIC_THREADS`、`MANAGE_THREADS` など | 自動アーカイブは 60、1440、4320、10080 分のみ |
| 3 | 絵文字とステッカー（作成、名前、削除） | `CREATE_GUILD_EXPRESSIONS`、`MANAGE_GUILD_EXPRESSIONS` | 名前の文字種と長さ、画像の種類とサイズ |
| 3 | ステージ、予定イベント、AutoMod、Webhook、招待 | それぞれ別 | 別に設計 |

#### 統一する名前と形

- 入り口は `ctx.manage()` です。動詞は `create_*`、`edit_*`、`delete_*` に統一します（例: `create_role`、`edit_guild`、`delete_channel`）。
- 作成は `New*`（例: `NewChannel`、`NewRole`、`NewThread`）、編集は `*Edit`（例: `GuildEdit`、`RoleEdit`、`ThreadEdit`）の型を使います。どちらも、種類ごとに、設定できる項目だけを持ちます。
- 結果は、更新後の Twilight の型です（`Channel`、`Role`、`Guild` など）。
- 理由は必須で、512 文字以内です。


#### 作成

<!-- no-check -->
```rust
let channel = ctx.manage().create_channel(
    guild_id,
    NewChannel::voice("雑談")
        .user_limit(10)
        .bitrate(64_000)
        .category(category_id)
        .overwrite(Target::Role(members), Overwrite::new().deny(Permissions::CONNECT)),
    "新しい雑談用の VC",       // 監査ログの理由（必須）
).await?;
```

- 種類ごとの入り口は `NewChannel::text`、`voice`、`category`、`stage`、`forum`、`announcement` です。
- 設定できる項目は、種類ごとの型にだけあります（テキストにはトピックとスローモード、VC にはビットレートと人数、など）。
- 上書きは、層 4 の `Overwrite` と `Target` をそのまま使います。

#### 編集

<!-- no-check -->
```rust
ctx.manage().edit_channel(
    channel_id,
    ChannelEdit::new().name("新しい名前").slowmode(Duration::from_secs(10)),
    "理由",
).await?;
```

- 変更したい項目だけを指定します（指定しない項目は、変更しません）。
- 項目ごとに、名前は 1〜100 文字、トピックは 1024 文字以内、スローモードは 0〜21600 秒、人数は 0〜99（ステージは別の上限）のように、Discord が決めている範囲を、送る前に検証します。ブーストの段階に依存するビットレートの上限は、Discord の応答に任せます。
- チャンネルの種類に合わない項目（テキストのビットレートなど）は、型がなく、書けません。

#### VC とメンバーの操作

<!-- no-check -->
```rust
ctx.manage().move_member(guild_id, user_id, Some(channel_id), "理由").await?;   // None で切断
ctx.manage().set_voice_mute(guild_id, user_id, true, "理由").await?;
ctx.manage().set_voice_deafen(guild_id, user_id, true, "理由").await?;
```

#### ロールとメンバー

<!-- no-check -->
```rust
let role = ctx.manage().create_role(
    guild_id,
    NewRole::new("認証済み").color(0x2ecc71).hoist(true).permissions(Permissions::text()),
    "認証用",
).await?;

ctx.manage().add_role(guild_id, user_id, role.id, "認証が完了したため").await?;
ctx.manage().timeout_member(guild_id, user_id, Duration::from_secs(600), "スパム").await?;
ctx.manage().kick_member(guild_id, user_id, "理由").await?;
```

- メンバーとロールの操作は、層 3 のロール階層の確認を、**自動で** 行います。Discord に送れば 403 で失敗する操作を、送る前に、理由つきで失敗させます（通信を 1 回減らせます）。
- `NewRole` と `RoleEdit` に渡す権限は、Bot が持っているものだけです。持たないものを含む場合は、送らずに失敗します。
- タイムアウトは、28 日を超えると、送る前に失敗します。

#### サーバー設定

<!-- no-check -->
```rust
ctx.manage().edit_guild(
    guild_id,
    GuildEdit::new()
        .name("新しい名前")
        .verification_level(VerificationLevel::Medium)
        .default_notifications(DefaultMessageNotificationLevel::Mentions)
        .system_channel(Some(channel_id)),
    "理由",
).await?;
```

- 列挙は Twilight の型をそのまま使います（数値を書きません）。層 0 の日本語名で、エラーや確認の表示ができます。
- 画像（アイコン、バナー）は、「画像」の節の型を通します。
- 機能の追加・削除（`features`）は、Discord が管理者権限を求めるものがあるので、最初は対象外にします。

#### フォーラムとスレッド

<!-- no-check -->
```rust
// フォーラムのタグ。Discord は、タグ一覧全体を 1 回で置き換える仕様
ctx.manage().edit_forum_tags(channel_id, |tags| {
    tags.add(ForumTag::new("質問").emoji("❓"));
    tags.rename(existing_tag_id, "解決済み");
    tags.remove(old_tag_id);
}, "理由").await?;

ctx.manage().create_forum_post(
    channel_id,
    NewForumPost::new("投稿のタイトル", "最初のメッセージ").tags(&[tag_id]),
).await?;
```

- タグの一覧は、Discord が全体を置き換える仕様です。同時に別の変更があると、巻き戻すおそれがあります。`edit_forum_tags` は、**現在の一覧を読み、既存のタグを ID で維持したまま、変更を適用して書き戻します**。ドキュメントに、競合の可能性を明記します。
- タグの数、名前の長さ、投稿に付けるタグの数は、送る前に検証します。
- スレッドの `NewThread` は、メッセージから作る、公開、非公開の入り口を分けます。自動アーカイブの時間は、列挙型（`OneHour`、`OneDay`、`ThreeDays`、`OneWeek`）にして、不正な値を書けなくします。

#### 画像

アイコン、バナー、絵文字、ステッカーに使う画像は、`ImageData` 型を通します。

- 画像の種類（PNG、JPEG、GIF、WebP）をバイト列から判別し、サイズの上限（絵文字は 256 KiB、ステッカーは 512 KiB）を、送る前に検証します。
- Akari は、画像を勝手に取得しません。バイト列を渡すのは、アプリです。
- Discord に送る形式の変換（Base64）は、依存を足さずに、小さな自前の実装か、すでにある依存を使います（依存の枠は 132/140 です）。

#### 共通の決まり

- 入り口は `ctx.manage()` です。層 2〜4 の `Access` と同じ取得結果を共有します。
- 送る前に、Bot の権限を確認します（チャンネルの作成・編集は `MANAGE_CHANNELS`、移動は `MOVE_MEMBERS`、ミュートは `MUTE_MEMBERS`、スピーカーミュートは `DEAFEN_MEMBERS`）。足りなければ、通信せずに失敗します。
- 理由は必須で、512 文字以内です。
- 1 つの操作は、REST 1 回です。一括の操作は作りません。
- 編集の呼び出しには、Discord のレート制限があります（名前とトピックの変更は、チャンネルごとに厳しく制限されます）。待ちが長くなりうるので、ドキュメントに `tokio::time::timeout` で囲む例を載せます。
- 結果は、更新後の Twilight の `Channel` です。

#### 後回し

チャンネルとロールの並び替え、ステージ、予定イベント、AutoMod、Webhook、招待は、上の段階が入ってから別に設計します。

### 後回し

モデレーションの補助（`timeout_member` など）は、上の層が入ってから別に設計します。

## wait_for

### 使い方

<!-- no-check -->
```rust
let answer = ctx.wait_message(
    |m| m.author.id == author && m.channel_id == channel,
    Duration::from_secs(30),
).await;   // Result<Message, WaitError>
```

`WaitError` は、`Timeout`、`Cancelled`、`TooManyWaiters`、`NotEnabled` です。

### 制約

1. **opt-in**: `Bot::with_waiters(WaitLimits { max_pending, per_user })` を呼ぶまで無効です。未設定で呼ぶと `NotEnabled` を返します。
2. **タイムアウト必須**: 無期限の待機は書けません。
3. **上限つき**: 全体とユーザーごとの上限を超えたら `TooManyWaiters` を返します。
4. **後始末**: 待機中の Future が捨てられたら登録を外します（`Drop`）。シャットダウンでは全員に `Cancelled` を返します。
5. **高速**: 待機を（種類、チャンネル）で索引し、イベントごとの検査を、そのチャンネルの待機者だけにします。条件は同期で軽く、パニックしても Bot は止めません。
6. **規則性**: 一致したイベントは、通常のハンドラーにも渡します。待機が奪うことはありません。

### 制限と段階

- 再起動すると待機は消えます。再起動後も続けたい操作には `DynamicId` を使います。
- 第 1 段階はメッセージとリアクションです。ボタン、セレクト、モーダルは 3 秒以内の応答が必要でライフタイムも難しいので、別に設計します。

### テスト

タイムアウト、`Drop` での解除、シャットダウン、上限超過、条件のパニック、同時多数の待機でメモリが一定に収まること。
