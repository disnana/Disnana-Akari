# エラーと自動で送る文言

Akari は、頼まれていないことを勝手にしません。標準出力には何も書かず、エラーは **あなたが受け取る** 形にしています。

## ハンドラーが返せるエラー

ハンドラーは、`Result<(), E>` を返せます。`E` は、標準のエラー、自作のエラー型、`Box<dyn Error + Send + Sync>` など、何でもかまいません。`?` をそのまま使えます。

```rust
use disnana_akari::CommandContext;

// 標準のエラー（ParseIntError）と、Akari のエラー（reply の失敗）の両方に ? を使える
async fn double(ctx: CommandContext<'_>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let n: i64 = ctx.args().parse()?;
    ctx.reply(&(n * 2).to_string()).await?;
    Ok(())
}
```

```rust
use disnana_akari::{CommandContext, Error, Result};

// メッセージだけのエラーは Error::msg
async fn echo(ctx: CommandContext<'_>) -> Result<()> {
    if ctx.args().is_empty() {
        return Err(Error::msg("使い方: !echo 文章"));
    }
    ctx.reply(ctx.args()).await
}
```

- `Result<()>`（Akari の `Error`）を返すハンドラーは、Akari のエラーを `?` でそのまま伝えられます。
- ハンドラーが返したエラーは、`Bot::on_error` に報告されます。標準のエラーなどは、`Error::Handler` に包まれます。元のエラーは、`HandlerError` の `downcast_ref::<T>()` などで取り出せ、表示と原因の連鎖は、元のエラーと同じです。
- **クロージャ**は、末尾が関数呼び出しなら、そのまま渡せます。`Ok(())` で終わるときは、戻り値の型を書きます。書かないと、`type annotations needed` になります。

```rust
use disnana_akari::{CommandContext, Commands, Result};

fn register(commands: &mut Commands) -> Result<()> {
    commands.register("ping", async move |ctx: CommandContext<'_>| -> Result<()> {
        ctx.reply("Pong!").await?;
        Ok(())
    })
}
```

## エラーを受け取る

`Bot::on_error` に関数を渡すと、Akari の内部で起きたエラーを、一件ずつ受け取れます。

```rust
use disnana_akari::{Bot, Result};

fn build() -> Result<Bot> {
    Ok(Bot::from_env()?.on_error(|report| {
        eprintln!("[bot] {report}");
    }))
}
```

受け取る `report` には、次の情報があります。

| 項目 | 内容 |
| --- | --- |
| `report.source` | エラーが起きた場所。`TextCommand`、`SlashCommand`、`Button`、`Select`、`Modal`、`Message`、`Event`、`Gateway`、`Response`（自動応答の失敗）、`QueueFull`（処理枠が満杯）など |
| `report.name` | コマンド名、`custom_id`、イベント名など、原因を絞り込む手がかり。分からないときは `None` |
| `report.error` | `&Error`。ハンドラーが返した型付きエラーをそのまま渡します |

```rust
use disnana_akari::{Bot, ErrorSource, Result};

fn build() -> Result<Bot> {
    Ok(Bot::from_env()?.on_error(|report| match report.source {
        // 処理枠が満杯のときは、件数を数えるだけにする
        ErrorSource::QueueFull => {}
        // それ以外は、場所と名前つきで記録する
        _ => eprintln!("[bot] {} {:?}: {}", report.source, report.name, report.error),
    }))
}
```

- **Akari は、標準出力にも標準エラー出力にも書きません。** エラーは `tracing` のログとして発行します（[ログを出す](logging.md)）。この関数は、ログとは別に、コードで反応するためのものです。
- 関数は処理の途中で **同期的に** 呼ばれます。短く保ち、`panic` しないでください。重い処理は、チャンネルへ送るなどして、別のタスクで行います。
- `ErrorSource` は、今後増える可能性があります（`#[non_exhaustive]`）。`match` には `_` の分岐を付けてください。
- `{report}` で表示すると、場所、名前、エラーに続けて、**原因の連鎖も** `: 原因` の形で表示します（たとえば通信のエラーでは、本当の理由は原因の側にあります）。
- ロガーへの出力は、この関数がなくても、`tracing` を通して行われます。[ログを出す](logging.md)を見てください。

### アプリ固有のエラー

ハンドラーが返す `Result<T>` のエラー型は `disnana_akari::Error` です。Akari の操作にはそのまま `?` を使えます。アプリ側のエラーは `Error::handler` を使うと、原因を保持したまま `Error::Handler` に包めます。

```rust
use disnana_akari::{CommandContext, Error, Result};

async fn count(ctx: CommandContext<'_>) -> Result<()> {
    let count = ctx.args().parse::<u32>().map_err(Error::handler)?;
    ctx.reply(&format!("個数は {count} です。")).await
}
```

`Box<dyn std::error::Error + Send + Sync>` は `?` で `Error::Handler` に変換できます。`on_error` では `report.error` を `match` し、`Error::Handler` や Gateway の終了コードを持つ `Error::GatewayClose` などを判別できます。`Error` にバリアントが増える可能性があるため、`_` の分岐を付けてください。

### Gateway の切断

Gateway が接続を閉じたときも、理由つきで報告されます。

| コード | 報告される内容 |
| --- | --- |
| `4004` | Bot トークンが正しくありません。 |
| `4014` | 許可されていない Intent が要求されました。Developer Portal で、特権 Intent を有効にしてください。 |

## 自動で送る文言

Akari は、Discord の決まりを満たすために、いくつかの場面で利用者へメッセージを送ります。`Bot::with_messages` で、文言を変えたり、送らないようにしたりできます。

```rust
use disnana_akari::{Bot, Messages, Result};

fn build() -> Result<Bot> {
    // Messages は、今後項目が増える可能性があります。Default から始めて、変えたい項目だけ代入します。
    let mut messages = Messages::default();
    messages.interaction_busy = Some("Only a moment, please.".to_owned());
    messages.text_command_failed = Some("Something went wrong.".to_owned());
    messages.modal_accepted = None; // 何も送らない
    Ok(Bot::from_env()?.with_messages(messages))
}
```

| 項目 | 送る場面 | 既定 |
| --- | --- | --- |
| `text_command_failed` | テキストコマンドの関数がエラーを返したとき（元の投稿へ返信） | **送らない** |
| `text_command_busy` | テキストコマンドの処理枠が満杯のとき（元の投稿へ返信） | **送らない** |
| `interaction_busy` | スラッシュ、ボタン、選択メニュー、フォームの処理枠が満杯のとき（本人にだけ） | 「現在混雑しています。…」 |
| `slash_command_failed` | スラッシュコマンドの関数がエラーを返したか、応答しなかったとき | 「コマンドを処理できませんでした。」 |
| `slash_command_unavailable` | Akari に登録していないスラッシュコマンドが実行されたとき（本人にだけ） | 「このコマンドは現在利用できません。」 |
| `check_failed` | `require_*` の確認に通らず、ハンドラーが [`CheckFailure`](permissions.md) を返したとき（スラッシュでは本人にだけ、テキストでは元の投稿への返信） | 「このコマンドを使う権限がありません。」 |
| `button_failed` | ボタンの関数がエラーを返したとき（本人にだけ） | 「ボタンを処理できませんでした。」 |
| `select_failed` | 選択メニューの関数がエラーを返したとき、または選択内容を読み取れなかったとき（本人にだけ） | 「選択内容を処理できませんでした。」 |
| `modal_failed` | フォームの関数がエラーを返したとき（本人にだけ） | 「入力内容を処理できませんでした。」 |
| `modal_accepted` | フォームの関数が、成功したのに応答しなかったとき（本人にだけ） | 「入力を受け付けました。」 |

> [!NOTE]
> テキストコマンドには、応答の決まりがないので、既定では何も送りません。一方、スラッシュコマンド、ボタン、選択メニュー、フォームは、Discord が **受信後 3 秒以内の応答** を求めます。そのため、これらには既定の文言があります。`None` にすると、応答が済んでいない操作は、Discord 上で失敗と表示されます。

文言を `None` にしても、エラー自体は `on_error` に報告されます。

## 暗号プロバイダー（TLS）

Akari は、プロセス全体の暗号プロバイダー（`rustls` の `CryptoProvider`）を登録しません。`rustls` が、最初の使用時に自動で選びます。アプリで別のプロバイダーも有効にする場合は、起動前に、アプリ側で登録してください。
