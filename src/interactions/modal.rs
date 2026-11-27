use std::{borrow::Cow, time::Instant};

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use twilight_http::{request::Request, response::marker::EmptyBody, routing::Route};
use twilight_model::{
    channel::{
        ChannelType, Message,
        message::{
            Component,
            component::{ComponentType, SelectDefaultValue, SelectMenuOption},
        },
    },
    http::interaction::InteractionResponseType,
    id::{
        Id,
        marker::{ChannelMarker, InteractionMarker, RoleMarker, UserMarker},
    },
};

use super::{Mentionable, ModalContext, collect_selection, resolve_mentionable, submitted_field};
use crate::{
    Result, SelectionError,
    slash::InitialResponder,
    ui::{ComponentsV2, Modal, ModalField, RadioGroupOption, UiMessage},
};

const RADIO_GROUP_TYPE: u8 = 21;

pub(crate) struct RadioSubmission {
    custom_id: String,
    value: Option<String>,
}

// Twilight 0.18 の Unknown に失われる入力値だけを、同じ Gateway JSON から先に読む。
pub(crate) fn capture_modal_radios(json: &str) -> Result<Vec<RadioSubmission>> {
    #[derive(Deserialize)]
    struct Dispatch<'a> {
        #[serde(borrow)]
        d: Probe<'a>,
    }

    #[derive(Deserialize)]
    struct Probe<'a> {
        #[serde(rename = "type")]
        kind: u8,
        #[serde(borrow)]
        data: Option<&'a RawValue>,
    }

    #[derive(Deserialize)]
    struct Submission<'a> {
        #[serde(borrow)]
        components: Vec<Label<'a>>,
    }

    #[derive(Deserialize)]
    struct Label<'a> {
        #[serde(rename = "type")]
        kind: u8,
        #[serde(borrow)]
        component: Option<&'a RawValue>,
    }

    #[derive(Deserialize)]
    struct Child<'a> {
        #[serde(rename = "type")]
        kind: u8,
        #[serde(borrow)]
        custom_id: Option<Cow<'a, str>>,
    }

    #[derive(Deserialize)]
    struct Choice<'a> {
        #[serde(borrow)]
        value: &'a RawValue,
    }

    let unreadable = |_| SelectionError::Unreadable;
    let Dispatch { d } = serde_json::from_str(json).map_err(unreadable)?;
    if d.kind != 5 {
        return Ok(Vec::new());
    }
    let raw_data = d.data.ok_or(SelectionError::Unreadable)?;
    let data: Submission<'_> = serde_json::from_str(raw_data.get()).map_err(unreadable)?;
    let children = data
        .components
        .iter()
        .filter(|label| label.kind == u8::from(ComponentType::Label))
        .filter_map(|label| label.component)
        .map(|raw| {
            let child: Child<'_> = serde_json::from_str(raw.get()).map_err(unreadable)?;
            Ok((raw, child))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut selections = Vec::new();
    for (raw, field) in &children {
        if field.kind != RADIO_GROUP_TYPE {
            continue;
        }
        let custom_id = field
            .custom_id
            .as_deref()
            .ok_or(SelectionError::Unreadable)?;
        if !(1..=100).contains(&custom_id.chars().count()) || data.components.len() > 5 {
            return Err(SelectionError::Unreadable.into());
        }
        if children
            .iter()
            .filter(|(_, child)| child.custom_id.as_deref() == Some(custom_id))
            .count()
            > 1
        {
            return Err(SelectionError::Duplicate.into());
        }
        let choice: Choice<'_> = serde_json::from_str(raw.get()).map_err(unreadable)?;
        let value: Option<String> = serde_json::from_str(choice.value.get()).map_err(unreadable)?;
        if value
            .as_ref()
            .is_some_and(|value| !(1..=100).contains(&value.chars().count()))
        {
            return Err(SelectionError::Unreadable.into());
        }
        selections.push(RadioSubmission {
            custom_id: custom_id.to_owned(),
            value,
        });
    }
    Ok(selections)
}

macro_rules! submitted_values {
    ($method:ident, $component:ident, $value:ty, $description:literal) => {
        #[doc = $description]
        ///
        /// 入力欄がなければ `None`、選択がなければ空のスライスです。
        pub fn $method(&self, custom_id: &str) -> Option<&'a [$value]> {
            self.data.components.iter().find_map(|component| {
                submitted_field(component, |field| match field {
                    super::ModalInteractionComponent::$component(select)
                        if select.custom_id == custom_id =>
                    {
                        Some(select.values.as_slice())
                    }
                    _ => None,
                })
            })
        }
    };
}

impl<'a> ModalContext<'a> {
    /// モーダルの送信に対する元の応答を Discord から取得します。非公開の応答も取得できます。
    ///
    /// # エラー
    /// まだ応答していない、すでに削除した、または Discord が取得を受け付けない場合に失敗します。
    pub async fn original_response(&self) -> Result<Message> {
        self.response.original_response(self.initial).await
    }

    /// モーダルの送信に対する元の応答の本文を更新します。公開範囲と添付は維持し、メンション通知を抑止します。
    /// 遅延応答の直後に呼ぶと、処理中の表示を完成させます。
    ///
    /// # エラー
    /// 元の応答がない、Components V2 形式で送っている、または Discord が編集を受け付けない場合に失敗します。
    pub async fn edit_original_response(&mut self, content: &str) -> Result<()> {
        self.response
            .edit_original_response(self.initial, content)
            .await
    }

    /// 元の応答の本文・埋め込み・従来のコンポーネントを置き換えます。
    /// 残りの添付と公開範囲を維持し、遅延応答もこの形式で完成させられます。
    ///
    /// # エラー
    /// 元の応答や形式が対応しない、メッセージが無効、または Discord が編集を受け付けない場合に失敗します。
    pub async fn edit_original_ui(&mut self, message: &UiMessage) -> Result<()> {
        self.response.edit_original_ui(self.initial, message).await
    }

    /// 元の Components V2 応答の部品を置き換えます。公開範囲と添付は維持されます。
    /// 遅延応答から直接 Components V2 のメッセージを完成させることもできます。
    ///
    /// # エラー
    /// 元の応答や形式が対応しない、メッセージが無効、または Discord が編集を受け付けない場合に失敗します。
    pub async fn edit_original_v2(&mut self, message: &ComponentsV2) -> Result<()> {
        self.response.edit_original_v2(self.initial, message).await
    }

    /// モーダルの送信に対する元の応答または処理中の表示を削除します。
    /// モーダルを開いたボタン付きメッセージは削除の対象に含まれません。
    ///
    /// # エラー
    /// まだ応答していない、すでに削除した、または Discord が削除を受け付けない場合に失敗します。
    pub async fn delete_original_response(&mut self) -> Result<()> {
        self.response.delete_original_response(self.initial).await
    }

    /// 指定したラジオグループの選択値を借ります。
    ///
    /// 入力欄がなければ `None`、何も選択されていなければ `Some(None)` です。
    pub fn radio_group(&self, custom_id: &str) -> Option<Option<&'a str>> {
        self.radio
            .iter()
            .find(|field| field.custom_id == custom_id)
            .map(|field| field.value.as_deref())
    }

    submitted_values!(
        string_select,
        StringSelect,
        String,
        "指定した入力欄で選ばれた文字列を借ります。"
    );
    submitted_values!(
        user_select,
        UserSelect,
        Id<UserMarker>,
        "指定した入力欄で選ばれたユーザーの ID を借ります。"
    );
    submitted_values!(
        role_select,
        RoleSelect,
        Id<RoleMarker>,
        "指定した入力欄で選ばれたロールの ID を借ります。"
    );
    submitted_values!(
        channel_select,
        ChannelSelect,
        Id<ChannelMarker>,
        "指定した入力欄で選ばれたチャンネルの ID を借ります。"
    );
    submitted_values!(
        checkbox_group,
        CheckboxGroup,
        String,
        "指定したチェックボックスグループで選ばれた値を借ります。"
    );

    /// 指定した入力欄で選ばれたユーザーとロールを、Discord から届いた順に取得します。
    ///
    /// 入力欄がなければ `None`、何も選択されていなければ空の一覧です。
    ///
    /// # エラー
    /// 選択の重複や件数超過、解決データの不足や不一致がある場合に失敗します。
    pub fn mentionable_select(&self, custom_id: &str) -> Result<Option<Vec<Mentionable>>> {
        let values = self.data.components.iter().find_map(|component| {
            submitted_field(component, |field| match field {
                super::ModalInteractionComponent::MentionableSelect(select)
                    if select.custom_id == custom_id =>
                {
                    Some(&select.values)
                }
                _ => None,
            })
        });
        values
            .map(|values| {
                collect_selection(values, |id| {
                    resolve_mentionable(*id, self.data.resolved.as_ref())
                })
            })
            .transpose()
    }
}

impl InitialResponder<'_> {
    pub(crate) async fn modal(self, modal: &Modal) -> Result<()> {
        let started_at = self.info.measures_timing().then(Instant::now);
        let Some(request) =
            custom_modal_request(modal, self.interaction.id, &self.interaction.token)?
        else {
            return self.send(&modal.response()?).await;
        };
        self.http.request::<EmptyBody>(request).await?;
        self.report_sent(InteractionResponseType::Modal, started_at);
        Ok(())
    }
}

// Twilight 0.18 にはラジオグループがなく、選択メニューにメッセージ用の disabled が入る。
fn custom_modal_request(
    modal: &Modal,
    interaction_id: Id<InteractionMarker>,
    token: &str,
) -> Result<Option<Request>> {
    if !modal.components.iter().any(|field| {
        match field {
            ModalField::Radio(_) => true,
            ModalField::Standard(component) =>
                matches!(component, Component::Label(label) if matches!(*label.component, Component::SelectMenu(_))),
        }
    }) {
        return Ok(None);
    }

    modal.validate()?;
    let payload = CustomModal {
        kind: InteractionResponseType::Modal,
        data: CustomModalData {
            title: &modal.title,
            custom_id: &modal.custom_id,
            components: modal.components.iter().map(ModalComponent::from).collect(),
        },
    };
    Ok(Some(
        Request::builder(&Route::InteractionCallback {
            interaction_id: interaction_id.get(),
            interaction_token: token,
            with_response: false,
        })
        .use_authorization_token(false)
        .json(&payload)
        .build()?,
    ))
}

#[derive(Serialize)]
struct CustomModal<'a> {
    #[serde(rename = "type")]
    kind: InteractionResponseType,
    data: CustomModalData<'a>,
}

#[derive(Serialize)]
struct CustomModalData<'a> {
    title: &'a str,
    custom_id: &'a str,
    components: Vec<ModalComponent<'a>>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum ModalComponent<'a> {
    Standard(&'a Component),
    Label {
        #[serde(rename = "type")]
        kind: ComponentType,
        label: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<&'a str>,
        component: CustomField<'a>,
    },
}

impl<'a> From<&'a ModalField> for ModalComponent<'a> {
    fn from(field: &'a ModalField) -> Self {
        let component = match field {
            ModalField::Radio(radio) => {
                return Self::Label {
                    kind: ComponentType::Label,
                    label: &radio.label,
                    description: radio.description.as_deref(),
                    component: CustomField::Radio(ModalRadio {
                        kind: RADIO_GROUP_TYPE,
                        custom_id: &radio.custom_id,
                        options: &radio.options,
                        required: radio.required,
                    }),
                };
            }
            ModalField::Standard(component) => component,
        };
        if let Component::Label(label) = component
            && let Component::SelectMenu(select) = label.component.as_ref()
        {
            return Self::Label {
                kind: ComponentType::Label,
                label: &label.label,
                description: label.description.as_deref(),
                component: CustomField::Select(ModalSelect {
                    kind: label.component.kind(),
                    custom_id: &select.custom_id,
                    channel_types: select.channel_types.as_deref(),
                    default_values: select.default_values.as_deref(),
                    max_values: select.max_values,
                    min_values: select.min_values,
                    options: select.options.as_deref(),
                    placeholder: select.placeholder.as_deref(),
                    required: select.required,
                }),
            };
        }
        Self::Standard(component)
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum CustomField<'a> {
    Select(ModalSelect<'a>),
    Radio(ModalRadio<'a>),
}

#[derive(Serialize)]
struct ModalRadio<'a> {
    #[serde(rename = "type")]
    kind: u8,
    custom_id: &'a str,
    options: &'a [RadioGroupOption],
    #[serde(skip_serializing_if = "Option::is_none")]
    required: Option<bool>,
}

#[derive(Serialize)]
struct ModalSelect<'a> {
    #[serde(rename = "type")]
    kind: ComponentType,
    custom_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    channel_types: Option<&'a [ChannelType]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default_values: Option<&'a [SelectDefaultValue]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_values: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_values: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<&'a [SelectMenuOption]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    placeholder: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    required: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{
        ChannelSelect, MentionableSelect, RadioGroup, RadioGroupOption, RoleSelect, SelectOption,
        StringSelect, TextInput, UserSelect,
    };
    use twilight_http::request::Method;

    #[test]
    fn serializes_radio_group_modal_as_a_mixed_token_only_callback() {
        let modal = Modal::new("survey", "アンケート")
            .text_input(TextInput::new("notes", "追記"))
            .radio_group(
                RadioGroup::new(
                    "color",
                    "色",
                    vec![
                        RadioGroupOption::new("青", "blue").selected_by_default(true),
                        RadioGroupOption::new("緑", "green").description("明るい緑"),
                    ],
                )
                .description("一色を選ぶ")
                .required(false),
            )
            .select(
                "場所",
                StringSelect::new("where", vec![SelectOption::new("屋内", "inside")]),
                true,
            );
        let request = custom_modal_request(&modal, Id::new(1), "token")
            .unwrap()
            .unwrap();
        assert_eq!(request.method(), Method::Post);
        assert_eq!(request.path(), "interactions/1/token/callback");
        assert!(!request.use_authorization_token());
        let body: serde_json::Value = serde_json::from_slice(request.body().unwrap()).unwrap();
        assert_eq!(body.pointer("/type"), Some(&serde_json::json!(9)));
        assert_eq!(
            body.pointer("/data/components/0/component/type"),
            Some(&serde_json::json!(4))
        );
        assert_eq!(
            body.pointer("/data/components/1"),
            Some(&serde_json::json!({
                "type": 18, "label": "色", "description": "一色を選ぶ",
                "component": {"type": 21, "custom_id": "color", "required": false, "options": [
                    {"label": "青", "value": "blue", "default": true},
                    {"label": "緑", "value": "green", "description": "明るい緑"}
                ]}
            }))
        );
        let select = body.pointer("/data/components/2/component").unwrap();
        assert_eq!(select.get("type"), Some(&serde_json::json!(3)));
        assert!(select.get("disabled").is_none());
    }

    #[test]
    fn rejects_invalid_radio_group_defaults_duplicate_values_and_field_ids() {
        let modal = || Modal::new("survey", "アンケート");
        let one = || RadioGroupOption::new("青", "blue");
        let two = || RadioGroupOption::new("緑", "green");
        let group = |options| RadioGroup::new("color", "色", options);
        for invalid in [
            group(vec![one()]),
            group(vec![one(), one()]),
            group(vec![
                one().selected_by_default(true),
                two().selected_by_default(true),
            ]),
            group(vec![RadioGroupOption::new("青", ""), two()]),
            group(vec![one(), two().description("あ".repeat(101))]),
        ] {
            assert!(
                custom_modal_request(&modal().radio_group(invalid), Id::new(1), "token").is_err()
            );
        }
        let duplicate = modal()
            .radio_group(group(vec![one(), two()]))
            .text_input(TextInput::new("color", "追記"));
        assert!(custom_modal_request(&duplicate, Id::new(1), "token").is_err());
    }

    #[test]
    fn serializes_modal_select_request_without_the_message_only_field() {
        let modal = Modal::new("survey", "アンケート")
            .select(
                "色",
                StringSelect::new("color", vec![SelectOption::new("青", "blue")]).min_values(0),
                false,
            )
            .user_select(
                "ユーザー",
                UserSelect::new("user").default_values([Id::new(42)]),
                true,
            )
            .role_select("ロール", RoleSelect::new("role"), true)
            .channel_select(
                "チャンネル",
                ChannelSelect::new("channel").channel_types(vec![ChannelType::GuildText]),
                true,
            )
            .mentionable_select("ユーザーかロール", MentionableSelect::new("mention"), true);
        let request = custom_modal_request(&modal, Id::new(1), "token")
            .unwrap()
            .unwrap();
        assert_eq!(request.method(), Method::Post);
        assert_eq!(request.path(), "interactions/1/token/callback");
        assert!(!request.use_authorization_token());
        let wire: serde_json::Value = serde_json::from_slice(request.body().unwrap()).unwrap();
        assert_eq!(wire.pointer("/type"), Some(&serde_json::json!(9)));
        assert_eq!(
            wire.pointer("/data/components/0"),
            Some(&serde_json::json!({
                "type": 18, "label": "色", "component": {
                    "type": 3, "custom_id": "color", "min_values": 0, "required": false,
                    "options": [{"label": "青", "value": "blue", "default": false}]
                }
            }))
        );
        assert_eq!(
            wire.pointer("/data/components/1/component/default_values"),
            Some(&serde_json::json!([{"type": "user", "id": "42"}]))
        );
        assert_eq!(
            wire.pointer("/data/components/3/component/channel_types"),
            Some(&serde_json::json!([0]))
        );
        for (index, kind) in [(1, 5), (2, 6), (3, 8), (4, 7)] {
            let path = format!("/data/components/{index}/component");
            let component = wire.pointer(&path).unwrap();
            assert_eq!(component.get("type"), Some(&serde_json::json!(kind)));
            assert_eq!(component.get("required"), Some(&serde_json::json!(true)));
            assert!(component.get("disabled").is_none());
        }
        let plain = Modal::new("plain", "感想").text_input(TextInput::new("text", "感想"));
        assert!(
            custom_modal_request(&plain, Id::new(1), "token")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn validates_modal_only_select_rules_and_shared_field_ids() {
        let menu = || StringSelect::new("color", vec![SelectOption::new("青", "blue")]);
        let modal = || Modal::new("survey", "アンケート");
        assert!(
            modal()
                .select("色", menu().min_values(0), true)
                .response()
                .is_err()
        );
        assert!(
            modal()
                .select("色", menu().min_values(0), false)
                .response()
                .is_ok()
        );
        assert!(
            modal()
                .select("色", menu().disabled(true), false)
                .response()
                .is_err()
        );
        assert!(
            modal()
                .text_input(TextInput::new("color", "感想"))
                .select("色", menu(), false)
                .response()
                .is_err()
        );
    }
}
