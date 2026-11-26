# ドキュメントサイト

[VitePress](https://vitepress.dev/) で作っています。パッケージ管理は [Bun](https://bun.sh/) です（VitePress 自体は Node で動きます）。原稿は `content/` の Markdown です。

```sh
cd site
bun install --frozen-lockfile --ignore-scripts
cp ../CHANGELOG.md content/reference/changelog.md   # 変更履歴（生成物なので Git には入れません）
bun run dev                                          # http://localhost:5173/Disnana-Akari/
```

- コードブロックのうち ` ```rust ` のものは、`python3 site/tools/check_snippets.py` で実際にコンパイルして確認します（CI でも実行）。確認から外すときは、ブロックの直前に `<!-- no-check -->` を書きます。
- API リファレンスは rustdoc です。公開時に `cargo doc` の出力を `/api/` に置きます。

## 依存パッケージ

- ロックファイルは `bun.lock` です。CI は `--frozen-lockfile` で、ロックどおりに入れます。
- `bun audit` が毎日実行されます（`.github/workflows/site-audit.yml`）。
- `package.json` の `overrides` で `vite` を 7 系に固定しています。VitePress 1.6 が使う `vite` 5 系には、開発サーバーの既知の脆弱性があり、修正版が出ないためです。VitePress が `vite` の新しい版に追従したら、外せます。
- `bunfig.toml` で、公開から 3 日以内の版は入れないようにしています。
