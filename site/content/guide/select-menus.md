# 選択メニュー

プルダウンから選ばせる部品です。文字列の選択肢のほかに、ユーザー、ロール、チャンネル、そして「ユーザーかロール」を選ばせるメニューがあります。

## 文字列の選択メニュー

```rust
use disnana_akari::ui::{SelectOption, StringSelect, UiMessage};
use disnana_akari::{Events, Result, SelectContext, SlashContext};

const COLOR: &str = "akari:demo:color";

async fn show(ctx: &mut SlashContext<'_>) -> Result<()> {
    let select = StringSelect::new(
        COLOR,
        vec![
            SelectOption::new("青", "blue").description("落ち着いた色"),
            SelectOption::new("赤", "red"),
            SelectOption::new("緑", "green"),
        ],
    )
    .placeholder("好きな色を選んでください");

    let message = UiMessage::new().content("色を選んでください").select(select);
    ctx.reply_ui(&message).await
}

fn register(events: &mut Events) -> Result<()> {
    events.on_select(COLOR, choose)
}

async fn choose(ctx: &mut SelectContext<'_>) -> Result<()> {
    let color = ctx.values().first().map(String::as_str).unwrap_or("?");
    ctx.reply_ephemeral(&format!("{color} を選びました。")).await
}
```

- `SelectOption::new(ラベル, 値)` の **ラベル** は画面に表示され、**値** は選ばれたときに `ctx.values()` で受け取ります。
- 選択肢は 1〜25 個です。値は重複できません。
- 複数選べるようにするには、`.min_values(1).max_values(3)` のように個数を指定します。最大値は選択肢の数以下にします。
- `.disabled(true)` で、操作できない状態にできます。

## ユーザー・ロール・チャンネルを選ばせる

候補は Discord が用意してくれるので、選択肢を自分で作る必要はありません。

| 作る | 追加する | 登録する | 受け取る型 |
| --- | --- | --- | --- |
| `UserSelect::new(id)` | `.user_select(..)` | `on_user_select` | `&[Id<UserMarker>]` |
| `RoleSelect::new(id)` | `.role_select(..)` | `on_role_select` | `&[Id<RoleMarker>]` |
| `ChannelSelect::new(id)` | `.channel_select(..)` | `on_channel_select` | `&[Id<ChannelMarker>]` |
| `MentionableSelect::new(id)` | `.mentionable_select(..)` | `on_mentionable_select` | `&[Mentionable]` |

```rust
use disnana_akari::model::channel::ChannelType;
use disnana_akari::ui::{ChannelSelect, UiMessage, UserSelect};
use disnana_akari::{ChannelSelectContext, Events, Result, SlashContext, UserSelectContext};

const USER: &str = "akari:demo:user";
const CHANNEL: &str = "akari:demo:channel";

async fn show(ctx: &mut SlashContext<'_>) -> Result<()> {
    let message = UiMessage::new()
        .content("選んでください")
        .user_select(UserSelect::new(USER).placeholder("ユーザー"))
        .channel_select(
            ChannelSelect::new(CHANNEL)
                .channel_types(vec![ChannelType::GuildText]) // テキストチャンネルだけ
                .placeholder("チャンネル"),
        );
    ctx.reply_ui(&message).await
}

fn register(events: &mut Events) -> Result<()> {
    events.on_user_select(USER, user_selected)?;
    events.on_channel_select(CHANNEL, channel_selected)
}

async fn user_selected(ctx: &mut UserSelectContext<'_>) -> Result<()> {
    let Some(id) = ctx.values().first() else {
        return ctx.reply_ephemeral("選択が空です。").await;
    };
    // バッククォートで囲むと、メンションにならず ID だけが表示される
    ctx.reply_ephemeral(&format!("ユーザー ID は `{}` です。", id.get())).await
}

async fn channel_selected(ctx: &mut ChannelSelectContext<'_>) -> Result<()> {
    let ids: Vec<String> = ctx.values().iter().map(|id| id.get().to_string()).collect();
    ctx.reply_ephemeral(&format!("チャンネル ID: {}", ids.join(", "))).await
}
```

ユーザーとロールを同じメニューで選ばせるときは、`MentionableSelect` を使います。選ばれた項目は `Mentionable::User(id)` か `Mentionable::Role(id)` で届くので、`match` で分けます。

```rust
use disnana_akari::{Mentionable, MentionableSelectContext, Result};

async fn picked(ctx: &mut MentionableSelectContext<'_>) -> Result<()> {
    let mut lines = Vec::new();
    for item in ctx.values() {
        match item {
            Mentionable::User(id) => lines.push(format!("ユーザー `{}`", id.get())),
            Mentionable::Role(id) => lines.push(format!("ロール `{}`", id.get())),
            _ => lines.push("この種類の選択項目には対応していません。".to_owned()),
        }
    }
    ctx.reply_ephemeral(&lines.join("\n")).await
}
```

Discord が選択対象の詳しい情報を一緒に送ってきたときは、`ctx.resolved()` で読めます。

## 初期選択を指定する

`SelectOption::selected_by_default(true)` を付けた文字列の選択肢は初期状態で選択されます。ユーザー、ロール、チャンネルのメニューには、各種類に合う型付き ID を `default_values` に渡します。メンションのメニューでは、異なる種類を `Mentionable` でまとめます。

```rust
use disnana_akari::{Mentionable, model::id::Id, ui::{MentionableSelect, SelectOption, UserSelect}};

fn defaults() -> (SelectOption, UserSelect, MentionableSelect) {
    (
        SelectOption::new("青", "blue").selected_by_default(true),
        UserSelect::new("user").default_values([Id::new(123)]),
        MentionableSelect::new("team").max_values(2).default_values([
            Mentionable::User(Id::new(123)),
            Mentionable::Role(Id::new(456)),
        ]),
    )
}
```

初期選択の件数はメニューの上限に収め、同じ対象を重複させないようにします。

対象ごとに違う ID を使うメニューは、`DynamicId` と `on_dynamic_select`、`on_dynamic_user_select` などを使います。対象は `ctx.dynamic_id()`、実際に選ばれた項目は `ctx.values()` で取得します。[動的 ID の使い方](buttons.md#再起動後も同じボタンを使う)

## 選択のあとにできること

選択メニューのコンテキストでも、[ボタンと同じメソッド](buttons.md)が使えます。`reply_ephemeral`、`defer_update`、`update_message`、`update_v2`、`show_modal`、`followup`、`author_id`、`invoker_id`、`message` などです。時間のかかる処理は、先に `defer_update` で受け付けます。受け付け後は `followup_files` や `followup_files_ephemeral` で添付付きのメッセージを追加できます。[添付付きフォローアップ](responding.md#追加のメッセージにファイルを添付する)
