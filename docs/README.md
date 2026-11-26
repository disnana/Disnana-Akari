# Disnana Akari 日本語ドキュメント

## はじめに

- [クイックスタート](quickstart.md) — Discord の設定から `!ping` と `/ping` の確認まで
- [チュートリアル](tutorial.md) — Rust の文法、共通の `echo` ハンドラー、埋め込みとボタンの作り方
- [実行できるサンプル](../examples/ping/main.rs) — コマンドをモジュールにまとめた Bot

## リファレンス

- [API リファレンス](api.md) — 公開型、Intent、スラッシュコマンドの同期と遅延応答、ハンドラーの実行順
- [トラブルシューティング](troubleshooting.md) — 起動、Gateway 接続、返信のエラー

型やメソッドを検索したい場合は、リポジトリのルートで次のコマンドを実行すると rustdoc が開きます。

```sh
cargo doc --locked --no-deps --open
```
