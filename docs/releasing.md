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
