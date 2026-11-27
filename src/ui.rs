//! Discord の埋め込み、操作部品、モーダルを組み立てます。

mod dynamic_id;
mod v2;

pub use dynamic_id::{DynamicId, DynamicIdRef};
pub(crate) use dynamic_id::{
    invalid_namespace as invalid_dynamic_namespace, valid_namespace as valid_dynamic_namespace,
};
pub use v2::{ComponentsV2, FileDisplay, Media};

use std::collections::HashSet;

use http_types::Uri;
use serde::Serialize;
use twilight_model::{
    channel::ChannelType,
    channel::message::{
        Component, EmojiReactionType,
        component::{
            ActionRow, Button as RawButton, ButtonStyle, Checkbox as RawCheckbox,
            CheckboxGroup as RawCheckboxGroup, CheckboxGroupOption as RawCheckboxGroupOption,
            FileUpload as RawFileUpload, Label, SelectDefaultValue, SelectMenu, SelectMenuOption,
            SelectMenuType, TextInput as RawTextInput, TextInputStyle,
        },
        embed::{Embed as RawEmbed, EmbedField},
    },
    http::interaction::{InteractionResponse, InteractionResponseData, InteractionResponseType},
    id::{
        Id,
        marker::{ChannelMarker, RoleMarker, UserMarker},
    },
};
use twilight_validate::{component, message};

use crate::{Error, Mentionable, Result};

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

    /// `true` を指定すると、この選択肢を初めから選択済みにします。
    pub fn selected_by_default(mut self, selected: bool) -> Self {
        self.0.default = selected;
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
    ($name:ident, $kind:ident, $description:literal, $value:ty, $to_default:expr) => {
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

            /// 初めから選択済みにする値を設定します。
            /// 個数は最小・最大選択数の範囲内にし、空なら設定を取り除きます。
            pub fn default_values(mut self, values: impl IntoIterator<Item = $value>) -> Self {
                let values: Vec<_> = values.into_iter().map($to_default).collect();
                self.0.default_values = (!values.is_empty()).then_some(values);
                self
            }
        }
    };
}

entity_select!(
    UserSelect,
    User,
    "ユーザーの選択メニューです。",
    Id<UserMarker>,
    SelectDefaultValue::User
);
entity_select!(
    RoleSelect,
    Role,
    "ロールの選択メニューです。",
    Id<RoleMarker>,
    SelectDefaultValue::Role
);
entity_select!(
    ChannelSelect,
    Channel,
    "チャンネルの選択メニューです。",
    Id<ChannelMarker>,
    SelectDefaultValue::Channel
);
entity_select!(
    MentionableSelect,
    Mentionable,
    "ユーザーとロールの両方を候補にする選択メニューです。",
    Mentionable,
    |value| match value {
        Mentionable::User(id) => SelectDefaultValue::User(id),
        Mentionable::Role(id) => SelectDefaultValue::Role(id),
    }
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

/// モーダルに表示するファイル添付欄です。
#[derive(Clone, Debug)]
#[must_use]
pub struct FileUpload {
    label: String,
    description: Option<String>,
    input: RawFileUpload,
}

impl FileUpload {
    /// 添付欄の ID とラベルを設定します。既定では 1 ファイルの添付が必須です。
    pub fn new(custom_id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            description: None,
            input: RawFileUpload {
                custom_id: custom_id.into(),
                id: None,
                min_values: None,
                max_values: None,
                required: None,
                file_types: None,
            },
        }
    }

    /// 添付欄の説明を設定します。最大 100 文字です。
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// 添付する最小ファイル数を 0〜10 個で指定します。0 は任意入力の場合に使います。
    pub fn min_values(mut self, min: u8) -> Self {
        self.input.min_values = Some(min);
        self
    }

    /// 添付できる最大ファイル数を 0〜10 個で指定します。
    pub fn max_values(mut self, max: u8) -> Self {
        self.input.max_values = Some(max);
        self
    }

    /// 添付を必須にするかどうかを設定します。既定では必須です。
    pub fn required(mut self, required: bool) -> Self {
        self.input.required = Some(required);
        self
    }
}

/// モーダルでオンかオフかを選べるチェックボックスです。
///
/// オフのままでもモーダルを送信できます。チェックを必須にする設定はありません。
#[derive(Clone, Debug)]
#[must_use]
pub struct Checkbox {
    label: String,
    description: Option<String>,
    input: RawCheckbox,
}

impl Checkbox {
    /// チェックボックスの ID とラベルを設定します。既定ではオフです。
    pub fn new(custom_id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            description: None,
            input: RawCheckbox {
                custom_id: custom_id.into(),
                default: None,
                id: None,
            },
        }
    }

    /// チェックボックスの説明を設定します。最大 100 文字です。
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// モーダルを開いたときにチェックされているかを設定します。
    pub fn selected_by_default(mut self, selected: bool) -> Self {
        self.input.default = Some(selected);
        self
    }
}

/// モーダルのチェックボックスグループに表示する選択肢です。
#[derive(Clone, Debug)]
#[must_use]
pub struct CheckboxGroupOption(RawCheckboxGroupOption);

impl CheckboxGroupOption {
    /// 選択肢のラベルと、選択されたときに受け取る値を設定します。どちらも最大 100 文字です。
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self(RawCheckboxGroupOption {
            default: None,
            description: None,
            label: label.into(),
            value: value.into(),
        })
    }

    /// 選択肢の説明を設定します。最大 100 文字です。
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.0.description = Some(description.into());
        self
    }

    /// モーダルを開いたときに選択されているかを設定します。
    pub fn selected_by_default(mut self, selected: bool) -> Self {
        self.0.default = Some(selected);
        self
    }
}

/// モーダルで複数の項目をチェックできる入力欄です。
#[derive(Clone, Debug)]
#[must_use]
pub struct CheckboxGroup {
    label: String,
    description: Option<String>,
    input: RawCheckboxGroup,
}

impl CheckboxGroup {
    /// 入力欄の ID、ラベル、1〜10 個の選択肢を設定します。
    pub fn new(
        custom_id: impl Into<String>,
        label: impl Into<String>,
        options: Vec<CheckboxGroupOption>,
    ) -> Self {
        Self {
            label: label.into(),
            description: None,
            input: RawCheckboxGroup {
                custom_id: custom_id.into(),
                id: None,
                min_values: None,
                max_values: None,
                options: options.into_iter().map(|option| option.0).collect(),
                required: None,
            },
        }
    }

    /// 入力欄の説明を設定します。最大 100 文字です。
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// 選ぶ必要がある最小数を指定します。既定では 1 個です。
    /// 0 を指定するときは、あわせて `required(false)` を指定します。
    pub fn min_values(mut self, min: u8) -> Self {
        self.input.min_values = Some(min);
        self
    }

    /// 選べる最大数を指定します。既定では選択肢の総数です。
    pub fn max_values(mut self, max: u8) -> Self {
        self.input.max_values = Some(max);
        self
    }

    /// 回答を必須にするかどうかを設定します。既定では必須です。
    pub fn required(mut self, required: bool) -> Self {
        self.input.required = Some(required);
        self
    }
}

/// モーダルのラジオグループに表示する選択肢です。
#[derive(Clone, Debug, Serialize)]
#[must_use]
pub struct RadioGroupOption {
    label: String,
    value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<bool>,
}

impl RadioGroupOption {
    /// 選択肢のラベルと、選択されたときに受け取る値を設定します。どちらも最大 100 文字です。
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            description: None,
            default: None,
        }
    }

    /// 選択肢の説明を設定します。最大 100 文字です。
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// モーダルを開いたときに選択されているかを設定します。初期選択は一つまでです。
    pub fn selected_by_default(mut self, selected: bool) -> Self {
        self.default = Some(selected);
        self
    }
}

/// モーダルで一つの選択肢を選べる入力欄です。
#[derive(Clone, Debug)]
#[must_use]
pub struct RadioGroup {
    pub(crate) custom_id: String,
    pub(crate) label: String,
    pub(crate) options: Vec<RadioGroupOption>,
    pub(crate) description: Option<String>,
    pub(crate) required: Option<bool>,
}

impl RadioGroup {
    /// 入力欄の ID、ラベル、2〜10 個の選択肢を設定します。
    pub fn new(
        custom_id: impl Into<String>,
        label: impl Into<String>,
        options: Vec<RadioGroupOption>,
    ) -> Self {
        Self {
            custom_id: custom_id.into(),
            label: label.into(),
            options,
            description: None,
            required: None,
        }
    }

    /// 入力欄の説明を設定します。最大 100 文字です。
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// 選択を必須にするかどうかを設定します。既定では必須です。
    pub fn required(mut self, required: bool) -> Self {
        self.required = Some(required);
        self
    }

    fn validate<'a>(&'a self, ids: &mut HashSet<&'a str>) -> Result<()> {
        check_custom_id(ids, &self.custom_id)?;
        if self.label.trim().is_empty() || self.label.chars().count() > 45 {
            return Err(Error::InvalidUi(
                "入力欄のラベルは空白以外を含む 1〜45 文字にしてください。",
            ));
        }
        if self
            .description
            .as_ref()
            .is_some_and(|text| text.chars().count() > 100)
        {
            return Err(Error::InvalidUi(
                "入力欄の説明は 100 文字以下にしてください。",
            ));
        }
        if !(2..=10).contains(&self.options.len()) {
            return Err(Error::InvalidUi(
                "ラジオグループの選択肢は 2〜10 個にしてください。",
            ));
        }
        let mut values = HashSet::with_capacity(self.options.len());
        let mut selected = 0;
        for option in &self.options {
            if option.label.trim().is_empty()
                || option.label.chars().count() > 100
                || !(1..=100).contains(&option.value.chars().count())
                || option
                    .description
                    .as_ref()
                    .is_some_and(|text| text.chars().count() > 100)
            {
                return Err(Error::InvalidUi(
                    "選択肢のラベルと値は 1〜100 文字、説明は 100 文字以下にしてください。",
                ));
            }
            if !values.insert(&option.value) {
                return Err(Error::InvalidUi("選択肢の値は重複できません。"));
            }
            selected += usize::from(option.default == Some(true));
        }
        if selected > 1 {
            return Err(Error::InvalidUi("ラジオグループの初期選択は一つまでです。"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) enum ModalField {
    Standard(Component),
    Radio(RadioGroup),
}

/// Discord の入力フォームです。1 つ以上の入力欄を追加してから開きます。
#[derive(Clone, Debug)]
#[must_use]
pub struct Modal {
    pub(crate) custom_id: String,
    pub(crate) title: String,
    pub(crate) components: Vec<ModalField>,
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
    pub fn text_input(self, input: TextInput) -> Self {
        self.label(input.label, None, Component::TextInput(input.input))
    }

    /// ファイル添付欄を追加します。入力欄はほかの種類と合わせて 5 欄までです。
    pub fn file_upload(self, input: FileUpload) -> Self {
        self.label(
            input.label,
            input.description,
            Component::FileUpload(input.input),
        )
    }

    /// チェックボックスを追加します。入力欄はほかの種類と合わせて 5 欄までです。
    pub fn checkbox(self, input: Checkbox) -> Self {
        self.label(
            input.label,
            input.description,
            Component::Checkbox(input.input),
        )
    }

    /// チェックボックスグループを追加します。入力欄はほかの種類と合わせて 5 欄までです。
    pub fn checkbox_group(self, input: CheckboxGroup) -> Self {
        self.label(
            input.label,
            input.description,
            Component::CheckboxGroup(input.input),
        )
    }

    /// ラジオグループを追加します。入力欄はほかの種類と合わせて 5 欄までです。
    pub fn radio_group(mut self, input: RadioGroup) -> Self {
        self.components.push(ModalField::Radio(input));
        self
    }

    /// 文字列選択メニューを入力欄として追加し、選択を必須にするか指定します。
    pub fn select(self, label: impl Into<String>, select: StringSelect, required: bool) -> Self {
        self.select_input(label, select.0, required)
    }

    /// ユーザー選択メニューを入力欄として追加し、選択を必須にするか指定します。
    pub fn user_select(self, label: impl Into<String>, select: UserSelect, required: bool) -> Self {
        self.select_input(label, select.0, required)
    }

    /// ロール選択メニューを入力欄として追加し、選択を必須にするか指定します。
    pub fn role_select(self, label: impl Into<String>, select: RoleSelect, required: bool) -> Self {
        self.select_input(label, select.0, required)
    }

    /// チャンネル選択メニューを入力欄として追加し、選択を必須にするか指定します。
    pub fn channel_select(
        self,
        label: impl Into<String>,
        select: ChannelSelect,
        required: bool,
    ) -> Self {
        self.select_input(label, select.0, required)
    }

    /// ユーザーとロールの選択メニューを入力欄として追加し、選択を必須にするか指定します。
    pub fn mentionable_select(
        self,
        label: impl Into<String>,
        select: MentionableSelect,
        required: bool,
    ) -> Self {
        self.select_input(label, select.0, required)
    }

    fn select_input(
        self,
        label: impl Into<String>,
        mut select: SelectMenu,
        required: bool,
    ) -> Self {
        select.required = Some(required);
        self.label(label.into(), None, Component::SelectMenu(select))
    }

    fn label(mut self, label: String, description: Option<String>, component: Component) -> Self {
        self.components
            .push(ModalField::Standard(Component::Label(Label {
                component: Box::new(component),
                description,
                id: None,
                label,
            })));
        self
    }

    /// フォームの構成を Discord に送る前に検証します。
    ///
    /// # エラー
    ///
    /// タイトル、入力項目、識別子などが Discord の制約に合わない場合は失敗します。
    pub fn validate(&self) -> Result<()> {
        validate_custom_id(&self.custom_id)?;
        if self.title.chars().nth(45).is_some() || self.title.trim().is_empty() {
            return Err(Error::InvalidUi(
                "モーダルのタイトルは空白以外を含む 1〜45 文字にしてください。",
            ));
        }
        if !(1..=5).contains(&self.components.len()) {
            return Err(Error::InvalidUi("モーダルには入力欄が 1〜5 個必要です。"));
        }

        let mut ids = HashSet::new();
        for item in &self.components {
            let component = match item {
                ModalField::Radio(input) => {
                    input.validate(&mut ids)?;
                    continue;
                }
                ModalField::Standard(component) => component,
            };
            let Component::Label(label) = component else {
                return Err(Error::InvalidUi("入力欄は Label の中に配置してください。"));
            };
            component::label(label)?;
            if label.label.trim().is_empty() {
                return Err(Error::InvalidUi("入力欄には空でないラベルが必要です。"));
            }
            match label.component.as_ref() {
                Component::TextInput(input) => {
                    check_custom_id(&mut ids, &input.custom_id)?;
                    check_text_input_value(input)?;
                }
                Component::FileUpload(input) => {
                    check_custom_id(&mut ids, &input.custom_id)?;
                    check_file_upload_count(input)?;
                }
                Component::Checkbox(input) => check_custom_id(&mut ids, &input.custom_id)?,
                Component::CheckboxGroup(input) => {
                    check_custom_id(&mut ids, &input.custom_id)?;
                    check_checkbox_group(input)?;
                }
                Component::SelectMenu(select) => {
                    check_select(select, &mut ids)?;
                    if select.required == Some(true) && select.min_values == Some(0) {
                        return Err(Error::InvalidUi(
                            "必須の選択欄には 1 以上の最小選択数を指定してください。",
                        ));
                    }
                }
                _ => return Err(Error::InvalidUi("モーダルの入力欄の種類が無効です。")),
            }
        }

        Ok(())
    }

    pub(crate) fn response(&self) -> Result<InteractionResponse> {
        self.validate()?;
        let components = self
            .components
            .iter()
            .map(|item| match item {
                ModalField::Standard(component) => Ok(component.clone()),
                ModalField::Radio(_) => Err(Error::InvalidUi(
                    "ラジオグループは拡張モーダルとして送信してください。",
                )),
            })
            .collect::<Result<_>>()?;
        Ok(InteractionResponse {
            kind: InteractionResponseType::Modal,
            data: Some(InteractionResponseData {
                custom_id: Some(self.custom_id.clone()),
                title: Some(self.title.clone()),
                components: Some(components),
                ..Default::default()
            }),
        })
    }
}

fn check_text_input_value(input: &RawTextInput) -> Result<()> {
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
    Ok(())
}

fn check_file_upload_count(input: &RawFileUpload) -> Result<()> {
    let min = input.min_values.unwrap_or(1);
    let max = input.max_values.unwrap_or(1);
    if min > max {
        return Err(Error::InvalidUi(
            "添付の最小ファイル数は最大ファイル数以下にしてください。",
        ));
    }
    if min == 0 && input.required.unwrap_or(true) {
        return Err(Error::InvalidUi(
            "必須の添付欄には 1 以上の最小ファイル数を指定してください。",
        ));
    }
    Ok(())
}

fn check_checkbox_group(input: &RawCheckboxGroup) -> Result<()> {
    let count = input.options.len();
    if !(1..=10).contains(&count) {
        return Err(Error::InvalidUi(
            "チェックボックスグループの選択肢は 1〜10 個にしてください。",
        ));
    }
    let min = usize::from(input.min_values.unwrap_or(1));
    let max = input.max_values.map_or(count, usize::from);
    if min > max || max > count {
        return Err(Error::InvalidUi(
            "チェックボックスの最小選択数は最大選択数以下にし、最大選択数は選択肢の総数以下にしてください。",
        ));
    }
    let mut values = HashSet::with_capacity(count);
    let mut selected = 0;
    for option in &input.options {
        if option.label.trim().is_empty()
            || option.label.chars().count() > 100
            || !(1..=100).contains(&option.value.chars().count())
            || option
                .description
                .as_ref()
                .is_some_and(|text| text.chars().count() > 100)
        {
            return Err(Error::InvalidUi(
                "選択肢のラベルと値は 1〜100 文字、説明は 100 文字以下にしてください。",
            ));
        }
        if !values.insert(&option.value) {
            return Err(Error::InvalidUi("選択肢の値は重複できません。"));
        }
        selected += usize::from(option.default == Some(true));
    }
    if selected > max {
        return Err(Error::InvalidUi(
            "選択済みの項目数は最大選択数以下にしてください。",
        ));
    }
    Ok(())
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

    /// メッセージの本文、Embed、操作部品を Discord に送る前に検証します。
    ///
    /// # エラー
    ///
    /// メッセージが空か、文字数や配置などが Discord の制約に合わない場合は失敗します。
    pub fn validate(&self) -> Result<()> {
        let content = self.content.as_deref().unwrap_or_default();
        crate::validation::message_content(content)?;
        if content.trim().is_empty() && self.embeds.is_empty() && self.components.is_empty() {
            return Err(Error::InvalidUi(
                "本文、埋め込み、ボタン、選択メニューのいずれかを追加してください。",
            ));
        }
        crate::validation::embeds(&self.embeds)?;
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
    if row.components.len() > component::ACTION_ROW_COMPONENT_COUNT {
        return Err(Error::InvalidUi("1 行の部品は 5 個までです。"));
    }
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
    if button.label.as_deref().is_some_and(|label| {
        exceeds_character_limit(label, component::COMPONENT_BUTTON_LABEL_LENGTH)
    }) {
        return Err(Error::InvalidUi(
            "ボタンのラベルは 80 文字以下にしてください。",
        ));
    }
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
        if exceeds_character_limit(url, 512) || !is_absolute_http_url(url) {
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

fn is_absolute_http_url(url: &str) -> bool {
    let target = url.split_once('#').map_or(url, |(target, _)| target);
    if url.chars().any(|ch| ch.is_whitespace() || ch.is_control()) {
        return false;
    }
    let Ok(uri) = target.parse::<Uri>() else {
        return false;
    };
    matches!(uri.scheme_str(), Some("http" | "https"))
        && uri.authority().is_some_and(|authority| {
            let host = authority.host();
            let address = authority.as_str().rsplit('@').next().unwrap_or_default();
            !host.is_empty()
                && (matches!(address.strip_prefix(host), Some("" | ":"))
                    || authority.port_u16().is_some())
        })
}

fn check_select<'a>(select: &'a SelectMenu, ids: &mut HashSet<&'a str>) -> Result<()> {
    check_custom_id(ids, &select.custom_id)?;
    if select.placeholder.as_deref().is_some_and(|placeholder| {
        exceeds_character_limit(placeholder, component::SELECT_PLACEHOLDER_LENGTH)
    }) {
        return Err(Error::InvalidUi(
            "選択メニューのプレースホルダーは 150 文字以下にしてください。",
        ));
    }
    let min = select.min_values.unwrap_or(1);
    let max = select.max_values.unwrap_or(1);
    if !(component::SELECT_MAXIMUM_VALUES_REQUIREMENT..=component::SELECT_MAXIMUM_VALUES_LIMIT)
        .contains(&usize::from(max))
    {
        return Err(Error::InvalidUi(
            "選択できる最大数は 1〜25 にしてください。",
        ));
    }
    if min > max {
        return Err(Error::InvalidUi(
            "選択できる最小数は最大数以下にしてください。",
        ));
    }
    if select.kind == SelectMenuType::Text {
        let options = select.options.as_deref().unwrap_or_default();
        if options.len() > component::SELECT_OPTION_COUNT {
            return Err(Error::InvalidUi("選択肢は 25 個までです。"));
        }
        if options.is_empty() || usize::from(max) > options.len() {
            return Err(Error::InvalidUi(
                "選択メニューには選択できる数以上の選択肢が必要です。",
            ));
        }
        let mut values = HashSet::new();
        let mut selected = 0;
        for option in options {
            if exceeds_character_limit(&option.label, component::SELECT_OPTION_LABEL_LENGTH)
                || exceeds_character_limit(&option.value, component::SELECT_OPTION_VALUE_LENGTH)
                || option.description.as_deref().is_some_and(|description| {
                    exceeds_character_limit(
                        description,
                        component::SELECT_OPTION_DESCRIPTION_LENGTH,
                    )
                })
            {
                return Err(Error::InvalidUi(
                    "選択肢の表示名、値、説明はそれぞれ 100 文字以下にしてください。",
                ));
            }
            if option.label.trim().is_empty() || option.value.is_empty() {
                return Err(Error::InvalidUi("選択肢には空でない表示名と値が必要です。"));
            }
            if !values.insert(&option.value) {
                return Err(Error::InvalidUi("選択肢の値は重複できません。"));
            }
            selected += usize::from(option.default);
        }
        if selected > usize::from(max) {
            return Err(Error::InvalidUi(
                "選択済みの項目数は最大選択数以下にしてください。",
            ));
        }
    }
    if let Some(defaults) = &select.default_values {
        check_select_defaults(select.kind, defaults, min, max)?;
    }
    Ok(())
}

fn check_select_defaults(
    kind: SelectMenuType,
    defaults: &[SelectDefaultValue],
    min: u8,
    max: u8,
) -> Result<()> {
    if !(usize::from(min)..=usize::from(max)).contains(&defaults.len()) {
        return Err(Error::InvalidUi(
            "初期選択の個数を最小・最大選択数の範囲内にしてください。",
        ));
    }
    let mut unique = HashSet::new();
    for value in defaults {
        if !matches!(
            (kind, value),
            (
                SelectMenuType::User | SelectMenuType::Mentionable,
                SelectDefaultValue::User(_)
            ) | (
                SelectMenuType::Role | SelectMenuType::Mentionable,
                SelectDefaultValue::Role(_)
            ) | (SelectMenuType::Channel, SelectDefaultValue::Channel(_))
        ) {
            return Err(Error::InvalidUi(
                "初期選択の値の種類が選択メニューと一致していません。",
            ));
        }
        if !unique.insert(value) {
            return Err(Error::InvalidUi("同じ値を初期選択に複数指定できません。"));
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

fn exceeds_character_limit(value: &str, limit: usize) -> bool {
    value.len() > limit
        && (value.len() > limit.saturating_mul(4) || value.chars().nth(limit).is_some())
}

fn validate_custom_id(id: &str) -> Result<()> {
    if id.is_empty() || exceeds_character_limit(id, component::COMPONENT_CUSTOM_ID_LENGTH) {
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
            "http://:80",
            "http://example.com:abc",
            "http://example.com:65536",
            "https://example.com/white space",
            "https://example.com/fragment#white space",
            "https://example.com/fragment#\0",
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
        Button, ButtonStyle, Component, Embed, Id, InteractionResponseType, Mentionable, Modal,
        SelectDefaultValue, SelectOption, StringSelect, TextInput, TextInputStyle, UiMessage,
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
            .max_values(2)
            .default_values([Id::new(11)]);
        let role = RoleSelect::new("role")
            .disabled(true)
            .default_values([Id::new(22)]);
        let channel = ChannelSelect::new("channel")
            .channel_types(vec![ChannelType::GuildText])
            .default_values([Id::new(33)]);
        let mentionable = MentionableSelect::new("mentionable")
            .placeholder("ユーザーやロールを選択")
            .min_values(1)
            .max_values(3)
            .disabled(true)
            .default_values([
                Mentionable::User(Id::new(11)),
                Mentionable::Role(Id::new(22)),
            ]);

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
        for (item, kind, defaults) in [
            (
                user,
                SelectMenuType::User,
                &[SelectDefaultValue::User(Id::new(11))][..],
            ),
            (
                role,
                SelectMenuType::Role,
                &[SelectDefaultValue::Role(Id::new(22))][..],
            ),
            (
                channel,
                SelectMenuType::Channel,
                &[SelectDefaultValue::Channel(Id::new(33))][..],
            ),
            (
                mentionable,
                SelectMenuType::Mentionable,
                &[
                    SelectDefaultValue::User(Id::new(11)),
                    SelectDefaultValue::Role(Id::new(22)),
                ][..],
            ),
        ] {
            let Component::ActionRow(row) = item else {
                panic!("Action Row を想定")
            };
            let [Component::SelectMenu(select)] = row.components.as_slice() else {
                panic!("選択メニュー 1 個を想定")
            };
            assert_eq!(select.kind, kind);
            assert!(select.options.is_none());
            assert_eq!(select.default_values.as_deref(), Some(defaults));
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
    fn rejects_invalid_entity_defaults_and_allows_clearing_them() {
        use super::{ComponentsV2, RoleSelect, UserSelect};

        let defaults = UserSelect::new("user").default_values([Id::new(11), Id::new(12)]);
        assert!(
            UiMessage::new()
                .user_select(defaults.clone())
                .validate()
                .is_err()
        );
        let cleared = defaults.default_values([]).min_values(2).max_values(2);
        assert!(cleared.0.default_values.is_none());
        assert!(ComponentsV2::new().user_select(cleared).validate().is_ok());

        let repeated = UserSelect::new("user")
            .max_values(2)
            .default_values([Id::new(11), Id::new(11)]);
        assert!(UiMessage::new().user_select(repeated).validate().is_err());
        let insufficient = RoleSelect::new("role")
            .min_values(2)
            .max_values(2)
            .default_values([Id::new(22)]);
        assert!(
            UiMessage::new()
                .role_select(insufficient)
                .validate()
                .is_err()
        );
        let mut wrong_kind = UserSelect::new("user");
        wrong_kind.0.default_values = Some(vec![SelectDefaultValue::Role(Id::new(22))]);
        assert!(
            ComponentsV2::new()
                .user_select(wrong_kind)
                .validate()
                .is_err()
        );
    }

    #[test]
    fn string_options_can_be_preselected_up_to_the_maximum() {
        use super::ComponentsV2;

        let blue = SelectOption::new("青", "blue").selected_by_default(true);
        let green = SelectOption::new("緑", "green").selected_by_default(true);
        let invalid = StringSelect::new("color", vec![blue.clone(), green.clone()]);
        assert!(UiMessage::new().select(invalid).validate().is_err());
        let valid = StringSelect::new("color", vec![blue, green.selected_by_default(false)]);
        let options = valid.0.options.as_deref().unwrap();
        assert!(matches!(options, [blue, green] if blue.default && !green.default));
        assert!(ComponentsV2::new().select(valid).validate().is_ok());
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
    fn builds_file_upload_as_a_labeled_modal_component() {
        use super::FileUpload;

        let response = Modal::new("feedback", "感想を書く")
            .text_input(TextInput::new("body", "感想"))
            .file_upload(
                FileUpload::new("evidence", "参考ファイル")
                    .description("スクリーンショットなど")
                    .min_values(0)
                    .max_values(3)
                    .required(false),
            )
            .response()
            .unwrap();
        let wire = serde_json::to_value(response).unwrap();
        assert_eq!(wire.pointer("/type"), Some(&serde_json::json!(9)));
        assert_eq!(
            wire.pointer("/data/custom_id"),
            Some(&serde_json::json!("feedback"))
        );
        assert_eq!(
            wire.pointer("/data/components/1"),
            Some(&serde_json::json!({
                "type": 18,
                "label": "参考ファイル",
                "description": "スクリーンショットなど",
                "component": {
                    "type": 19,
                    "custom_id": "evidence",
                    "min_values": 0,
                    "max_values": 3,
                    "required": false
                }
            }))
        );
    }

    #[test]
    fn rejects_invalid_file_upload_configuration_and_shared_field_ids() {
        use super::FileUpload;

        let file = FileUpload::new("evidence", "添付");
        for invalid in [
            file.clone().min_values(2),
            file.clone().min_values(0),
            file.clone().max_values(11),
            file.clone().description("あ".repeat(101)),
            FileUpload::new("evidence", " "),
        ] {
            assert!(
                Modal::new("feedback", "感想")
                    .file_upload(invalid)
                    .response()
                    .is_err()
            );
        }
        assert!(
            Modal::new("feedback", "感想")
                .text_input(TextInput::new("evidence", "感想"))
                .file_upload(file)
                .response()
                .is_err()
        );
    }

    #[test]
    fn serializes_checkboxes_as_labeled_optional_boolean_fields() {
        use super::Checkbox;

        let response = Modal::new("feedback", "感想を書く")
            .checkbox(
                Checkbox::new("updates", "更新情報")
                    .description("新しい返信があれば知らせます")
                    .selected_by_default(true),
            )
            .checkbox(Checkbox::new("share", "共有する"))
            .response()
            .unwrap();
        let wire = serde_json::to_value(response).unwrap();
        assert_eq!(
            wire.pointer("/data/components/0"),
            Some(&serde_json::json!({
                "type": 18,
                "label": "更新情報",
                "description": "新しい返信があれば知らせます",
                "component": {"type": 23, "custom_id": "updates", "default": true}
            }))
        );
        assert_eq!(
            wire.pointer("/data/components/1/component"),
            Some(&serde_json::json!({"type": 23, "custom_id": "share"}))
        );
    }

    #[test]
    fn validates_checkbox_labels_and_shared_modal_field_ids() {
        use super::{Checkbox, FileUpload};

        let form = || Modal::new("feedback", "感想を書く");
        for field in [
            Checkbox::new("", "共有する"),
            Checkbox::new("share", " "),
            Checkbox::new("share", "共有する").description("あ".repeat(101)),
        ] {
            assert!(form().checkbox(field).response().is_err());
        }
        let checkbox = Checkbox::new("share", "共有する");
        assert!(
            form()
                .text_input(TextInput::new("share", "感想"))
                .checkbox(checkbox.clone())
                .response()
                .is_err()
        );
        assert!(
            form()
                .checkbox(checkbox)
                .file_upload(FileUpload::new("share", "添付"))
                .response()
                .is_err()
        );
    }

    #[test]
    fn serializes_modal_checkbox_group_options_and_limits() {
        use super::{CheckboxGroup, CheckboxGroupOption};

        let response = Modal::new("poll", "アンケート")
            .checkbox_group(
                CheckboxGroup::new(
                    "colors",
                    "好きな色",
                    vec![
                        CheckboxGroupOption::new("青", "blue")
                            .description("濃い青")
                            .selected_by_default(true),
                        CheckboxGroupOption::new("緑", "green"),
                    ],
                )
                .description("最大2色まで")
                .required(false)
                .min_values(0)
                .max_values(2),
            )
            .response()
            .unwrap();
        let wire = serde_json::to_value(response).unwrap();
        assert_eq!(
            wire.pointer("/data/components/0"),
            Some(&serde_json::json!({
                "type": 18, "label": "好きな色", "description": "最大2色まで",
                "component": {
                    "type": 22, "custom_id": "colors", "required": false,
                    "min_values": 0, "max_values": 2,
                    "options": [
                        {"label": "青", "value": "blue", "description": "濃い青", "default": true},
                        {"label": "緑", "value": "green"}
                    ]
                }
            }))
        );
    }

    #[test]
    fn validates_modal_checkbox_group_counts_and_unique_values() {
        use super::{Checkbox, CheckboxGroup, CheckboxGroupOption};

        let response = |field| {
            Modal::new("survey", "アンケート")
                .checkbox_group(field)
                .response()
        };
        let group = |options| CheckboxGroup::new("choice", "選択", options);
        let option = CheckboxGroupOption::new("1", "one");
        assert!(response(group(vec![option.clone()])).is_ok());
        let maximum = (1..=10)
            .map(|value| CheckboxGroupOption::new(value.to_string(), value.to_string()))
            .collect::<Vec<_>>();
        assert!(response(group(maximum.clone())).is_ok());
        let repeated_defaults = vec![
            option.clone().selected_by_default(true),
            CheckboxGroupOption::new("2", "two").selected_by_default(true),
        ];
        for field in [
            group(vec![]),
            group(
                maximum
                    .into_iter()
                    .chain([CheckboxGroupOption::new("11", "11")])
                    .collect(),
            ),
            group(vec![option.clone()]).min_values(0),
            group(vec![option.clone()]).min_values(2),
            group(vec![option.clone()]).max_values(2),
            group(repeated_defaults).max_values(1),
            group(vec![option.clone(), option.clone()]),
            group(vec![CheckboxGroupOption::new("", "one")]),
            group(vec![CheckboxGroupOption::new("1", "")]),
            group(vec![CheckboxGroupOption::new("1", "あ".repeat(101))]),
            group(vec![option.clone().description("あ".repeat(101))]),
        ] {
            assert!(response(field).is_err());
        }
        assert!(
            Modal::new("survey", "アンケート")
                .checkbox(Checkbox::new("choice", "一つ選んでください"))
                .checkbox_group(group(vec![option]))
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
