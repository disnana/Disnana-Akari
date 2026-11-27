# 0.4 以降の設計メモ

discord.py と比べて不便な点を洗い出し、Akari の方針（シンプル、規則的、高速、安定、安全）を守ったまま埋める順番と設計をまとめます。**ここに書いた API はまだ実装していません。** 実装するときに、この文書と食い違う点があれば、先にこの文書を直します。

## 守る方針

- 公開 API は追加だけにします。`cargo-semver-checks` で既存の API が壊れていないかを確かめます。
- 勝手に確認や処理をしません。呼んだとき、または登録で宣言したときだけ動きます。
- 状態を持つ機能は opt-in にして、上限、タイムアウト、後始末を必須にします。
- 取得できない、判断できないときは、許可せずに失敗します。
- 公開した型は文書に載せます（載せないと CI が落ちます）。例のコードは、文書の検査でコンパイルします。
- 実機でしか確かめられない部分は「実機未確認」と書きます。

## 実装の順番

| 段階 | 内容 | d.py での対応 |
| --- | --- | --- |
| 0.4.0 | 権限の補助（下の「権限の補助」） | `has_permissions`、`bot_has_permissions`、`permissions_for`、`top_role` |
| 0.4.0 | 引数の型変換をテキスト、ハイブリッド、スラッシュで共通にする（`Greedy`、`Optional`、範囲、フラグを含む） | `Converter` |
| 0.4.0 | サーバー・チャンネル・ロールの opt-in キャッシュ。依存の枠（132/140）を見て、自前の小さな実装を先に検討する | `bot.get_channel`、`guild.get_role` |
| 0.4.0 | `wait_for`（メッセージ、リアクション） | `bot.wait_for` |
| 0.5.0 | `Member`、`Role`、`Channel`、`User` の変換（メンション、ID、名前）。キャッシュがなければ上限つきで REST を使う | `MemberConverter` など |
| 0.5.0 | `wait_for` のボタン、セレクト、モーダル。UI 補助（実行者だけ操作、タイムアウトで無効化） | `View.interaction_check`、`on_timeout` |
| 0.5.0 | 確認、ページ送りの例を `examples/` に置く（本体には入れない） | `ext.menus` |
| 0.6.0 | コマンド単位のエラー処理、メッセージの多言語化 | `@cmd.error`、`on_command_error` |
| 0.6.0 | 性能ベンチを CI に載せ、遅くなったら落とす | なし |

## 権限の補助

### 現状と不便な点

- `ctx.permissions()` と `ctx.require_permissions(..)` は、実行者の権限しか扱えません。
- チャンネルの上書きやロールの権限を、読んで変更する手段がありません（Twilight の型を直接組み立てます）。
- Bot 自身の権限、別のメンバーの権限、ロール階層、サーバーの所有者、ロールの所持は調べられません。
- 毎回 `.await?` と `contains` を書く必要があります。足りない権限を名前で取り出す方法がありません。

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

### テスト

階層の関数は、所有者、同位、上位、下位、Bot が下位、自分自身、`@everyone` だけのメンバーを網羅します。権限の計算は、サーバー側の上書きとスレッドを含めて、既存のテストを拡張します。

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
