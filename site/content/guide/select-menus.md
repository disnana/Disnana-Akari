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

## 選択のあとにできること

選択メニューのコンテキストでも、[ボタンと同じメソッド](buttons.md)が使えます。`reply_ephemeral`、`defer_update`、`update_message`、`update_v2`、`show_modal`、`followup`、`author_id`、`invoker_id`、`message` などです。時間のかかる処理は、先に `defer_update` で受け付けます。
