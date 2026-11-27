use crate::{Error, Result};

const MAX_ID_LENGTH: usize = 100;
const MAX_NAMESPACE_LENGTH: usize = MAX_ID_LENGTH - 2;

/// 再起動後も同じハンドラーで処理できる、名前空間と値を持つ操作 ID です。
///
/// ボタンやセレクト、モーダルのコンストラクターに直接渡せます。
/// ID は Discord に送信されるため、秘密情報を含めないでください。
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[must_use]
pub struct DynamicId {
    id: String,
    namespace_len: usize,
}

impl DynamicId {
    /// `名前空間:値` の形式で操作 ID を組み立てます。
    ///
    /// 名前空間は ASCII 英数字、`_`、`-` の 1〜98 文字です。
    /// 値は空でなく、制御文字を含められません。値に `:` を使えます。
    ///
    /// # エラー
    ///
    /// 上記の条件を満たさないか、全体が 100 文字を超えると失敗します。
    pub fn new(namespace: &str, value: &str) -> Result<Self> {
        if !valid_namespace(namespace) {
            return Err(invalid_namespace());
        }
        if !valid_value(namespace, value) {
            return Err(Error::InvalidUi(
                "動的 ID の値は空でなく、制御文字を含まず、名前空間と合わせて 100 文字以内にしてください。",
            ));
        }
        Ok(Self {
            id: format!("{namespace}:{value}"),
            namespace_len: namespace.len(),
        })
    }

    /// Discord へ送る完全な操作 ID を返します。
    pub fn as_str(&self) -> &str {
        &self.id
    }

    /// 名前空間と値を借ります。
    pub fn parts(&self) -> DynamicIdRef<'_> {
        let (namespace, suffix) = self.id.split_at(self.namespace_len);
        let (_, value) = suffix.split_at(1);
        DynamicIdRef { namespace, value }
    }
}

impl From<DynamicId> for String {
    fn from(id: DynamicId) -> Self {
        id.id
    }
}

/// 受信した動的 ID の名前空間と値を借りたものです。
///
/// 値を使う前に、操作したユーザーの権限や対象の現在の状態を確認してください。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DynamicIdRef<'a> {
    namespace: &'a str,
    value: &'a str,
}

impl<'a> DynamicIdRef<'a> {
    pub(crate) fn parse(id: &'a str) -> Option<Self> {
        // Discord の上限内の Unicode 文字列は最大 4 バイト × 100 文字です。
        if id.len() > 4 * MAX_ID_LENGTH {
            return None;
        }
        let (namespace, value) = id.split_once(':')?;
        (valid_namespace(namespace) && valid_value(namespace, value))
            .then_some(Self { namespace, value })
    }

    /// 登録時に指定した名前空間を返します。
    pub fn namespace(self) -> &'a str {
        self.namespace
    }

    /// 発行時に ID に埋め込んだ値を返します。
    pub fn value(self) -> &'a str {
        self.value
    }
}

pub(crate) fn valid_namespace(namespace: &str) -> bool {
    (1..=MAX_NAMESPACE_LENGTH).contains(&namespace.len())
        && namespace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

pub(crate) fn invalid_namespace() -> Error {
    Error::InvalidUi("動的 ID の名前空間は ASCII 英数字、_、- の 1〜98 文字にしてください。")
}

fn valid_value(namespace: &str, value: &str) -> bool {
    let max_length = MAX_ID_LENGTH - namespace.len() - 1;
    !value.is_empty()
        && value.len() <= 4 * max_length
        && value
            .chars()
            .enumerate()
            .all(|(index, character)| index < max_length && !character.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{Button, Modal, SelectOption, StringSelect, TextInput, UiMessage};

    #[test]
    fn dynamic_id_checks_bounds_and_round_trips_without_truncation() {
        let namespace = "a".repeat(32);
        let value = "🦀".repeat(67);
        let id = DynamicId::new(&namespace, &value).unwrap();
        assert_eq!(id.as_str().chars().count(), 100);
        assert_eq!(id.parts().namespace(), namespace);
        assert_eq!(id.parts().value(), value);
        assert_eq!(DynamicIdRef::parse(id.as_str()), Some(id.parts()));
        assert!(DynamicId::new(&namespace, &(value + "x")).is_err());
        assert!(DynamicId::new(&"a".repeat(98), "x").is_ok());
        assert!(DynamicId::new(&"a".repeat(99), "x").is_err());
        assert!(DynamicId::new("名前", "x").is_err());
        assert!(DynamicId::new("bad:prefix", "x").is_err());
        assert!(DynamicId::new("ok", "").is_err());
        assert!(DynamicId::new("ok", "bad\nvalue").is_err());
        assert!(DynamicIdRef::parse("ok:").is_none());
        assert_eq!(DynamicIdRef::parse("ok:a:b").unwrap().value(), "a:b");
    }

    #[test]
    fn existing_ui_constructors_accept_dynamic_ids() {
        let id = DynamicId::new("queue", "ticket:7").unwrap();
        UiMessage::new()
            .buttons(vec![Button::primary(id.clone(), "Ticket")])
            .validate()
            .unwrap();
        UiMessage::new()
            .select(StringSelect::new(
                id.clone(),
                vec![SelectOption::new("Ticket", "7")],
            ))
            .validate()
            .unwrap();
        Modal::new(id, "Ticket")
            .text_input(TextInput::new("summary", "Summary"))
            .validate()
            .unwrap();
    }
}
