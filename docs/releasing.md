# crates.io へ公開する前の確認

Disnana Akari のパッケージ名は `disnana-akari`、Rust コードでのクレート名は `disnana_akari` です。[MIT ライセンス](../LICENSE)を採用しています。公開準備が完了するまでは `Cargo.toml` の `publish = false` を維持します。

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
- [ ] `cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked` を通す。最小対応バージョンとして宣言した Rust 1.89 でもテストする。
- [ ] 公開先と設定を確定した後、クリーンな作業ツリーで `cargo publish --dry-run` を実行し、`.crate` からのビルドを確認する。

[crates.io への公開手順](https://doc.rust-lang.org/cargo/reference/publishing.html) · [Cargo のメタデータ仕様](https://doc.rust-lang.org/cargo/reference/manifest.html)
