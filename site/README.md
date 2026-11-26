# ドキュメントサイト

[VitePress](https://vitepress.dev/) で作っています。原稿は `content/` の Markdown です。

```sh
cd site
npm ci --ignore-scripts
cp ../CHANGELOG.md content/reference/changelog.md   # 変更履歴（生成物なので Git には入れません）
npm run dev                                          # http://localhost:5173/Disnana-Akari/
```

- コードブロックのうち ` ```rust ` のものは、`python3 site/tools/check_snippets.py` で実際にコンパイルして確認します（CI でも実行）。確認から外すときは、ブロックの直前に `<!-- no-check -->` を書きます。
- API リファレンスは rustdoc です。公開時に `cargo doc` の出力を `/api/` に置きます。
