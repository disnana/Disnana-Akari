# セキュリティポリシー

## 脆弱性の報告

脆弱性を見つけた場合は、公開の Issue にせず、GitHub の [Private vulnerability reporting](https://github.com/disnana/Disnana-Akari/security/advisories/new) から報告してください。Bot のトークンなどの認証情報は、報告に含めないでください。

## 対象

最新の `main` と、crates.io で公開した最新のバージョンを対象にします。

## 自動検査

- RustSec による `Cargo.lock` の監査(毎日と、pull request、`main` への push)
- `cargo-deny` によるライセンス、重複、取得元の検査
- CodeQL による Rust とワークフローの解析
- pull request での依存関係の変更の確認
- zizmor によるワークフローの検査
- Dependabot による依存関係と GitHub Actions の更新
