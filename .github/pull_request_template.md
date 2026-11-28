## 概要

<!-- 何を、なぜ変えたか。 -->

## 確認

- [ ] `cargo fmt --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked` が通る
- [ ] 公開 API を変えた場合、`CHANGELOG.md` に書いた（互換性を壊す変更は、移行の手引きにも）
- [ ] 新しい挙動には、テストがある（バグの修正には、再現するテスト）
- [ ] 新しい `#[allow]` には、`reason` を書いた
- [ ] 新しい依存を足した場合、`.github/dependencies.toml` に理由を書いた
- [ ] ガイドや API 一覧の説明を更新した
- [ ] 権限に関わる変更では、実行者と Bot の権限を、操作対象の範囲で確認している
- [ ] 実際の Discord でしか確認できないことは、PR に書いた（`docs/releasing.md` の確認項目）
