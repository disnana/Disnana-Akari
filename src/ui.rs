//! Discord の埋め込みとボタンを組み立てます。

use std::collections::HashSet;

use twilight_model::channel::message::{
    Component,
    component::{ActionRow, Button as RawButton, ButtonStyle},
    embed::{Embed as RawEmbed, EmbedField},
};
use twilight_validate::message;

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

/// Discord への返信に使う本文、埋め込み、ボタンの組み合わせです。
#[derive(Clone, Debug, Default)]
#[must_use]
pub struct UiMessage {
    pub(crate) content: Option<String>,
    pub(crate) embeds: Vec<RawEmbed>,
    pub(crate) components: Vec<Component>,
}

impl UiMessage {
    /// 空の返信を作ります。本文、埋め込み、ボタンのいずれかを追加してから送信します。
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
        self.components.push(Component::ActionRow(ActionRow {
            id: None,
            components: buttons
                .into_iter()
                .map(|button| Component::Button(button.0))
                .collect(),
        }));
        self
    }

    pub(crate) fn validate(&self) -> Result<()> {
        let content = self.content.as_deref().unwrap_or_default();
        if content.trim().is_empty() && self.embeds.is_empty() && self.components.is_empty() {
            return Err("本文、埋め込み、ボタンのいずれかを追加してください。".into());
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
                return Err("ボタンは Action Row の中に配置してください。".into());
            };
            if row.components.is_empty() {
                return Err("行にはボタンが一つ以上必要です。".into());
            }
            for component in &row.components {
                let Component::Button(button) = component else {
                    return Err("行に追加できるのはボタンです。".into());
                };
                let id = button.custom_id.as_deref().unwrap_or_default();
                let label = button.label.as_deref().unwrap_or_default();
                if id.is_empty() || label.trim().is_empty() {
                    return Err("ボタンには空でない ID とラベルが必要です。".into());
                }
                if !ids.insert(id) {
                    return Err("同じメッセージ内のボタン ID は重複できません。".into());
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Button, ButtonStyle, Component, Embed, UiMessage};

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
}
