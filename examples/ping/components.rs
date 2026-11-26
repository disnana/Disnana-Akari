use disnana_akari::{
    ButtonContext, ModalContext, Result, SelectContext, SlashContext,
    ui::{Button, ComponentsV2, Modal, SelectOption, StringSelect, TextInput},
};

// Discord からイベントが届くときの識別子です。作成側と登録側でそろえます。
pub const SELECT_ID: &str = "akari:sample:components:choice";
pub const OPEN_MODAL_ID: &str = "akari:sample:components:open";
pub const MODAL_ID: &str = "akari:sample:components:form";
const NOTE_ID: &str = "note";

pub async fn show(ctx: &mut SlashContext<'_>) -> Result<()> {
    // ラベルは画面に表示され、値は選択時に Discord から返されます。
    let select = StringSelect::new(
        SELECT_ID,
        vec![
            SelectOption::new("概要", "overview"),
            SelectOption::new("詳細", "details"),
        ],
    )
    .placeholder("表示する項目を選んでください");

    // Components V2 では文章も text(...) コンポーネントで組み立てます。
    let message = ComponentsV2::new()
        .text("## Components V2\n項目を選ぶか、ボタンでフォームを開けます。")
        .separator()
        .select(select)
        .buttons(vec![Button::primary(OPEN_MODAL_ID, "フォームを開く")]);
    ctx.reply_v2(&message).await
}

pub async fn choose(ctx: &mut SelectContext<'_>) -> Result<()> {
    // first() は一覧の先頭を Option で返します。このメニューでは一項目を選べます。
    let reply = match ctx.values().first().map(String::as_str) {
        Some("overview") => "概要を選びました。",
        Some("details") => "詳細を選びました。",
        _ => "選択肢を確認できませんでした。",
    };
    ctx.reply_ephemeral(reply).await
}

pub async fn open_modal(ctx: &mut ButtonContext<'_>) -> Result<()> {
    // フォームを開く操作は、このボタンクリックへの最初の応答として送ります。
    let form = Modal::new(MODAL_ID, "入力フォーム").text_input(
        TextInput::new(NOTE_ID, "メモ")
            .placeholder("サンプルの入力欄")
            .paragraph(),
    );
    ctx.show_modal(&form).await
}

pub async fn submit(ctx: &mut ModalContext<'_>) -> Result<()> {
    // 送信された値を確認し、入力内容はログや返信に含めません。
    if ctx
        .text_input(NOTE_ID)
        .is_some_and(|text| !text.trim().is_empty())
    {
        ctx.reply_ephemeral("入力を受け取りました。").await
    } else {
        ctx.reply_ephemeral("メモを入力してから送信してください。")
            .await
    }
}
