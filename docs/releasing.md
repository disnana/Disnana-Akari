# crates.io へ公開する前の確認

Disnana Akari のパッケージ名は `disnana-akari`、Rust コードでのクレート名は `disnana_akari` です。[MIT ライセンス](../LICENSE)を採用しています。`Cargo.toml` の `publish = ["crates-io"]` は公開先を crates.io に限定します。

## 配布内容を確認する

リポジトリのルートで、実際にパッケージへ含まれるファイルを一覧できます。手元の未コミット差分を調べる場合だけ `--allow-dirty` を追加します。

```sh
cargo package --offline --list
```

`Cargo.toml` の `include` はライブラリのソース、サンプル、ドキュメント、変更履歴、ライセンス関連文書を対象にします。Cargo は `Cargo.toml` と `Cargo.lock` も含めます。公開前に一覧を確認し、`playground/`、`.env`、テスト環境の認証情報が含まれていないことを確かめます。[Cargo のパッケージング仕様](https://doc.rust-lang.org/cargo/commands/cargo-package.html)

## リリース候補を確認する

- [ ] crates.io で `disnana-akari` を利用できることと、公開するアカウントを確認する。
- [ ] 配布ファイル一覧に `LICENSE` があり、`Cargo.toml` で `license = "MIT"` が設定されていることを確認する。
- [ ] パッケージのバージョンと [変更履歴](../CHANGELOG.md) を揃え、公開 API と互換性に関わる変更を記載する。
- [ ] GitHub Actions で整形、Clippy、Linux・Windows のテスト、最小対応 Rust での配布パッケージのビルドを確認する。
- [ ] `cargo-deny` によるライセンス・重複・取得元の検査（`deny.toml`）が通ることを確認する。新しい依存でライセンスが増えたら、内容を確認したうえで `deny.toml` の許可リストに追加する。
- [ ] RustSec による `Cargo.lock` の監査を確認する。監査は pull request、`main` への push、毎日の定期実行で最新のアドバイザリを取得します。脆弱性と取り下げられた依存バージョンは CI を失敗させます。
- [ ] 公開先と設定を確定した後、クリーンな作業ツリーで `cargo publish --dry-run` を実行し、`.crate` からのビルドを確認する。

## 公開前に、実際の Discord で確認すること（0.2.0 の記録）

リリース候補をテスト用 Bot とサーバーで起動し、結果を記録します。ローカルの Playground の手順は `playground/SMOKE.md` にあります。通常の確認は RustRover の **Akari Playground** に `--skip-replace` を付けて始めます。

### 確認済み

- [x] `bd12867` の修正を Discord で確認。`/akari-check deferred-denial` の公開の待機表示は「処理を終了しました。」に変わり、権限拒否の文言は実行者にだけ返った。
- [x] `--missing-intent` は接続前に `MESSAGE_CONTENT` の不足を報告する。これはローカルで確認済み。
- [x] 候補 `9094b11` で Gateway の Ready、ギルドコマンド9件の同期、`!ping` と `/ping` の応答を確認した。
- [x] `/akari-check typed` の補完と型付き引数、`/akari-check advanced ping` の非公開応答を確認した。
- [x] 管理者は `!akari-perms`、`/akari-perms`、`!akari-bancheck` を実行でき、`/akari-admin` も表示され非公開で応答した。
- [x] `!akari-notify` の返信だけが投稿者に通知した。`/ui` の即時・遅延更新と、`/components`、`/entities`、`!owner` の非公開応答を確認した。
- [x] `--one-worker` では `!slow` 中の2件を混雑として拒否し、終了後に `!ping` は通常応答へ戻った。`--include-bots` でも自身の投稿には反応しなかった。
- [x] グローバル登録後、`/akari-scope` はサーバーと Bot の DM で応答し、DM の `/akari-guild-only` は拒否された。
- [x] ギルドの一括置換では試験用の1件が消え、通常の9件とグローバルの2件が残った。
- [x] 特権 Intent を無効にして起動すると `Bot::on_error` に `4014` と対処が報告された。有効に戻すと Gateway に再接続できた。

### 一般メンバーの権限試験

専用テストサーバーで、オーナーでも管理者でもないメンバーを使って確認しました。

- [x] `!akari-perms` と `/akari-perms` は初めに拒否され、テスト用ロールに `MANAGE_MESSAGES` を付けると通った。スラッシュの拒否は実行者にだけ返った。
- [x] ロールに `MANAGE_MESSAGES` がある状態で、テストチャンネルの上書きで拒否すると、チャンネルと配下のスレッドの両方で拒否された。上書きを解除すると通った。
- [x] `!akari-bancheck` は `BAN_MEMBERS` の有無で結果が変わった。チャンネルの `MANAGE_MESSAGES` の上書きは `BAN_MEMBERS` の判定に影響しなかった。
- [x] `MANAGE_GUILD` がない場合は `/akari-admin` が表示されず、権限を付けると表示された。
- [x] `--cache-permissions` で `!akari-perms` を実行し、権限変更直後はキャッシュ済みの結果が返り、期限切れ後に変更が反映されることを確認した。

`--include-bots` で別の Bot と Webhook の投稿から `!ping` が起動するかは未確認です。

## 0.4.0 候補を Discord で確認する

候補コミットと結果を記録します。テストには専用 Bot、専用サーバー、使い捨てのチャンネルと Webhook を使います。

- [ ] 通常起動でギルドコマンドの同期とテキスト・スラッシュの返信を確認する。
- [ ] 管理者と一般メンバーで権限チェックを試す。公開の `defer()` 後の拒否文が実行者だけに届くことを確認する。
- [ ] 投票を公開し、複数選択、投票者取得、投票終了を確認する。公開の `defer()` 後は元の応答に投票が付くことを確認する。
- [ ] Webhook でスレッドに投稿し、同じメッセージを取得・編集・削除する。編集では添付を保持して追加できることを確認する。
- [ ] Akari を対象とした推奨のログ設定で試行し、Webhook トークンがログに含まれないことを確認する。

### 管理操作を確認する

ローカル HTTP テストで、別サーバーのチャンネルへの変更が送信されないこと、同じ位置のロールが ID 順になること、BAN で対象の Unknown Member だけを特例にすること、VC 操作が Discord の拒否を返すことを確認します。

Discord 実機の結果はローカルの結果と区別して記録します。実機では専用サーバーと試験用アカウントを使い、作成したロール、チャンネル、タイムアウト、BAN を試験後に元へ戻します。

- [ ] 同じ位置の異なるロールと、まったく同じロールで階層の判定を比較する。
- [ ] 管理者に対するタイムアウトの拒否と、一般メンバーの設定・解除を確認する。
- [ ] Bot の対象 VC に対する権限を変え、テキスト経由で移動・切断・ミュート・スピーカーミュートを確認する。
- [ ] チャンネルの上書きと、試験用チャンネル・ロールの作成、編集、削除を確認する。

## 公開 API の互換性を確認する

Rust CI の `Public API compatibility` は、公開済みのタグ `v0.3.0` と現在の公開 API を比較します。新しい API の追加を許し、既存の公開関数や型の削除などの互換性を壊す変更は検出します。

同じ検査を手元で実行する場合は、Rust 1.90.0 と cargo-semver-checks 0.46.0 を使います。Akari の最小対応 Rust は、別のパッケージ検証ジョブで確認します。

```sh
cargo +1.90.0 semver-checks check-release --baseline-rev v0.3.0 --release-type minor
```

互換性の基準を次の公開版へ移す場合は、CI のタグとこの手順を一緒に変更します。[cargo-semver-checks の公式ドキュメント](https://github.com/obi1kenobi/cargo-semver-checks)

## 初回公開の認証

公開に使う GitHub アカウントで crates.io にログインし、アカウントのメールアドレスを確認済みにします。[crates.io のアカウント設定](https://crates.io/settings/profile)

[New API Token](https://crates.io/settings/tokens/new) で `akari-0.1.0` などの名前を付け、有効期限に `7 days` を選びます。`Scopes` では `publish-new` だけを選び、`Crates` に `disnana-akari` を完全一致で追加します。

手元のターミナルで次を実行し、プロンプトが出たらトークンを貼り付けます。トークンはコマンドの引数やリポジトリのファイルに入れません。[`cargo login` の仕様](https://doc.rust-lang.org/cargo/commands/cargo-login.html)

```sh
cargo login --registry crates-io
```

公開するコミットのリポジトリルートで、先に dry-run を通します。

```sh
cargo publish --dry-run --locked --registry crates-io
```

dry-run が成功したら、次のコマンドで公開します。

```sh
cargo publish --locked --registry crates-io
```

[`disnana-akari` 0.1.0 のページ](https://crates.io/crates/disnana-akari/0.1.0)で公開を確認します。初回公開に使ったトークンが不要になったら、crates.io の [API Tokens](https://crates.io/settings/tokens) で失効させ、`cargo logout --registry crates-io` で手元の保存分も削除します。

## バージョンタグで公開する

初回公開後は crates.io のクレート設定から GitHub の Trusted Publisher を登録します。Repository owner は `disnana`、Repository name は `Disnana-Akari`、Workflow filename は `publish.yml`、Environment name は `crates-io` です。GitHub の同名環境では `v*` タグを許可します。タグルールでは `v*` の作成・変更・削除をリポジトリ管理者に限定しています。[crates.io のクレート設定](https://crates.io/crates/disnana-akari/settings) · [GitHub の環境設定](https://github.com/disnana/Disnana-Akari/settings/environments)

次の版では `Cargo.toml` と `Cargo.lock` のバージョンを更新し、変更履歴を揃えて `main` に反映します。公開対象のコミットに、その版と一致する `v` 付きタグを作って push します。たとえば、`0.4.0` を公開する場合は次のように実行します。

```sh
git tag -a v0.4.0 -m "Release v0.4.0"
git push origin refs/tags/v0.4.0
```

GitHub Actions はタグ名と `Cargo.toml` のバージョン、公開対象コミットが `main` の履歴に含まれることを確認します。CI と RustSec 監査の成功後、GitHub の OIDC で取得した短期トークンを使って公開します。Actions の **Run workflow** では検証だけ実行できます。[Publish to crates.io](https://github.com/disnana/Disnana-Akari/actions/workflows/publish.yml) · [Trusted Publishing の公式ガイド](https://crates.io/docs/trusted-publishing)

crates.io への公開後、GitHub Release に同じ `.crate`、SHA-256 チェックサム、GitHub Actions が署名した来歴証明を添付します。リリースノートには前版からのコミット、マージされた PR、コントリビューターを掲載します。署名の照合方法は各リリースに記載します。[最新リリース](https://github.com/disnana/Disnana-Akari/releases/latest) · [GitHub の来歴証明](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations)

[crates.io への公開手順](https://doc.rust-lang.org/cargo/reference/publishing.html) · [Cargo のメタデータ仕様](https://doc.rust-lang.org/cargo/reference/manifest.html)
