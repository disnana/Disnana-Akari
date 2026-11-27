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

## 0.2.0 の前に、実際の Discord で確認すること

リリース候補をテスト用 Bot とサーバーで起動し、結果を記録します。ローカルの Playground の手順は `playground/SMOKE.md` にあります。通常の確認は RustRover の **Akari Playground** に `--skip-replace` を付けて始めます。

### 確認済み

- [x] `bd12867` の修正を Discord で確認。`/akari-check deferred-denial` の公開の待機表示は「処理を終了しました。」に変わり、権限拒否の文言は実行者にだけ返った。
- [x] `--missing-intent` は接続前に `MESSAGE_CONTENT` の不足を報告する。これはローカルで確認済み。

### 通常の起動で確認する項目

- [ ] Gateway の Ready とギルドコマンドの同期後、`!ping` と `/ping` が返る。ライブラリから標準出力に診断が出ない。
- [ ] `/akari-check typed` で `color` に `bl` を入れると `blue` が出る。一致しない入力では候補が空になる。`count:3`、`ratio:1.5`、`enabled:True` が本人にだけ返る。`/akari-check advanced ping` ではネストした経路が返る。
- [ ] `!akari-perms` と `/akari-perms` で、一般ユーザーと `MANAGE_MESSAGES` を付けたユーザーを比較する。スラッシュの拒否は本人にだけ届く。チャンネルの拒否設定、配下のスレッド、オーナーと管理者の判定も確認する。
- [ ] `!akari-bancheck` で `BAN_MEMBERS` の有無が反映される。`/akari-admin` は `MANAGE_GUILD` を持つ人に表示され、持たない人には表示されないことを確認する。
- [ ] `!ping` と `!akari-notify` を比較し、後者だけが元の投稿者を通知対象にする。
- [ ] `/ui` の Ping、即時更新と遅延更新、`/components` のセレクトとモーダル、`/entities` のユーザー・ロール・チャンネルと混在選択、`!owner` の投稿者確認を一通り試す。

### 起動設定を切り替えて確認する項目

- [ ] `--cache-permissions` で権限キャッシュを作った後、ロールの権限やチャンネルの上書きを変更する。直後はキャッシュされた判定が返り、20 秒経過後には変更が反映される。
- [ ] `--one-worker` で `!slow` の待機中に `!ping` を 2 件以上送る。枠を超えた分に混雑の返信が出る。
- [ ] `--include-bots` で別の Bot と Webhook の投稿から `!ping` を呼べる。人が `!echo !ping` を送ったとき、Playground 自身の返信からコマンドが起動しない。
- [ ] `--global` は専用のテスト Bot で試す。反映後に別サーバーと DM から `/akari-scope` を呼ぶ。DM の `/akari-guild-only` では本人にだけ拒否が返り、診断で `GuildOnly` を確認できる。
- [ ] 使い捨てのテストサーバーで、一覧外のテストコマンドを作り、`DISCORD_REPLACE_GUILD_COMMANDS=true` を指定して起動する。登録対象だけが残る。
- [ ] 専用のテスト Bot でポータル側の特権 Intent を無効にした状態で要求し、`Bot::on_error` が Gateway のコード 4014 と対処を報告することを確認する。

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

次の版では `Cargo.toml` と `Cargo.lock` のバージョンを更新し、変更履歴を揃えて `main` に反映します。公開対象のコミットに、その版と一致する `v` 付きタグを作って push します。たとえば、`0.1.1` を公開する場合は次のように実行します。

```sh
git tag -a v0.1.1 -m "Release v0.1.1"
git push origin refs/tags/v0.1.1
```

GitHub Actions はタグ名と `Cargo.toml` のバージョン、公開対象コミットが `main` の履歴に含まれることを確認します。CI と RustSec 監査の成功後、GitHub の OIDC で取得した短期トークンを使って公開します。Actions の **Run workflow** では検証だけ実行できます。[Publish to crates.io](https://github.com/disnana/Disnana-Akari/actions/workflows/publish.yml) · [Trusted Publishing の公式ガイド](https://crates.io/docs/trusted-publishing)

crates.io への公開後、GitHub Release に同じ `.crate`、SHA-256 チェックサム、GitHub Actions が署名した来歴証明を添付します。リリースノートには前版からのコミット、マージされた PR、コントリビューターを掲載します。署名の照合方法は各リリースに記載します。[最新リリース](https://github.com/disnana/Disnana-Akari/releases/latest) · [GitHub の来歴証明](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations)

[crates.io への公開手順](https://doc.rust-lang.org/cargo/reference/publishing.html) · [Cargo のメタデータ仕様](https://doc.rust-lang.org/cargo/reference/manifest.html)
