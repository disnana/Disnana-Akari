# ステータスとアクティビティ

Bot のオンライン状態（オンライン、離席中など）と、「〇〇をプレイ中」のようなアクティビティは、`PresenceHandle` で設定します。起動時の設定と、実行中の変更の両方に使えます。Akari が勝手に設定することはありません。

## 設定する

`PresenceHandle` を作り、クローンを `Bot::with_presence` に渡します。ハンドルは、ハンドラーなどに持たせて、実行中に使い回せます。

```rust
use disnana_akari::model::gateway::presence::{ActivityType, MinimalActivity, Status};
use disnana_akari::{Bot, Presence, PresenceHandle, Result};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let activity = MinimalActivity {
        kind: ActivityType::Playing,
        name: "Akari".to_owned(),
        url: None,
    };
    let presence = PresenceHandle::new(Presence::new(Status::Online).with_activity(activity));

    // 実行中に変えたいときは、ハンドルのクローンをハンドラーに持たせます
    let _updates = presence.clone();

    Bot::from_env()?.with_presence(presence).start().await
}
```

## 実行中に変える

| メソッド | 動作 |
| --- | --- |
| `handle.set(presence)` | 状態とアクティビティを、まとめて置き換えます |
| `handle.set_status(status)` | オンライン状態だけを変えます。アクティビティは残ります |
| `handle.set_activity(Some(activity))` | アクティビティだけを変えます。`None` で消します |

```rust
use disnana_akari::model::gateway::presence::Status;
use disnana_akari::{Presence, PresenceHandle};

fn go_idle(handle: &PresenceHandle) {
    handle.set_status(Status::Idle);
    // アクティビティも消して、まとめて設定する場合
    handle.set(Presence::new(Status::Idle));
}
```

## 知っておくこと

- **待たずに戻ります。** `set` は、Discord での表示が変わるのを待ちません。短い間に何度も呼ぶと、最新の値にまとめて送ります。
- **送る間隔は 5 秒以上です。** Discord の制限（20 秒に最大 5 回）を守るためです。
- **再接続しても残ります。** 接続が回復したときは、最後に設定した値を送り直します。
- **`Status::Offline` は `Invisible` として送ります。** Discord では、Bot をオフライン表示にするには `Invisible` を使うためです（discord.py と同じ扱いです）。
- 管理者だけが変えられるコマンドのサンプルは、リポジトリの `examples/presence` にあります。

## 特定の権限を持つ人だけに使わせる

ステータスを変えるコマンドのように、権限のある人だけに使わせたいコマンドには、`register_with_permissions` が使えます。Discord の一覧での表示条件と、実行前の権限チェックを、同じ権限でまとめて設定します。足りないときは、ハンドラーを呼ばずに、`Messages::check_failed` を本人にだけ返します。グループには `group_with_permissions` を使います。詳しくは[権限の確認](permissions.md)を見てください。

> [!NOTE]
> `register_with_permissions` は、サーバー限定の登録（`SlashCommands::for_guild`）でだけ使えます。DM では権限を確認できないためです。空の権限集合を渡すと、管理者専用になります。
