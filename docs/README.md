# Disnana Akari 日本語ドキュメント

## はじめに

- [クイックスタート](quickstart.md) — Discord の設定から `!ping` と `/ping` の確認まで
- [チュートリアル](tutorial.md) — Rust の文法、共通の `echo` ハンドラー、埋め込み、Components V2、入力フォームの作り方
- [実行できるサンプル](../examples/ping/main.rs) — コマンドをモジュールにまとめた Bot

## リファレンス

- [API リファレンス](api.md) — 公開型、Intent、スラッシュコマンドの同期と遅延応答、ハンドラーの実行順
- [トラブルシューティング](troubleshooting.md) — 起動、Gateway 接続、返信のエラー

型やメソッドを検索したい場合は、リポジトリのルートで次のコマンドを実行すると rustdoc が開きます。

```sh
cargo doc --locked --no-deps --open
```

## crates.io で公開する前に確認すること

- `Cargo.toml` のパッケージ名、バージョン、説明、リポジトリなど、公開用メタデータを確認します。
- ライセンスを決定し、ライセンス文書と `license` または `license-file` の設定を揃えます。
- 変更履歴に最初のリリースの機能と互換性を記載します。
- CI で整形、Clippy、テストを確認します。公開可否の設定を整えた後に `cargo package --list` と `cargo publish --dry-run` で配布ファイルとビルドを確認します。
