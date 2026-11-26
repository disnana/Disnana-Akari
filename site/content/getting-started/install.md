# インストール

## Rust を用意する

Akari には Rust 1.89 以上が必要です。[rustup](https://rustup.rs/) でインストールできます。

```sh
rustup update stable
rustc --version
```

## プロジェクトを作る

新しい Cargo プロジェクトを作り、Akari と Tokio を追加します。

```sh
cargo new my-bot
cd my-bot
cargo add disnana-akari
cargo add tokio --features macros,rt
```

`Cargo.toml` の `[dependencies]` が次のようになっていれば準備完了です（バージョンは異なることがあります）。

```toml
[dependencies]
disnana-akari = "0.1"
tokio = { version = "1", features = ["macros", "rt"] }
```

> [!NOTE]
> Tokio は Rust で非同期処理を動かすためのライブラリです。`#[tokio::main]` を使うために `macros` と `rt` が必要です。時間待ちなど `tokio::time` を使う場合は `time` も追加します。

## 次のステップ

Bot を動かすには、Discord 側にもアプリケーションを作る必要があります。[Discord で Bot を用意する](discord-setup.md) へ進みましょう。
