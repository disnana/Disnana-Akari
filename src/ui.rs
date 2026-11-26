//! Discord の埋め込み、操作部品、モーダルを組み立てます。

mod v2;

pub use v2::ComponentsV2;

use std::collections::HashSet;

use twilight_model::{
    channel::message::{
        Component,
        component::{
            ActionRow, Button as RawButton, ButtonStyle, Label, SelectMenu, SelectMenuOption,
            SelectMenuType, TextInput as RawTextInput, TextInputStyle,
        },
        embed::{Embed as RawEmbed, EmbedField},
    },
    http::interaction::{InteractionResponse, InteractionResponseData, InteractionResponseType},
};
use twilight_validate::{component, message};

use crate::Result;

/// タイトル、説明、色、項目を持つ Discord の埋め込みです。
#[derive(Clone, Debug)]
#[must_use]
pub struct Embed(RawEmbed);

impl Embed {
    /// タイトルを持つ埋め込みを作ります。空白だけのタイトルは省略されます。
    pub fn new(title: impl Into<String>) -> Self {
        let title = title.into();
        Self(RawEmbed {
            author: None,
            color: None,
            description: None,
            fields: Vec::new(),
            flags: None,
            footer: None,
            image: None,
            kind: String::from("rich"),
            provider: None,
            thumbnail: None,
            timestamp: None,
            title: (!title.trim().is_empty()).then_some(title),
            url: None,
            video: None,
        })
    }

    /// タイトルの下に表示する説明を設定します。
    pub fn description(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        self.0.description = (!text.trim().is_empty()).then_some(text);
        self
    }

    /// 左端の色を `0x5865F2` のような RGB 値で設定します。
    pub fn color(mut self, rgb: u32) -> Self {
        self.0.color = Some(rgb);
        self
    }

    /// 名前と値を持つ項目を追加します。`inline` が `true` の項目は横並びで表示できます。
    pub fn field(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
        inline: bool,
    ) -> Self {
        self.0.fields.push(EmbedField {
            inline,
            name: name.into(),
            value: value.into(),
        });
        self
    }
}

/// 押すと `custom_id` に対応するイベントを発生させるボタンです。
#[derive(Clone, Debug)]
#[must_use]
pub struct Button(RawButton);

impl Button {
    /// 青色のボタンを作ります。`custom_id` はハンドラー登録時と同じ ID を指定します。
    pub fn primary(custom_id: impl Into<String>, label: impl Into<String>) -> Self {
        Self(RawButton {
            custom_id: Some(custom_id.into()),
            disabled: false,
            emoji: None,
            id: None,
            label: Some(label.into()),
            sku_id: None,
            style: ButtonStyle::Primary,
            url: None,
        })
    }

    /// `true` を指定するとボタンを押せなくします。
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.0.disabled = disabled;
        self
    }
}

/// 文字列選択メニューに表示する選択肢です。
#[derive(Clone, Debug)]
#[must_use]
pub struct SelectOption(SelectMenuOption);

impl SelectOption {
    /// 表示名と、選んだときにハンドラーに渡る値を設定します。
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self(SelectMenuOption {
            default: false,
            description: None,
            emoji: None,
            label: label.into(),
            value: value.into(),
        })
    }

    /// 選択肢の説明を設定します。
    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.0.description = Some(text.into());
        self
    }
}

/// 文字列の選択肢を一つ選べるメニューです。
#[derive(Clone, Debug)]
#[must_use]
pub struct StringSelect(SelectMenu);

impl StringSelect {
    /// ハンドラーと対応する ID と、1〜25 個の選択肢を設定します。
    pub fn new(custom_id: impl Into<String>, options: Vec<SelectOption>) -> Self {
        Self(SelectMenu {
            channel_types: None,
            custom_id: custom_id.into(),
            default_values: None,
            disabled: false,
            id: None,
            kind: SelectMenuType::Text,
            max_values: None,
            min_values: None,
            options: Some(options.into_iter().map(|option| option.0).collect()),
            placeholder: None,
            required: None,
        })
    }

    /// 未選択の状態で表示する案内を設定します。
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.0.placeholder = Some(text.into());
        self
    }
}

/// モーダルに表示する文字入力欄です。
#[derive(Clone, Debug)]
#[must_use]
pub struct TextInput {
    label: String,
    input: RawTextInput,
}

impl TextInput {
    /// 入力欄の ID とラベルを設定します。
    pub fn new(custom_id: impl Into<String>, label: impl Into<String>) -> Self {
        #[allow(deprecated)] // 表示ラベルは外側の Label に設定します。
        let input = RawTextInput {
            custom_id: custom_id.into(),
            id: None,
            label: None,
            max_length: None,
            min_length: None,
            placeholder: None,
            required: None,
            style: TextInputStyle::Short,
            value: None,
        };
        Self {
            label: label.into(),
            input,
        }
    }

    /// 未入力の状態で表示する案内を設定します。
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.input.placeholder = Some(text.into());
        self
    }

    /// 複数行を入力できるようにします。
    pub fn paragraph(mut self) -> Self {
        self.input.style = TextInputStyle::Paragraph;
        self
    }

    /// 入力を必須にするかどうかを設定します。既定では必須です。
    pub fn required(mut self, required: bool) -> Self {
        self.input.required = Some(required);
        self
    }
}

/// Discord の入力フォームです。1 つ以上の入力欄を追加してから開きます。
#[derive(Clone, Debug)]
#[must_use]
pub struct Modal {
    custom_id: String,
    title: String,
    components: Vec<Component>,
}

impl Modal {
    /// 送信イベントと対応する ID と、フォームのタイトルを設定します。
    pub fn new(custom_id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            custom_id: custom_id.into(),
            title: title.into(),
            components: Vec::new(),
        }
    }

    /// 文字入力欄を追加します。1 つのフォームに最大 5 欄まで追加できます。
    pub fn text_input(mut self, input: TextInput) -> Self {
        self.components.push(Component::Label(Label {
            component: Box::new(Component::TextInput(input.input)),
            description: None,
            id: None,
            label: input.label,
        }));
        self
    }

    pub(crate) fn response(&self) -> Result<InteractionResponse> {
        validate_custom_id(&self.custom_id)?;
        if self.title.trim().is_empty() || self.title.chars().count() > 45 {
            return Err("モーダルのタイトルは空白以外を含む 1〜45 文字にしてください。".into());
        }
        if !(1..=5).contains(&self.components.len()) {
            return Err("モーダルには入力欄が 1〜5 個必要です。".into());
        }

        let mut ids = HashSet::new();
        for item in &self.components {
            let Component::Label(label) = item else {
                return Err("入力欄は Label の中に配置してください。".into());
            };
            component::label(label)?;
            let Component::TextInput(input) = label.component.as_ref() else {
                return Err("モーダルの入力欄の種類が無効です。".into());
            };
            if label.label.trim().is_empty() {
                return Err("入力欄には空でないラベルが必要です。".into());
            }
            check_custom_id(&mut ids, &input.custom_id)?;
        }

        Ok(InteractionResponse {
            kind: InteractionResponseType::Modal,
            data: Some(InteractionResponseData {
                custom_id: Some(self.custom_id.clone()),
                title: Some(self.title.clone()),
                components: Some(self.components.clone()),
                ..Default::default()
            }),
        })
    }
}

/// Discord への返信に使う本文、埋め込み、ボタン、選択メニューの組み合わせです。
#[derive(Clone, Debug, Default)]
#[must_use]
pub struct UiMessage {
    pub(crate) content: Option<String>,
    pub(crate) embeds: Vec<RawEmbed>,
    pub(crate) components: Vec<Component>,
}

impl UiMessage {
    /// 空の返信を作ります。本文や埋め込み、ボタン、選択メニューを追加してから送信します。
    pub fn new() -> Self {
        Self::default()
    }

    /// メッセージの本文を設定します。
    pub fn content(mut self, text: impl Into<String>) -> Self {
        self.content = Some(text.into());
        self
    }

    /// メッセージに埋め込みを一つ追加します。
    pub fn embed(mut self, embed: Embed) -> Self {
        self.embeds.push(embed.0);
        self
    }

    /// 1〜5 個のボタンを横一列に追加します。1 メッセージに最大 5 列まで追加できます。
    pub fn buttons(mut self, buttons: Vec<Button>) -> Self {
        self.components.push(button_row(buttons));
        self
    }

    /// 文字列選択メニューを一つの行として追加します。
    pub fn select(mut self, select: StringSelect) -> Self {
        self.components.push(select_row(select));
        self
    }

    pub(crate) fn validate(&self) -> Result<()> {
        let content = self.content.as_deref().unwrap_or_default();
        if content.trim().is_empty() && self.embeds.is_empty() && self.components.is_empty() {
            return Err(
                "本文、埋め込み、ボタン、選択メニューのいずれかを追加してください。".into(),
            );
        }
        message::content(content)?;
        message::embeds(&self.embeds)?;
        message::components(&self.components, false)?;

        for embed in &self.embeds {
            if embed.title.is_none() && embed.description.is_none() && embed.fields.is_empty() {
                return Err("埋め込みにタイトル、説明、項目のいずれかを追加してください。".into());
            }
            if embed
                .fields
                .iter()
                .any(|field| field.name.trim().is_empty() || field.value.trim().is_empty())
            {
                return Err("埋め込みの項目には名前と値が必要です。".into());
            }
        }

        let mut ids = HashSet::new();
        for component in &self.components {
            let Component::ActionRow(row) = component else {
                return Err("操作部品は Action Row の中に配置してください。".into());
            };
            check_row(row, &mut ids)?;
        }

        Ok(())
    }
}

fn button_row(buttons: Vec<Button>) -> Component {
    Component::ActionRow(ActionRow {
        id: None,
        components: buttons
            .into_iter()
            .map(|button| Component::Button(button.0))
            .collect(),
    })
}

fn select_row(select: StringSelect) -> Component {
    Component::ActionRow(ActionRow {
        id: None,
        components: vec![Component::SelectMenu(select.0)],
    })
}

fn check_row<'a>(row: &'a ActionRow, ids: &mut HashSet<&'a str>) -> Result<()> {
    match row.components.as_slice() {
        [] => return Err("空の行は追加できません。".into()),
        [Component::SelectMenu(select)] => return check_select(select, ids),
        buttons => {
            for item in buttons {
                let Component::Button(button) = item else {
                    return Err(
                        "1 行にはボタンだけ、または選択メニュー 1 個を配置してください。".into(),
                    );
                };
                check_button(button, ids)?;
            }
        }
    }
    Ok(())
}

fn check_button<'a>(button: &'a RawButton, ids: &mut HashSet<&'a str>) -> Result<()> {
    if button.style != ButtonStyle::Premium
        && button.emoji.is_none()
        && button
            .label
            .as_deref()
            .is_none_or(|label| label.trim().is_empty())
    {
        return Err("ボタンにはラベルまたは絵文字が必要です。".into());
    }
    if let Some(id) = button.custom_id.as_deref() {
        check_custom_id(ids, id)?;
    }
    Ok(())
}

fn check_select<'a>(select: &'a SelectMenu, ids: &mut HashSet<&'a str>) -> Result<()> {
    check_custom_id(ids, &select.custom_id)?;
    if select.kind == SelectMenuType::Text {
        let options = select.options.as_deref().unwrap_or_default();
        if options.is_empty() || usize::from(select.max_values.unwrap_or(1)) > options.len() {
            return Err("選択メニューには選択できる数以上の選択肢が必要です。".into());
        }
        let mut values = HashSet::new();
        for option in options {
            if option.label.trim().is_empty() || option.value.is_empty() {
                return Err("選択肢には空でない表示名と値が必要です。".into());
            }
            if !values.insert(&option.value) {
                return Err("選択肢の値は重複できません。".into());
            }
        }
    }
    if select.min_values.unwrap_or(1) > select.max_values.unwrap_or(1) {
        return Err("選択できる最小数は最大数以下にしてください。".into());
    }
    Ok(())
}

fn check_custom_id<'a>(ids: &mut HashSet<&'a str>, id: &'a str) -> Result<()> {
    validate_custom_id(id)?;
    if !ids.insert(id) {
        return Err("同じメッセージやモーダルの中で操作部品の ID は重複できません。".into());
    }
    Ok(())
}

fn validate_custom_id(id: &str) -> Result<()> {
    if id.is_empty() || id.chars().count() > component::COMPONENT_CUSTOM_ID_LENGTH {
        return Err("操作部品の ID は 1〜100 文字にしてください。".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        Button, ButtonStyle, Component, Embed, InteractionResponseType, Modal, SelectOption,
        StringSelect, TextInput, TextInputStyle, UiMessage,
    };

    #[test]
    fn builds_embed_and_buttons_in_the_same_message() {
        let message = UiMessage::new()
            .content("接続状況")
            .embed(
                Embed::new("Akari")
                    .description("Bot の状況を表示")
                    .color(0x5865F2)
                    .field("状態", "オンライン", true),
            )
            .buttons(vec![Button::primary("refresh", "更新").disabled(true)]);

        assert!(message.validate().is_ok());
        assert_eq!(message.content.as_deref(), Some("接続状況"));
        assert_eq!(message.embeds[0].fields[0].value, "オンライン");
        let [Component::ActionRow(row)] = message.components.as_slice() else {
            panic!("1 行を想定");
        };
        let [Component::Button(button)] = row.components.as_slice() else {
            panic!("ボタン 1 個を想定");
        };
        assert_eq!(button.style, ButtonStyle::Primary);
        assert_eq!(button.custom_id.as_deref(), Some("refresh"));
        assert!(button.disabled);
    }

    #[test]
    fn rejects_empty_or_oversized_payloads() {
        assert!(UiMessage::new().content("　").validate().is_err());
        assert!(UiMessage::new().embed(Embed::new("")).validate().is_err());
        assert!(
            UiMessage::new()
                .content("あ".repeat(2_001))
                .validate()
                .is_err()
        );
        assert!(
            UiMessage::new()
                .embed(Embed::new("1").description("あ".repeat(3_000)))
                .embed(Embed::new("2").description("い".repeat(3_000)))
                .validate()
                .is_err()
        );
    }

    #[test]
    fn rejects_missing_buttons_and_duplicate_ids_across_rows() {
        assert!(
            UiMessage::new()
                .content("本文")
                .buttons(vec![])
                .validate()
                .is_err()
        );
        assert!(
            UiMessage::new()
                .buttons(vec![Button::primary("", "更新")])
                .validate()
                .is_err()
        );
        assert!(
            UiMessage::new()
                .buttons(vec![Button::primary("refresh", "更新")])
                .buttons(vec![Button::primary("refresh", "もう一度")])
                .validate()
                .is_err()
        );
        assert!(
            UiMessage::new()
                .buttons(
                    (0..6)
                        .map(|index| Button::primary(index.to_string(), "更新"))
                        .collect()
                )
                .validate()
                .is_err()
        );
    }

    #[test]
    fn string_select_gets_its_own_row_and_rejects_empty_or_ambiguous_options() {
        let message = UiMessage::new().select(
            StringSelect::new("color", vec![SelectOption::new("青", "blue")])
                .placeholder("一つ選択"),
        );
        assert!(message.validate().is_ok());
        let [Component::ActionRow(row)] = message.components.as_slice() else {
            panic!("選択メニュー用の行を想定");
        };
        let [Component::SelectMenu(select)] = row.components.as_slice() else {
            panic!("選択メニュー 1 個を想定");
        };
        assert_eq!(select.placeholder.as_deref(), Some("一つ選択"));
        assert!(
            UiMessage::new()
                .select(StringSelect::new("color", vec![]))
                .validate()
                .is_err()
        );
        assert!(
            UiMessage::new()
                .select(StringSelect::new(
                    "color",
                    vec![
                        SelectOption::new("青", "blue"),
                        SelectOption::new("別の青", "blue"),
                    ]
                ))
                .validate()
                .is_err()
        );
    }

    #[test]
    fn builds_modal_with_labels_and_rejects_duplicate_input_ids() {
        let modal = Modal::new("feedback", "感想を書く")
            .text_input(TextInput::new("body", "感想").paragraph().required(false));
        let response = modal.response().unwrap();
        assert_eq!(response.kind, InteractionResponseType::Modal);
        let data = response.data.unwrap();
        assert_eq!(data.custom_id.as_deref(), Some("feedback"));
        let components = data.components.unwrap();
        let [Component::Label(label)] = components.as_slice() else {
            panic!("Label を想定");
        };
        let Component::TextInput(input) = label.component.as_ref() else {
            panic!("Label 内の文字入力欄を想定");
        };
        assert_eq!(input.custom_id, "body");
        assert_eq!(input.style, TextInputStyle::Paragraph);
        assert_eq!(input.required, Some(false));
        assert!(Modal::new("feedback", "感想を書く").response().is_err());
        assert!(
            modal
                .text_input(TextInput::new("body", "同じ ID"))
                .response()
                .is_err()
        );
    }
}
