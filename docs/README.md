# Disnana Akari 日本語ドキュメント

## はじめに

- [クイックスタート](quickstart.md) — Discord 側の設定から Ping Bot の起動まで
- [チュートリアル](tutorial.md) — `!ping` と `!echo` を個別の関数として登録する
- [実行できるサンプル](../examples/ping/main.rs) — コマンドをモジュールにまとめた Bot

## リファレンス

- [API リファレンス](api.md) — `Bot`、`Context`、`Result` の挙動と実行モデル
- [トラブルシューティング](troubleshooting.md) — 起動、Gateway 接続、返信のエラー

型やメソッドを検索したい場合は、リポジトリのルートで次のコマンドを実行すると rustdoc が開きます。

```sh
cargo doc --locked --no-deps --open
```
