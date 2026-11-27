# ログを出す

Akari は、起きたことを **`tracing` のイベント** として発行します。画面やファイルのどこに、どのレベルから出すかは、**アプリが決めます**。何も設定しなければ、何も出ません（Akari が勝手に出力することはありません）。

## 画面に出す

`tracing-subscriber` を、アプリの依存に足して、起動の最初に 1 行呼びます。

<!-- no-check -->
```rust
#[tokio::main(flavor = "current_thread")]
async fn main() -> disnana_akari::Result<()> {
    // RUST_LOG で、見たいレベルを選べます（例: RUST_LOG=disnana_akari=debug）
    tracing_subscriber::fmt::init();

    // ... Bot を起動する
    Ok(())
}
```

`tracing-subscriber` を使いたくないときは、`tracing` に対応した別の購読先（ファイル出力、JSON など）を、自由に選べます。

## どのレベルで出るか

イベントの `target` は `disnana_akari` です。

| レベル | 出るもの |
| --- | --- |
| `error` | ハンドラーが返したエラー、Gateway の切断と受信のエラー |
| `warn` | 処理枠の満杯で捨てたこと、自動の応答（エラー文言など）の失敗 |
| `info` | Gateway のセッションの準備と再開、スラッシュコマンドの同期 |
| `debug` | Interaction への初回応答（かかった時間つき） |

`error` と `warn` には、`source`（場所）と `name`（コマンドやボタンの名前）が付きます。

## `on_error`、`on_info` との使い分け

| | 用途 |
| --- | --- |
| `tracing` のログ | 人が読む記録。出力先とレベルを、アプリが決めます |
| `Bot::on_error`、`Bot::on_info` | **コードで反応する** ためのもの。たとえば、エラーを Discord のチャンネルに送る、回数を数える、など |

どちらも同時に使えます。`on_error` を設定しても、`tracing` のログは変わらず出ます。

> [!NOTE]
> 0.1 は、`on_error` を設定しないと、エラーを標準エラー出力に書いていました。0.2 では書かなくなりました。エラーを見たいときは、上の 1 行を足してください。
