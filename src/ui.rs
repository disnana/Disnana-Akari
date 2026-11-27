//! Discord の埋め込み、操作部品、モーダルを組み立てます。

mod v2;

pub use v2::ComponentsV2;

use std::collections::HashSet;

use http_types::Uri;
use twilight_model::{
    channel::ChannelType,
    channel::message::{
        Component, EmojiReactionType,
        component::{
            ActionRow, Button as RawButton, ButtonStyle, Label, SelectMenu, SelectMenuOption,
            SelectMenuType, TextInput as RawTextInput, TextInputStyle,
        },
        embed::{Embed as RawEmbed, EmbedField},
    },
    http::interaction::{InteractionResponse, InteractionResponseData, InteractionResponseType},
};
use twilight_validate::{component, message};

use crate::{Error, Result};

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

/// 登録したイベントを呼ぶか、リンク先を開くボタンです。
#[derive(Clone, Debug)]
#[must_use]
pub struct Button(RawButton);

impl Button {
    /// 青色のボタンを作ります。`custom_id` はハンドラー登録時と同じ ID を指定します。
    pub fn primary(custom_id: impl Into<String>, label: impl Into<String>) -> Self {
        Self::interactive(ButtonStyle::Primary, custom_id, label)
    }

    /// 補助操作向けの灰色のボタンを作ります。
    pub fn secondary(custom_id: impl Into<String>, label: impl Into<String>) -> Self {
        Self::interactive(ButtonStyle::Secondary, custom_id, label)
    }

    /// 確定操作向けの緑色のボタンを作ります。
    pub fn success(custom_id: impl Into<String>, label: impl Into<String>) -> Self {
        Self::interactive(ButtonStyle::Success, custom_id, label)
    }

    /// 削除など、影響の大きい操作向けの赤色のボタンを作ります。
    pub fn danger(custom_id: impl Into<String>, label: impl Into<String>) -> Self {
        Self::interactive(ButtonStyle::Danger, custom_id, label)
    }

    /// 絶対 HTTP(S) URL を開くボタンを作ります。クリックのイベントは発生しません。
    pub fn link(url: impl Into<String>, label: impl Into<String>) -> Self {
        Self::styled(ButtonStyle::Link, None, Some(url.into()), label.into())
    }

    fn interactive(
        style: ButtonStyle,
        custom_id: impl Into<String>,
        label: impl Into<String>,
    ) -> Self {
        Self::styled(style, Some(custom_id.into()), None, label.into())
    }

    fn styled(
        style: ButtonStyle,
        custom_id: Option<String>,
        url: Option<String>,
        label: String,
    ) -> Self {
        Self(RawButton {
            custom_id,
            disabled: false,
            emoji: None,
            id: None,
            label: (!label.is_empty()).then_some(label),
            sku_id: None,
            style,
            url,
        })
    }

    /// Unicode 絵文字や Discord のカスタム絵文字を設定します。
    /// 絵文字だけを表示する場合は、作成時に空のラベルを渡します。
    pub fn emoji(mut self, emoji: EmojiReactionType) -> Self {
        self.0.emoji = Some(emoji);
        self
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

/// 文字列の選択肢から、指定した個数を選べるメニューです。既定では一つ選びます。
#[derive(Clone, Debug)]
#[must_use]
pub struct StringSelect(SelectMenu);

impl StringSelect {
    /// ハンドラーと対応する ID と、1〜25 個の選択肢を設定します。
    pub fn new(custom_id: impl Into<String>, options: Vec<SelectOption>) -> Self {
        let mut menu = new_select(custom_id, SelectMenuType::Text);
        menu.options = Some(options.into_iter().map(|option| option.0).collect());
        Self(menu)
    }

    /// 未選択の状態で表示する案内を設定します。
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.0.placeholder = Some(text.into());
        self
    }

    /// 選ぶ必要がある最小数を 0〜25 個で指定します。既定では 1 個です。
    pub fn min_values(mut self, min: u8) -> Self {
        self.0.min_values = Some(min);
        self
    }

    /// 選べる最大数を 1〜25 個で指定します。選択肢の総数以下にします。
    pub fn max_values(mut self, max: u8) -> Self {
        self.0.max_values = Some(max);
        self
    }

    /// `true` を指定するとメニューを操作できなくします。
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.0.disabled = disabled;
        self
    }
}

macro_rules! entity_select {
    ($name:ident, $kind:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Debug)]
        #[must_use]
        pub struct $name(SelectMenu);

        impl $name {
            /// ハンドラーに対応する ID を指定します。Discord が選択肢を用意します。
            pub fn new(custom_id: impl Into<String>) -> Self {
                Self(new_select(custom_id, SelectMenuType::$kind))
            }

            /// 未選択の状態で表示する案内を設定します。
            pub fn placeholder(mut self, text: impl Into<String>) -> Self {
                self.0.placeholder = Some(text.into());
                self
            }

            /// 選ぶ必要がある最小数を 0〜25 個で指定します。既定では 1 個です。
            pub fn min_values(mut self, min: u8) -> Self {
                self.0.min_values = Some(min);
                self
            }

            /// 選べる最大数を 1〜25 個で指定します。既定では 1 個です。
            pub fn max_values(mut self, max: u8) -> Self {
                self.0.max_values = Some(max);
                self
            }

            /// `true` を指定するとメニューを操作できなくします。
            pub fn disabled(mut self, disabled: bool) -> Self {
                self.0.disabled = disabled;
                self
            }
        }
    };
}

entity_select!(UserSelect, User, "ユーザーの選択メニューです。");
entity_select!(RoleSelect, Role, "ロールの選択メニューです。");
entity_select!(ChannelSelect, Channel, "チャンネルの選択メニューです。");
entity_select!(
    MentionableSelect,
    Mentionable,
    "ユーザーとロールの両方を候補にする選択メニューです。"
);

impl ChannelSelect {
    /// 選択肢に表示するチャンネルの種類を絞り込みます。
    pub fn channel_types(mut self, channel_types: Vec<ChannelType>) -> Self {
        self.0.channel_types = Some(channel_types);
        self
    }
}

fn new_select(custom_id: impl Into<String>, kind: SelectMenuType) -> SelectMenu {
    SelectMenu {
        channel_types: None,
        custom_id: custom_id.into(),
        default_values: None,
        disabled: false,
        id: None,
        kind,
        max_values: None,
        min_values: None,
        options: None,
        placeholder: None,
        required: None,
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
        #[allow(deprecated, reason = "表示ラベルは外側の Label に設定する。")]
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

    /// 入力の最小文字数を 0〜4000 文字で指定します。既定では 0 文字です。
    pub fn min_length(mut self, min: u16) -> Self {
        self.input.min_length = Some(min);
        self
    }

    /// 入力の最大文字数を 1〜4000 文字で指定します。既定では 4000 文字です。
    pub fn max_length(mut self, max: u16) -> Self {
        self.input.max_length = Some(max);
        self
    }

    /// フォームを開いた時点で入力されている文字列を指定します。
    /// 空でない文字列は指定した最小・最大文字数に収めます。
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.input.value = Some(value.into());
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
            return Err(Error::InvalidUi(
                "モーダルのタイトルは空白以外を含む 1〜45 文字にしてください。",
            ));
        }
        if !(1..=5).contains(&self.components.len()) {
            return Err(Error::InvalidUi("モーダルには入力欄が 1〜5 個必要です。"));
        }

        let mut ids = HashSet::new();
        for item in &self.components {
            let Component::Label(label) = item else {
                return Err(Error::InvalidUi("入力欄は Label の中に配置してください。"));
            };
            component::label(label)?;
            let Component::TextInput(input) = label.component.as_ref() else {
                return Err(Error::InvalidUi("モーダルの入力欄の種類が無効です。"));
            };
            if label.label.trim().is_empty() {
                return Err(Error::InvalidUi("入力欄には空でないラベルが必要です。"));
            }
            check_custom_id(&mut ids, &input.custom_id)?;

            let min = usize::from(input.min_length.unwrap_or_default());
            let max = input
                .max_length
                .map_or(component::TEXT_INPUT_LENGTH_MAX, usize::from);
            if min > max {
                return Err(Error::InvalidUi(
                    "入力欄の最小文字数は最大文字数以下にしてください。",
                ));
            }
            if let Some(value) = input.value.as_deref().filter(|value| !value.is_empty())
                && !(min..=max).contains(&value.chars().count())
            {
                return Err(Error::InvalidUi(
                    "入力欄の初期値を指定した文字数の範囲に収めてください。",
                ));
            }
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
        self.components.push(select_row(select.0));
        self
    }

    /// ユーザー選択メニューを一つの行として追加します。
    pub fn user_select(mut self, select: UserSelect) -> Self {
        self.components.push(select_row(select.0));
        self
    }

    /// ロール選択メニューを一つの行として追加します。
    pub fn role_select(mut self, select: RoleSelect) -> Self {
        self.components.push(select_row(select.0));
        self
    }

    /// チャンネル選択メニューを一つの行として追加します。
    pub fn channel_select(mut self, select: ChannelSelect) -> Self {
        self.components.push(select_row(select.0));
        self
    }

    /// ユーザーとロールの混在する選択メニューを一つの行として追加します。
    pub fn mentionable_select(mut self, select: MentionableSelect) -> Self {
        self.components.push(select_row(select.0));
        self
    }

    pub(crate) fn validate(&self) -> Result<()> {
        let content = self.content.as_deref().unwrap_or_default();
        if content.trim().is_empty() && self.embeds.is_empty() && self.components.is_empty() {
            return Err(Error::InvalidUi(
                "本文、埋め込み、ボタン、選択メニューのいずれかを追加してください。",
            ));
        }
        message::content(content)?;
        message::embeds(&self.embeds)?;
        message::components(&self.components, false)?;

        for embed in &self.embeds {
            if embed.title.is_none() && embed.description.is_none() && embed.fields.is_empty() {
                return Err(Error::InvalidUi(
                    "埋め込みにタイトル、説明、項目のいずれかを追加してください。",
                ));
            }
            if embed
                .fields
                .iter()
                .any(|field| field.name.trim().is_empty() || field.value.trim().is_empty())
            {
                return Err(Error::InvalidUi("埋め込みの項目には名前と値が必要です。"));
            }
        }

        let mut ids = HashSet::new();
        for component in &self.components {
            let Component::ActionRow(row) = component else {
                return Err(Error::InvalidUi(
                    "操作部品は Action Row の中に配置してください。",
                ));
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

fn select_row(select: SelectMenu) -> Component {
    Component::ActionRow(ActionRow {
        id: None,
        components: vec![Component::SelectMenu(select)],
    })
}

fn check_row<'a>(row: &'a ActionRow, ids: &mut HashSet<&'a str>) -> Result<()> {
    match row.components.as_slice() {
        [] => return Err(Error::InvalidUi("空の行は追加できません。")),
        [Component::SelectMenu(select)] => return check_select(select, ids),
        buttons => {
            for item in buttons {
                let Component::Button(button) = item else {
                    return Err(Error::InvalidUi(
                        "1 行にはボタンだけ、または選択メニュー 1 個を配置してください。",
                    ));
                };
                check_button(button, ids)?;
            }
        }
    }
    Ok(())
}

fn check_button<'a>(button: &'a RawButton, ids: &mut HashSet<&'a str>) -> Result<()> {
    if matches!(&button.emoji, Some(EmojiReactionType::Unicode { name }) if name.trim().is_empty())
    {
        return Err(Error::InvalidUi("Unicode 絵文字を指定してください。"));
    }
    if button.style != ButtonStyle::Premium
        && button.emoji.is_none()
        && button
            .label
            .as_deref()
            .is_none_or(|label| label.trim().is_empty())
    {
        return Err(Error::InvalidUi("ボタンにはラベルまたは絵文字が必要です。"));
    }
    if button.style == ButtonStyle::Link {
        let url = button.url.as_deref().unwrap_or_default();
        let target = url.split_once('#').map_or(url, |(target, _)| target);
        if url.chars().count() > 512
            || url.chars().any(char::is_whitespace)
            || !target.parse::<Uri>().is_ok_and(|uri| {
                matches!(uri.scheme_str(), Some("http" | "https")) && uri.host().is_some()
            })
        {
            return Err(Error::InvalidUi(
                "リンクボタンには512文字以内の絶対 HTTP(S) URL を指定してください。",
            ));
        }
    }
    if let Some(id) = button.custom_id.as_deref() {
        check_custom_id(ids, id)?;
    }
    Ok(())
}

fn check_select<'a>(select: &'a SelectMenu, ids: &mut HashSet<&'a str>) -> Result<()> {
    check_custom_id(ids, &select.custom_id)?;
    let min = select.min_values.unwrap_or(1);
    let max = select.max_values.unwrap_or(1);
    if min > max {
        return Err(Error::InvalidUi(
            "選択できる最小数は最大数以下にしてください。",
        ));
    }
    if select.kind == SelectMenuType::Text {
        let options = select.options.as_deref().unwrap_or_default();
        if options.is_empty() || usize::from(max) > options.len() {
            return Err(Error::InvalidUi(
                "選択メニューには選択できる数以上の選択肢が必要です。",
            ));
        }
        let mut values = HashSet::new();
        for option in options {
            if option.label.trim().is_empty() || option.value.is_empty() {
                return Err(Error::InvalidUi("選択肢には空でない表示名と値が必要です。"));
            }
            if !values.insert(&option.value) {
                return Err(Error::InvalidUi("選択肢の値は重複できません。"));
            }
        }
    }
    Ok(())
}

fn check_custom_id<'a>(ids: &mut HashSet<&'a str>, id: &'a str) -> Result<()> {
    validate_custom_id(id)?;
    if !ids.insert(id) {
        return Err(Error::InvalidUi(
            "同じメッセージやモーダルの中で操作部品の ID は重複できません。",
        ));
    }
    Ok(())
}

fn validate_custom_id(id: &str) -> Result<()> {
    if id.is_empty() || id.chars().count() > component::COMPONENT_CUSTOM_ID_LENGTH {
        return Err(Error::InvalidUi(
            "操作部品の ID は 1〜100 文字にしてください。",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn link_and_styled_buttons_work_with_both_message_formats() {
        use super::{ComponentsV2, EmojiReactionType};

        let emoji = EmojiReactionType::Unicode {
            name: "🔄".into()
        };
        let secondary = Button::secondary("refresh", "").emoji(emoji.clone());
        let success = Button::success("accept", "承認");
        let danger = Button::danger("remove", "削除");
        let link = Button::link("https://example.com/docs?mode=ui#buttons", "ガイド");

        assert_eq!(secondary.0.style, ButtonStyle::Secondary);
        assert_eq!(secondary.0.custom_id.as_deref(), Some("refresh"));
        assert_eq!(secondary.0.emoji, Some(emoji));
        assert!(secondary.0.label.is_none());
        assert_eq!(success.0.style, ButtonStyle::Success);
        assert_eq!(danger.0.style, ButtonStyle::Danger);
        assert_eq!(link.0.style, ButtonStyle::Link);
        assert!(link.0.custom_id.is_none());
        assert_eq!(
            link.0.url.as_deref(),
            Some("https://example.com/docs?mode=ui#buttons")
        );

        let buttons = vec![secondary, success, danger, link];
        assert!(UiMessage::new().buttons(buttons.clone()).validate().is_ok());
        assert!(ComponentsV2::new().buttons(buttons).validate().is_ok());
    }

    #[test]
    fn rejects_invalid_links_and_empty_emoji_in_both_message_formats() {
        use super::{ComponentsV2, EmojiReactionType};

        for url in [
            "",
            "/docs",
            "javascript:alert(1)",
            "https://",
            "https://example.com/white space",
            "https://example.com/fragment#white space",
        ] {
            let button = Button::link(url, "ガイド");
            assert!(
                UiMessage::new()
                    .buttons(vec![button.clone()])
                    .validate()
                    .is_err()
            );
            assert!(
                ComponentsV2::new()
                    .buttons(vec![button])
                    .validate()
                    .is_err()
            );
        }
        let long = Button::link(format!("https://example.com/{}", "x".repeat(512)), "ガイド");
        assert!(UiMessage::new().buttons(vec![long]).validate().is_err());

        let empty_emoji =
            Button::primary("ping", "").emoji(EmojiReactionType::Unicode { name: " ".into() });
        assert!(
            UiMessage::new()
                .buttons(vec![empty_emoji])
                .validate()
                .is_err()
        );
    }

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
    fn entity_selects_keep_types_and_settings_in_both_message_formats() {
        use super::{
            ChannelSelect, ChannelType, ComponentsV2, MentionableSelect, RoleSelect,
            SelectMenuType, UserSelect,
        };

        let user = UserSelect::new("user")
            .placeholder("選択")
            .min_values(0)
            .max_values(2);
        let role = RoleSelect::new("role").disabled(true);
        let channel = ChannelSelect::new("channel").channel_types(vec![ChannelType::GuildText]);
        let mentionable = MentionableSelect::new("mentionable")
            .placeholder("ユーザーやロールを選択")
            .min_values(1)
            .max_values(3)
            .disabled(true);

        let legacy = UiMessage::new()
            .user_select(user.clone())
            .role_select(role.clone())
            .channel_select(channel.clone())
            .mentionable_select(mentionable.clone());
        let v2 = ComponentsV2::new()
            .user_select(user)
            .role_select(role)
            .channel_select(channel)
            .mentionable_select(mentionable);
        assert!(legacy.validate().is_ok());
        assert!(v2.validate().is_ok());
        assert_eq!(legacy.components, v2.components);

        let [user, role, channel, mentionable] = legacy.components.as_slice() else {
            panic!("4 行を想定")
        };
        for (item, kind) in [
            (user, SelectMenuType::User),
            (role, SelectMenuType::Role),
            (channel, SelectMenuType::Channel),
            (mentionable, SelectMenuType::Mentionable),
        ] {
            let Component::ActionRow(row) = item else {
                panic!("Action Row を想定")
            };
            let [Component::SelectMenu(select)] = row.components.as_slice() else {
                panic!("選択メニュー 1 個を想定")
            };
            assert_eq!(select.kind, kind);
            assert!(select.options.is_none());
            if kind == SelectMenuType::Mentionable {
                assert_eq!(select.custom_id, "mentionable");
                assert_eq!(
                    select.placeholder.as_deref(),
                    Some("ユーザーやロールを選択")
                );
                assert_eq!(select.min_values, Some(1));
                assert_eq!(select.max_values, Some(3));
                assert!(select.disabled);
            }
        }

        let Component::ActionRow(row) = channel else {
            panic!("想定外の構造")
        };
        let [Component::SelectMenu(select)] = row.components.as_slice() else {
            panic!("想定外の構造")
        };
        assert_eq!(
            select.channel_types.as_deref(),
            Some(&[ChannelType::GuildText][..])
        );
    }

    #[test]
    fn entity_selects_check_bounds_and_custom_ids_across_rows() {
        use super::{ComponentsV2, MentionableSelect, RoleSelect, UserSelect};

        assert!(
            UiMessage::new()
                .user_select(UserSelect::new(""))
                .validate()
                .is_err()
        );
        assert!(
            UiMessage::new()
                .role_select(RoleSelect::new("role").min_values(2).max_values(1))
                .validate()
                .is_err()
        );
        assert!(
            ComponentsV2::new()
                .mentionable_select(MentionableSelect::new("mentionable").max_values(26))
                .validate()
                .is_err()
        );
        assert!(
            ComponentsV2::new()
                .user_select(UserSelect::new("duplicate"))
                .mentionable_select(MentionableSelect::new("duplicate"))
                .validate()
                .is_err()
        );
    }

    #[test]
    fn string_select_checks_configured_bounds_and_keeps_disabled_state() {
        let select = StringSelect::new(
            "color",
            vec![
                SelectOption::new("青", "blue"),
                SelectOption::new("緑", "green"),
            ],
        );
        let configured = select.clone().min_values(0).max_values(2).disabled(true);
        assert_eq!(configured.0.min_values, Some(0));
        assert_eq!(configured.0.max_values, Some(2));
        assert!(configured.0.disabled);
        assert!(UiMessage::new().select(configured).validate().is_ok());

        for invalid in [
            select.clone().min_values(2),
            select.clone().max_values(3),
            select.clone().min_values(0).max_values(0),
            select.min_values(26).max_values(26),
        ] {
            assert!(UiMessage::new().select(invalid).validate().is_err());
        }
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

    #[test]
    fn modal_text_input_checks_length_bounds_and_prefill() {
        let input = || TextInput::new("body", "感想");
        let modal = |input| Modal::new("feedback", "感想を書く").text_input(input);

        let configured = input().min_length(2).max_length(4).value("日本");
        assert_eq!(configured.input.min_length, Some(2));
        assert_eq!(configured.input.max_length, Some(4));
        assert_eq!(configured.input.value.as_deref(), Some("日本"));
        assert!(modal(configured).response().is_ok());
        assert!(modal(input().min_length(2).value("")).response().is_ok());

        for invalid in [
            input().min_length(2).max_length(1),
            input().min_length(4_001),
            input().max_length(0),
            input().max_length(4_001),
            input().min_length(2).value("日"),
            input().max_length(1).value("日本"),
            input().value("あ".repeat(4_001)),
        ] {
            assert!(modal(invalid).response().is_err());
        }
    }
}
