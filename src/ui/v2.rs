use std::collections::HashSet;

use twilight_model::{
    channel::{
        Attachment as ReceivedAttachment,
        message::{
            Component,
            component::{
                Container, FileDisplay as RawFileDisplay, MediaGallery, MediaGalleryItem, Section,
                Separator, SeparatorSpacingSize, TextDisplay, Thumbnail, UnfurledMediaItem,
            },
        },
    },
    http::attachment::Attachment,
};
use twilight_validate::{
    component::{
        COMPONENT_V2_COUNT, MEDIA_GALLERY_ITEM_DESCRIPTION_LENGTH_MAX, MEDIA_GALLERY_ITEMS_MAX,
        MEDIA_GALLERY_ITEMS_MIN, SECTION_COMPONENTS_MAX, SECTION_COMPONENTS_MIN,
        TEXT_DISPLAY_CONTENT_LENGTH_MAX, THUMBNAIL_DESCRIPTION_LENGTH_MAX,
    },
    embed::COLOR_MAXIMUM,
    message,
};

use crate::{Error, Result};

use super::{
    Button, ChannelSelect, MentionableSelect, RoleSelect, StringSelect, UserSelect, button_row,
    check_button, check_row, exceeds_character_limit, is_absolute_http_url, select_row,
};

const TOTAL_TEXT_LENGTH: usize = 4_000;

/// ギャラリーやサムネイルに表示するメディアです。
#[derive(Clone, Debug)]
#[must_use]
pub struct Media(MediaGalleryItem);

impl Media {
    /// 外部の HTTP(S) URL またはメッセージの添付ファイル名（`attachment://ファイル名`）を指定します。
    ///
    /// この指定でファイルのアップロードは行いません。
    pub fn new(url: impl Into<String>) -> Self {
        Self(MediaGalleryItem {
            description: None,
            media: unfurled(url.into()),
            spoiler: None,
        })
    }

    /// メディアの代替テキストを設定します。最大 1024 文字です。
    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.0.description = Some(text.into());
        self
    }

    /// `true` を指定すると、利用者が開くまでメディアを隠します。
    pub fn spoiler(mut self, spoiler: bool) -> Self {
        self.0.spoiler = Some(spoiler);
        self
    }
}

/// メッセージに添付したファイルを表示します。
#[derive(Clone, Debug)]
#[must_use]
pub struct FileDisplay(RawFileDisplay);

impl FileDisplay {
    /// 添付ファイルを名前で参照します。同じ名前のファイルを
    /// [`ComponentsV2::attachment`] で添付するか、編集対象にすでにある名前を指定します。
    pub fn new(filename: impl AsRef<str>) -> Self {
        Self(RawFileDisplay {
            file: unfurled(format!("attachment://{}", filename.as_ref())),
            id: None,
            spoiler: None,
        })
    }

    /// 利用者が開くまでファイルを隠すかどうかを設定します。
    pub fn spoiler(mut self, spoiler: bool) -> Self {
        self.0.spoiler = Some(spoiler);
        self
    }
}

fn unfurled(url: String) -> UnfurledMediaItem {
    UnfurledMediaItem {
        content_type: None,
        height: None,
        proxy_url: None,
        url,
        width: None,
    }
}

/// Components V2 専用のメッセージを組み立てます。本文の代わりに [`Self::text`] を使います。
#[derive(Clone, Default)]
#[must_use]
pub struct ComponentsV2 {
    pub(crate) components: Vec<Component>,
    pub(crate) attachments: Vec<Attachment>,
}

impl std::fmt::Debug for ComponentsV2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentsV2")
            .field("components", &self.components)
            .field("attachment_count", &self.attachments.len())
            .finish()
    }
}

impl ComponentsV2 {
    /// 空のメッセージを作ります。送信前に部品を追加します。
    pub fn new() -> Self {
        Self::default()
    }

    /// Markdown を使えるテキスト表示を追加します。
    pub fn text(self, content: impl Into<String>) -> Self {
        self.component(text_display(content))
    }

    /// 1〜3 個のテキスト表示を、横に付けたボタンと一緒に表示します。
    pub fn section_with_button(
        self,
        text: impl IntoIterator<Item = impl Into<String>>,
        button: Button,
    ) -> Self {
        self.section(text, Component::Button(button.0))
    }

    /// 1〜3 個のテキスト表示を、横に付けた画像と一緒に表示します。
    pub fn section_with_thumbnail(
        self,
        text: impl IntoIterator<Item = impl Into<String>>,
        thumbnail: Media,
    ) -> Self {
        let MediaGalleryItem {
            description,
            media,
            spoiler,
        } = thumbnail.0;
        self.section(
            text,
            Component::Thumbnail(Thumbnail {
                description: description.map(Some),
                id: None,
                media,
                spoiler,
            }),
        )
    }

    fn section(
        self,
        text: impl IntoIterator<Item = impl Into<String>>,
        accessory: Component,
    ) -> Self {
        self.component(Component::Section(Section {
            accessory: Box::new(accessory),
            components: text.into_iter().map(text_display).collect(),
            id: None,
        }))
    }

    /// 1〜10 件のメディアをギャラリーに並べます。
    pub fn media_gallery(self, items: impl IntoIterator<Item = Media>) -> Self {
        self.component(Component::MediaGallery(MediaGallery {
            id: None,
            items: items.into_iter().map(|item| item.0).collect(),
        }))
    }

    /// 添付ファイルの表示部品を追加します。
    pub fn file(self, file: FileDisplay) -> Self {
        self.component(Component::File(file.0))
    }

    /// メッセージと同じリクエストでアップロードするファイルを追加します。
    ///
    /// 一つのメッセージに添付できるファイルは最大 10 個です。
    /// ファイル名と添付 ID が重複しないようにしてください。
    /// メッセージを更新する場合は、元の添付ファイルを残して追加します。
    /// 表示位置は [`Self::file`] や [`Self::media_gallery`] で指定できます。
    pub fn attachment(mut self, attachment: Attachment) -> Self {
        self.attachments.push(attachment);
        self
    }

    /// 横方向の区切り線を追加します。
    pub fn separator(self) -> Self {
        self.with_separator(None, true)
    }

    /// 余白のサイズと区切り線の有無を指定します。
    ///
    /// `divider` が `false` の場合は余白だけを追加します。
    /// サイズには `Small` または `Large` を指定します。
    pub fn separator_with_spacing(self, spacing: SeparatorSpacingSize, divider: bool) -> Self {
        self.with_separator(Some(spacing), divider)
    }

    fn with_separator(self, spacing: Option<SeparatorSpacingSize>, divider: bool) -> Self {
        self.component(Component::Separator(Separator {
            divider: Some(divider),
            id: None,
            spacing,
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
        self.with_container(Some(accent_color), None, contents)
    }

    /// 複数の部品をコンテナーにまとめます。色とスポイラー表示を指定できます。
    ///
    /// 色に `None` を指定すると色の帯を省略し、`spoiler` が `true` の場合は
    /// 利用者が開くまでコンテナーの中身を隠します。
    pub fn container_with_options(
        self,
        accent_color: Option<u32>,
        spoiler: bool,
        contents: Self,
    ) -> Self {
        self.with_container(accent_color, Some(spoiler), contents)
    }

    fn with_container(
        mut self,
        accent_color: Option<u32>,
        spoiler: Option<bool>,
        contents: Self,
    ) -> Self {
        self.attachments.extend(contents.attachments);
        self.component(Component::Container(Container {
            accent_color: accent_color.map(Some),
            components: contents.components,
            id: None,
            spoiler,
        }))
    }

    /// Twilight の部品をそのまま追加します。送信前にメッセージ上の配置と制限を検証します。
    pub fn component(mut self, component: Component) -> Self {
        self.components.push(component);
        self
    }

    /// 部品の配置、テキスト、追加する添付ファイルの上限やメタデータを送信前に検証します。
    ///
    /// 明示した数値 ID はメッセージ内で一意にしてください。ID の省略と `0` は Discord が自動採番します。
    /// 操作用の `custom_id` には独立した重複チェックを行います。
    /// 編集前のメッセージや操作が持つ情報が必要な検証は、実際に送信・編集するときに行います。
    ///
    /// # エラー
    /// 空のメッセージ、無効な部品や添付、Discord の上限を超える内容で失敗します。
    pub fn validate(&self) -> Result<()> {
        if self.components.is_empty() {
            return Err(Error::InvalidUi(
                "Components V2 には部品が一つ以上必要です。",
            ));
        }
        let mut limits = Limits::default();
        for component in &self.components {
            limits.check(component)?;
        }
        message::components(&self.components, true)?;
        self.validate_attachments()
    }

    fn validate_attachments(&self) -> Result<()> {
        crate::validation::attachment_count(0, self.attachments.len())?;
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for attachment in &self.attachments {
            message::attachment(attachment)?;
            if !valid_attachment_name(&attachment.filename) {
                return Err(Error::InvalidUi("添付ファイルにはファイル名が必要です。"));
            }
            if !ids.insert(attachment.id) || !names.insert(attachment.filename.as_str()) {
                return Err(Error::InvalidUi(
                    "添付 ID とファイル名は、メッセージ内で重複できません。",
                ));
            }
        }
        Ok(())
    }

    /// 添付一覧が分かる送信経路でだけ、部品からのファイル参照を確認します。
    pub(crate) fn validate_attachment_references(
        &self,
        retained: &[ReceivedAttachment],
    ) -> Result<()> {
        crate::validation::attachment_count(retained.len(), self.attachments.len())?;
        let check = |url: &str| {
            if let Some(filename) = attachment_filename(url)
                && !self
                    .attachments
                    .iter()
                    .any(|file| file.filename == filename)
                && !retained.iter().any(|file| file.filename == filename)
            {
                return Err(Error::InvalidUi(
                    "attachment:// の参照先を、このメッセージの添付に指定してください。",
                ));
            }
            Ok(())
        };
        for component in &self.components {
            check_component_file_references(component, &check)?;
        }
        Ok(())
    }

    pub(crate) fn validate_attachment_sizes(&self, limit: u64) -> Result<()> {
        crate::validation::attachment_sizes(&self.attachments, limit)
    }
}

fn check_component_file_references(
    component: &Component,
    check: &impl Fn(&str) -> Result<()>,
) -> Result<()> {
    match component {
        Component::File(file) => check(&file.file.url)?,
        Component::Thumbnail(thumbnail) => check(&thumbnail.media.url)?,
        Component::MediaGallery(gallery) => {
            for item in &gallery.items {
                check(&item.media.url)?;
            }
        }
        Component::Section(section) => check_component_file_references(&section.accessory, check)?,
        Component::Container(container) => {
            for child in &container.components {
                check_component_file_references(child, check)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn text_display(content: impl Into<String>) -> Component {
    Component::TextDisplay(TextDisplay {
        content: content.into(),
        id: None,
    })
}

fn check_media_url(url: &str) -> Result<()> {
    if attachment_filename(url).is_some() || is_absolute_http_url(url) {
        return Ok(());
    }
    Err(Error::InvalidUi(
        "メディアには絶対 HTTP(S) URL または attachment:// に続けて添付ファイル名を指定してください。",
    ))
}

fn attachment_filename(url: &str) -> Option<&str> {
    url.strip_prefix("attachment://")
        .filter(|filename| valid_attachment_name(filename))
}

fn valid_attachment_name(filename: &str) -> bool {
    !filename.trim().is_empty()
        && !matches!(filename, "." | "..")
        && !filename.contains(['/', '\\'])
        && !filename.chars().any(char::is_control)
}

#[derive(Default)]
struct Limits<'a> {
    count: usize,
    text_length: usize,
    custom_ids: HashSet<&'a str>,
    ids: HashSet<i32>,
}

impl<'a> Limits<'a> {
    fn check(&mut self, component: &'a Component) -> Result<()> {
        self.add_count(1)?;
        self.check_id(component)?;

        match component {
            Component::ActionRow(row) => {
                self.add_count(row.components.len())?;
                check_row(row, &mut self.custom_ids)?;
                for child in &row.components {
                    self.check_id(child)?;
                }
            }
            Component::TextDisplay(text) => self.check_text(&text.content)?,
            Component::Section(section) => {
                if !(SECTION_COMPONENTS_MIN..=SECTION_COMPONENTS_MAX)
                    .contains(&section.components.len())
                {
                    return Err(Error::InvalidUi(
                        "セクションにはテキスト表示を 1〜3 個指定してください。",
                    ));
                }
                self.add_count(1)?; // 横に付けるボタンや画像も部品の個数に含めます。
                self.check_id(&section.accessory)?;
                match section.accessory.as_ref() {
                    Component::Button(button) => check_button(button, &mut self.custom_ids)?,
                    Component::Thumbnail(thumbnail) => check_thumbnail(thumbnail)?,
                    _ => {
                        return Err(Error::InvalidUi(
                            "セクションの横に置ける部品はボタンまたはサムネイルです。",
                        ));
                    }
                }
                for child in &section.components {
                    self.check(child)?;
                }
            }
            Component::Container(container) => {
                if container.components.is_empty() {
                    return Err(Error::InvalidUi("コンテナーには部品が一つ以上必要です。"));
                }
                if container
                    .accent_color
                    .flatten()
                    .is_some_and(|color| color > COLOR_MAXIMUM)
                {
                    return Err(Error::InvalidUi(
                        "コンテナーの色は 0x000000〜0xFFFFFF にしてください。",
                    ));
                }
                for child in &container.components {
                    self.check(child)?;
                }
            }
            Component::Separator(separator) => {
                if matches!(separator.spacing, Some(SeparatorSpacingSize::Other(_))) {
                    return Err(Error::InvalidUi(
                        "区切り線の余白は Small または Large を指定してください。",
                    ));
                }
            }
            Component::MediaGallery(gallery) => check_gallery(gallery)?,
            Component::File(file) => {
                if attachment_filename(&file.file.url).is_none() {
                    return Err(Error::InvalidUi(
                        "File コンポーネントには attachment:// に続けて添付ファイル名を指定してください。",
                    ));
                }
            }
            _ => {
                return Err(Error::InvalidUi(
                    "この部品はメッセージ内のこの位置に配置できません。",
                ));
            }
        }
        Ok(())
    }

    fn check_id(&mut self, component: &Component) -> Result<()> {
        let id = match component {
            Component::ActionRow(value) => value.id,
            Component::Button(value) => value.id,
            Component::Container(value) => value.id,
            Component::File(value) => value.id,
            Component::MediaGallery(value) => value.id,
            Component::Section(value) => value.id,
            Component::SelectMenu(value) => value.id,
            Component::Separator(value) => value.id,
            Component::TextDisplay(value) => value.id,
            Component::Thumbnail(value) => value.id,
            _ => None, // メッセージ内に置けない型は配置の検証で弾きます。
        };
        if let Some(id) = id
            && id != 0
            && !self.ids.insert(id)
        {
            return Err(Error::InvalidUi(
                "Components V2 の数値 ID はメッセージ内で重複できません。",
            ));
        }
        Ok(())
    }

    fn check_text(&mut self, text: &str) -> Result<()> {
        let remaining = TOTAL_TEXT_LENGTH - self.text_length;
        let limit = remaining.min(TEXT_DISPLAY_CONTENT_LENGTH_MAX);
        let count = text.chars().take(limit + 1).count();
        if count > limit {
            let reason = if remaining < TEXT_DISPLAY_CONTENT_LENGTH_MAX {
                "テキスト表示の合計は 4000 文字以下にしてください。"
            } else {
                "一つのテキスト表示は 2000 文字以下にしてください。"
            };
            return Err(Error::InvalidUi(reason));
        }
        if text.trim().is_empty() {
            return Err(Error::InvalidUi(
                "テキスト表示には空白以外の内容が必要です。",
            ));
        }
        self.text_length += count;
        Ok(())
    }

    fn add_count(&mut self, count: usize) -> Result<()> {
        self.count += count;
        if self.count > COMPONENT_V2_COUNT {
            return Err(Error::InvalidUi(
                "Components V2 の部品は入れ子を含めて 40 個までです。",
            ));
        }
        Ok(())
    }
}

fn check_gallery(gallery: &MediaGallery) -> Result<()> {
    if !(MEDIA_GALLERY_ITEMS_MIN..=MEDIA_GALLERY_ITEMS_MAX).contains(&gallery.items.len()) {
        return Err(Error::InvalidUi(
            "メディアギャラリーにはメディアを 1〜10 個指定してください。",
        ));
    }
    for item in &gallery.items {
        if item.description.as_deref().is_some_and(|value| {
            exceeds_character_limit(value, MEDIA_GALLERY_ITEM_DESCRIPTION_LENGTH_MAX)
        }) {
            return Err(Error::InvalidUi(
                "メディアの代替テキストは 1024 文字以下にしてください。",
            ));
        }
        check_media_url(&item.media.url)?;
    }
    Ok(())
}

fn check_thumbnail(thumbnail: &Thumbnail) -> Result<()> {
    if thumbnail
        .description
        .as_ref()
        .and_then(|value| value.as_deref())
        .is_some_and(|value| exceeds_character_limit(value, THUMBNAIL_DESCRIPTION_LENGTH_MAX))
    {
        return Err(Error::InvalidUi(
            "サムネイルの代替テキストは 1024 文字以下にしてください。",
        ));
    }
    check_media_url(&thumbnail.media.url)
}

#[cfg(test)]
mod tests {
    use super::{Attachment, Component, ComponentsV2, FileDisplay, Media, SeparatorSpacingSize};
    use crate::ui::{Button, SelectOption, StringSelect};

    #[test]
    fn layout_options_encode_spacers_and_unaccented_spoilers() -> crate::Result<()> {
        let message = ComponentsV2::new()
            .separator()
            .container_with_options(
                None,
                true,
                ComponentsV2::new()
                    .text("details")
                    .separator_with_spacing(SeparatorSpacingSize::Large, false),
            )
            .container_with_options(Some(0), false, ComponentsV2::new().text("black"));
        message.validate()?;
        assert_eq!(
            serde_json::to_value(&message.components).map_err(crate::Error::handler)?,
            serde_json::json!([
                { "type": 14, "divider": true },
                {
                    "type": 17,
                    "spoiler": true,
                    "components": [
                        { "type": 10, "content": "details" },
                        { "type": 14, "divider": false, "spacing": 2 }
                    ]
                },
                {
                    "type": 17,
                    "accent_color": 0,
                    "spoiler": false,
                    "components": [{ "type": 10, "content": "black" }]
                }
            ])
        );
        assert!(
            ComponentsV2::new()
                .separator_with_spacing(SeparatorSpacingSize::Other(3), false)
                .validate()
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn file_names_and_uploads_are_validated_across_containers() -> crate::Result<()> {
        let file = |name: &str, id| Attachment::from_bytes(name.into(), b"bytes".to_vec(), id);
        let details = ComponentsV2::new()
            .file(FileDisplay::new("manual.pdf"))
            .attachment(file("manual.pdf", 0));
        let nested = ComponentsV2::new().container_with_options(None, true, details);
        nested.validate()?;
        nested.validate_attachment_references(&[])?;
        assert_eq!(nested.attachments.len(), 1);
        assert!(
            nested
                .clone()
                .attachment(file("again.pdf", 0))
                .validate()
                .is_err()
        );
        assert!(
            nested
                .clone()
                .attachment(file("manual.pdf", 1))
                .validate()
                .is_err()
        );
        assert!(
            nested
                .attachment(file("bad\"name.pdf", 0))
                .validate()
                .is_err()
        );
        for filename in ["", ".", "..", "../manual.pdf", "manual\n.pdf"] {
            assert!(
                ComponentsV2::new()
                    .file(FileDisplay::new(filename))
                    .validate()
                    .is_err()
            );
        }
        let raw = Component::File(super::RawFileDisplay {
            file: super::unfurled("https://example.com/manual.pdf".into()),
            id: None,
            spoiler: None,
        });
        assert!(ComponentsV2::new().component(raw).validate().is_err());
        assert!(
            ComponentsV2::new()
                .container_with_options(
                    None,
                    false,
                    ComponentsV2::new().file(FileDisplay::new("absent.pdf"))
                )
                .attachment(file("manual.pdf", 0))
                .validate_attachment_references(&[])
                .is_err()
        );
        assert!(
            ComponentsV2::new()
                .section_with_thumbnail(["photo"], Media::new("attachment://absent.png"))
                .validate_attachment_references(&[])
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn attachments_are_limited_to_ten_per_message_and_bytes_per_file() -> crate::Result<()> {
        let message = (0..10).fold(ComponentsV2::new().text("Files"), |message, id| {
            message.attachment(Attachment::from_bytes(format!("{id}.txt"), vec![1, 2], id))
        });
        message.validate()?;
        message.validate_attachment_sizes(2)?;
        assert!(message.validate_attachment_sizes(1).is_err());
        assert!(
            message
                .attachment(Attachment::from_bytes("extra.txt".into(), vec![3], 10))
                .validate()
                .is_err()
        );
        Ok(())
    }

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
    fn nonzero_numeric_ids_are_unique_for_every_message_component() -> crate::Result<()> {
        use crate::Error;
        use serde_json::json;

        let components: Vec<Component> = serde_json::from_value(json!([
            {
                "type": 17, "id": 12, "components": [
                    {
                        "type": 1, "id": 13, "components": [
                            { "type": 2, "id": 14, "style": 1, "label": "Run", "custom_id": "14" }
                        ]
                    },
                    {
                        "type": 9, "id": 15, "components": [
                            { "type": 10, "id": -16, "content": "Preview" }
                        ],
                        "accessory": { "type": 11, "id": 17, "media": { "url": "https://example.test/image.webp" } }
                    },
                    { "type": 13, "id": 18, "file": { "url": "attachment://manual.pdf" } },
                    {
                        "type": 12, "id": 19, "items": [
                            { "media": { "url": "https://example.test/image.webp" } }
                        ]
                    }
                ]
            },
            {
                "type": 1, "components": [
                    { "type": 5, "id": 20, "custom_id": "select" }
                ]
            },
            { "type": 14, "id": 21 },
            { "type": 14, "id": 0 },
            { "type": 10, "id": 0, "content": "auto" },
            { "type": 10, "id": 22, "content": "explicit" },
            { "type": 10, "content": "auto" }
        ]))
        .map_err(Error::handler)?;
        let message = components
            .into_iter()
            .fold(ComponentsV2::new(), ComponentsV2::component);
        message.validate()?;
        for id in [12, 13, 14, 15, -16, 17, 18, 19, 20, 21, 22] {
            let duplicate = Component::TextDisplay(super::TextDisplay {
                id: Some(id),
                content: "duplicate".into(),
            });
            assert!(matches!(
                message.clone().component(duplicate).validate(),
                Err(Error::InvalidUi(
                    "Components V2 の数値 ID はメッセージ内で重複できません。"
                ))
            ));
        }
        Ok(())
    }

    #[test]
    fn exact_component_and_text_bounds_accept_unicode_and_nested_components() {
        let contents = (0..39).fold(ComponentsV2::new(), |components, _| components.text("x"));
        let message = ComponentsV2::new().container(0, contents);
        assert!(message.validate().is_ok());
        assert!(message.separator().validate().is_err());
        assert!(
            ComponentsV2::new()
                .text("🦀".repeat(2000))
                .validate()
                .is_ok()
        );
        assert!(
            ComponentsV2::new()
                .text("🦀".repeat(2001))
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

    #[test]
    fn builds_section_accessories_and_gallery_items() {
        let message = ComponentsV2::new()
            .section_with_button(
                ["# サポート", "詳しくはボタンを押してください。"],
                Button::primary("help", "詳細"),
            )
            .section_with_thumbnail(
                ["# プレビュー"],
                Media::new("https://[2001:db8::1]:8080/preview.webp#image")
                    .description("プレビュー画像")
                    .spoiler(true),
            )
            .media_gallery([
                Media::new("attachment://screenshot.webp").description("スクリーンショット"),
                Media::new("https://example.test/recording.mp4"),
            ]);
        assert!(message.validate().is_ok());
        let [
            Component::Section(button),
            Component::Section(thumbnail),
            Component::MediaGallery(gallery),
        ] = message.components.as_slice()
        else {
            panic!("expected two sections and a gallery");
        };
        assert_eq!(button.components.len(), 2);
        assert!(matches!(
            button.accessory.as_ref(),
            Component::Button(button) if button.custom_id.as_deref() == Some("help")
        ));
        assert!(matches!(
            thumbnail.accessory.as_ref(),
            Component::Thumbnail(image)
                if image.description.as_ref().and_then(|description| description.as_deref())
                    == Some("プレビュー画像") && image.spoiler == Some(true)
                    && image.media.url.ends_with("preview.webp#image")
        ));
        assert!(matches!(
            gallery.items.as_slice(),
            [screenshot, recording]
                if screenshot.media.url == "attachment://screenshot.webp"
                    && screenshot.description.as_deref() == Some("スクリーンショット")
                    && recording.media.url == "https://example.test/recording.mp4"
        ));
    }

    #[test]
    fn validates_media_fields_counts_and_section_custom_ids() {
        let thumbnail = || Media::new("https://example.test/preview.webp");
        assert!(ComponentsV2::new().media_gallery([]).validate().is_err());
        assert!(
            ComponentsV2::new()
                .media_gallery((0..10).map(|_| thumbnail()))
                .validate()
                .is_ok()
        );
        assert!(
            ComponentsV2::new()
                .media_gallery((0..11).map(|_| thumbnail()))
                .validate()
                .is_err()
        );
        assert!(
            ComponentsV2::new()
                .section_with_thumbnail(["1", "2", "3", "4"], thumbnail())
                .validate()
                .is_err()
        );
        for url in [
            "relative.webp",
            "https://",
            "http://:80",
            "http://example.test:abc/preview.webp",
            "https://example.test/preview.webp#\0",
            "attachment://",
            "attachment://dir/file.webp",
        ] {
            assert!(
                ComponentsV2::new()
                    .media_gallery([Media::new(url)])
                    .validate()
                    .is_err()
            );
        }
        let description = "あ".repeat(1025);
        assert!(
            ComponentsV2::new()
                .section_with_thumbnail(["説明"], thumbnail().description(&description))
                .validate()
                .is_err()
        );
        assert!(
            ComponentsV2::new()
                .media_gallery([thumbnail().description(description)])
                .validate()
                .is_err()
        );
        assert!(
            ComponentsV2::new()
                .section_with_button(["説明"], Button::primary("duplicate", "開く"))
                .buttons(vec![Button::primary("duplicate", "開く")])
                .validate()
                .is_err()
        );
    }
}
