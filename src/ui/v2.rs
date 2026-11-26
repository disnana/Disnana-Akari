use std::collections::HashSet;

use twilight_model::channel::message::{
    Component,
    component::{Container, Separator, SeparatorSpacingSize, TextDisplay},
};
use twilight_validate::{component::COMPONENT_V2_COUNT, embed::COLOR_MAXIMUM, message};

use crate::Result;

use super::{
    Button, ChannelSelect, MentionableSelect, RoleSelect, StringSelect, UserSelect, button_row,
    check_button, check_row, select_row,
};

const TOTAL_TEXT_LENGTH: usize = 4_000;

/// Components V2 専用のメッセージを組み立てます。本文の代わりに [`Self::text`] を使います。
#[derive(Clone, Debug, Default)]
#[must_use]
pub struct ComponentsV2 {
    pub(crate) components: Vec<Component>,
}

impl ComponentsV2 {
    /// 空のメッセージを作ります。送信前に部品を追加します。
    pub fn new() -> Self {
        Self::default()
    }

    /// Markdown を使えるテキスト表示を追加します。
    pub fn text(self, content: impl Into<String>) -> Self {
        self.component(Component::TextDisplay(TextDisplay {
            content: content.into(),
            id: None,
        }))
    }

    /// 横方向の区切り線を追加します。
    pub fn separator(self) -> Self {
        self.component(Component::Separator(Separator {
            divider: Some(true),
            id: None,
            spacing: None,
        }))
    }

    /// 1〜5 個のボタンを一つの行に追加します。
    pub fn buttons(self, buttons: Vec<Button>) -> Self {
        self.component(button_row(buttons))
    }

    /// 文字列選択メニューを一つの行として追加します。
    pub fn select(self, select: StringSelect) -> Self {
        self.component(select_row(select.0))
    }

    /// ユーザー選択メニューを一つの行として追加します。
    pub fn user_select(self, select: UserSelect) -> Self {
        self.component(select_row(select.0))
    }

    /// ロール選択メニューを一つの行として追加します。
    pub fn role_select(self, select: RoleSelect) -> Self {
        self.component(select_row(select.0))
    }

    /// チャンネル選択メニューを一つの行として追加します。
    pub fn channel_select(self, select: ChannelSelect) -> Self {
        self.component(select_row(select.0))
    }

    /// ユーザーとロールの混在する選択メニューを一つの行として追加します。
    pub fn mentionable_select(self, select: MentionableSelect) -> Self {
        self.component(select_row(select.0))
    }

    /// 複数の部品を、RGB 値で指定した色の付いたコンテナーにまとめます。
    pub fn container(self, accent_color: u32, contents: Self) -> Self {
        self.component(Component::Container(Container {
            accent_color: Some(Some(accent_color)),
            components: contents.components,
            id: None,
            spoiler: None,
        }))
    }

    /// Twilight の部品をそのまま追加します。送信前にメッセージ上の配置と制限を検証します。
    pub fn component(mut self, component: Component) -> Self {
        self.components.push(component);
        self
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.components.is_empty() {
            return Err("Components V2 には部品が一つ以上必要です。".into());
        }
        message::components(&self.components, true)?;
        let mut limits = Limits::default();
        for component in &self.components {
            limits.check(component)?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct Limits<'a> {
    count: usize,
    text_length: usize,
    custom_ids: HashSet<&'a str>,
}

impl<'a> Limits<'a> {
    fn check(&mut self, component: &'a Component) -> Result<()> {
        self.add_count(1)?;

        match component {
            Component::ActionRow(row) => {
                self.add_count(row.components.len())?;
                check_row(row, &mut self.custom_ids)?;
            }
            Component::TextDisplay(text) => {
                if text.content.trim().is_empty() {
                    return Err("テキスト表示には空白以外の内容が必要です。".into());
                }
                self.text_length += text.content.chars().count();
                if self.text_length > TOTAL_TEXT_LENGTH {
                    return Err("テキスト表示の合計は 4000 文字以下にしてください。".into());
                }
            }
            Component::Section(section) => {
                self.add_count(1)?; // 横に付けるボタンや画像も部品の個数に含めます。
                if let Component::Button(button) = section.accessory.as_ref() {
                    check_button(button, &mut self.custom_ids)?;
                }
                for child in &section.components {
                    self.check(child)?;
                }
            }
            Component::Container(container) => {
                if container.components.is_empty() {
                    return Err("コンテナーには部品が一つ以上必要です。".into());
                }
                if container
                    .accent_color
                    .flatten()
                    .is_some_and(|color| color > COLOR_MAXIMUM)
                {
                    return Err("コンテナーの色は 0x000000〜0xFFFFFF にしてください。".into());
                }
                for child in &container.components {
                    self.check(child)?;
                }
            }
            Component::Separator(separator) => {
                if matches!(separator.spacing, Some(SeparatorSpacingSize::Other(_))) {
                    return Err("区切り線の余白は Small または Large を指定してください。".into());
                }
            }
            Component::File(_) | Component::MediaGallery(_) => {}
            _ => return Err("この部品はメッセージ内のこの位置に配置できません。".into()),
        }
        Ok(())
    }

    fn add_count(&mut self, count: usize) -> Result<()> {
        self.count += count;
        if self.count > COMPONENT_V2_COUNT {
            return Err("Components V2 の部品は入れ子を含めて 40 個までです。".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::ComponentsV2;
    use crate::ui::{Button, SelectOption, StringSelect};

    #[test]
    fn accepts_text_and_interactive_components_in_container() {
        let contents = ComponentsV2::new()
            .text("# Akari")
            .separator()
            .buttons(vec![Button::primary("refresh", "更新")])
            .select(StringSelect::new(
                "color",
                vec![SelectOption::new("青", "blue")],
            ));

        assert!(
            ComponentsV2::new()
                .container(0x3970C4, contents)
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn counts_nested_children_and_checks_ids_across_containers() {
        let mut contents = ComponentsV2::new();
        for row in 0..7 {
            contents = contents.buttons(
                (0..5)
                    .map(|index| Button::primary(format!("{row}:{index}"), "更新"))
                    .collect(),
            );
        }
        assert!(
            ComponentsV2::new()
                .container(0, contents)
                .validate()
                .is_err()
        );

        let nested = ComponentsV2::new().buttons(vec![Button::primary("duplicate", "更新")]);
        assert!(
            ComponentsV2::new()
                .container(0, nested)
                .select(StringSelect::new(
                    "duplicate",
                    vec![SelectOption::new("青", "blue")]
                ))
                .validate()
                .is_err()
        );
    }

    #[test]
    fn limits_text_across_sections_of_message() {
        let body = "あ".repeat(2_000);
        let nested = ComponentsV2::new().text(&body);
        let message = ComponentsV2::new().container(0, nested).text(&body);
        assert!(message.validate().is_ok());
        assert!(message.text("い").validate().is_err());
    }
}
