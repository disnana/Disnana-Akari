use disnana_akari::{
    ChannelSelectContext, Result, RoleSelectContext, SlashContext, UserSelectContext,
    model::channel::ChannelType,
    ui::{ChannelSelect, ComponentsV2, RoleSelect, UserSelect},
};

// 画面を作るときと、選択されたときの関数を登録するときに、同じ識別子を使います。
pub const USER_ID: &str = "akari:sample:entities:user";
pub const ROLE_ID: &str = "akari:sample:entities:role";
pub const CHANNEL_ID: &str = "akari:sample:entities:channel";

pub async fn show(ctx: &mut SlashContext<'_>) -> Result<()> {
    // 選択肢は Discord が用意するので、候補の一覧を自分で作る必要はありません。
    let message = ComponentsV2::new()
        .text("## ユーザー・ロール・チャンネルの選択\nメニューから一項目を選ぶと、対応する ID が表示されます。")
        .user_select(UserSelect::new(USER_ID).placeholder("ユーザーを選択"))
        .role_select(RoleSelect::new(ROLE_ID).placeholder("ロールを選択"))
        .channel_select(
            ChannelSelect::new(CHANNEL_ID)
                .placeholder("テキストチャンネルを選択")
                .channel_types(vec![ChannelType::GuildText]),
        );
    ctx.reply_v2(&message).await
}

pub async fn choose_user(ctx: &mut UserSelectContext<'_>) -> Result<()> {
    // values() はユーザー専用の ID の一覧です。first() で先頭の要素を借ります。
    let id = ctx.values().first().map(|id| id.get());
    ctx.reply_ephemeral(&selection("ユーザー", id)).await
}

pub async fn choose_role(ctx: &mut RoleSelectContext<'_>) -> Result<()> {
    let id = ctx.values().first().map(|id| id.get());
    ctx.reply_ephemeral(&selection("ロール", id)).await
}

pub async fn choose_channel(ctx: &mut ChannelSelectContext<'_>) -> Result<()> {
    let id = ctx.values().first().map(|id| id.get());
    ctx.reply_ephemeral(&selection("チャンネル", id)).await
}

fn selection(kind: &str, id: Option<u64>) -> String {
    // ID をコード表示するので、返信を見てもユーザーやロールへの通知は発生しません。
    match id {
        Some(id) => format!("選択した{kind}の ID は `{id}` です。"),
        None => format!("{kind}を選択してください。"),
    }
}
