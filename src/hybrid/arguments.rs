use std::{borrow::Cow, ops::RangeInclusive};

use twilight_model::{
    application::{
        command::{
            CommandOption, CommandOptionChoiceValue, CommandOptionType, CommandOptionValue as Bound,
        },
        interaction::{
            InteractionDataResolved,
            application_command::{CommandDataOption, CommandOptionValue},
        },
    },
    channel::Attachment,
    id::{
        Id,
        marker::{ChannelMarker, GenericMarker, RoleMarker, UserMarker},
    },
};

use crate::{
    ArgumentError, Arguments, Mentionable, RegistrationError, Result,
    interactions::resolve_mentionable,
};

/// 型付きハイブリッドコマンドが受け取った引数のエラーです。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HybridArgumentError {
    /// 必須の引数が届きませんでした。
    #[error("必須の引数 `{name}` がありません")]
    Missing {
        /// 対象の引数名です。
        name: String,
    },
    /// 入力を引数の型に変換できませんでした。
    #[error("引数 `{name}` を {kind:?} として読み取れません")]
    InvalidValue {
        /// 対象の引数名です。
        name: String,
        /// 定義にある引数の種類です。
        kind: CommandOptionType,
    },
    /// ユーザーとロールのどちらを指すか一意に判定できませんでした。
    #[error("引数 `{name}` のユーザーとロールを判別できません")]
    AmbiguousMentionable {
        /// 対象の引数名です。
        name: String,
    },
    /// テキスト入力のクォートに問題がありました。
    #[error("引数 `{name}` の構文が無効です")]
    Syntax {
        /// 対象の引数名です。
        name: String,
        /// クォートのエラーです。
        #[source]
        source: ArgumentError,
    },
    /// 引数が指定された範囲や選択肢を満たしませんでした。
    #[error("引数 `{name}` が定義された範囲または選択肢に含まれません")]
    OutOfRange {
        /// 対象の引数名です。
        name: String,
    },
    /// 定義に対応しない引数が届きました。
    #[error("定義に対応しない引数があります")]
    UnexpectedArguments,
}

/// 型付きハイブリッドコマンドの、検証済みの引数です。
///
/// テキストとスラッシュの両方で同じアクセサーを使います。
/// 引数が省略されたか、名前や型が指定と合わない場合は `None` です。
#[derive(Debug)]
pub struct HybridArguments<'a> {
    values: Vec<(&'a str, Value<'a>)>,
}

impl<'a> HybridArguments<'a> {
    fn get(&self, name: &str) -> Option<&Value<'a>> {
        self.values
            .iter()
            .find_map(|(key, value)| (*key == name).then_some(value))
    }

    /// 名前に対応する文字列を借ります。
    pub fn string(&self, name: &str) -> Option<&str> {
        match self.get(name)? {
            Value::String(value) => Some(value),
            _ => None,
        }
    }

    /// 名前に対応する整数を返します。
    pub fn integer(&self, name: &str) -> Option<i64> {
        match self.get(name)? {
            Value::Integer(value) => Some(*value),
            _ => None,
        }
    }

    /// 名前に対応する小数を含む数値を返します。
    pub fn number(&self, name: &str) -> Option<f64> {
        match self.get(name)? {
            Value::Number(value) => Some(*value),
            _ => None,
        }
    }

    /// 名前に対応する真偽値を返します。
    pub fn boolean(&self, name: &str) -> Option<bool> {
        match self.get(name)? {
            Value::Boolean(value) => Some(*value),
            _ => None,
        }
    }

    /// 名前に対応するユーザー ID を返します。
    pub fn user_id(&self, name: &str) -> Option<Id<UserMarker>> {
        match self.get(name)? {
            Value::User(value) => Some(*value),
            _ => None,
        }
    }

    /// 名前に対応するロール ID を返します。
    pub fn role_id(&self, name: &str) -> Option<Id<RoleMarker>> {
        match self.get(name)? {
            Value::Role(value) => Some(*value),
            _ => None,
        }
    }

    /// 名前に対応するチャンネル ID を返します。
    pub fn channel_id(&self, name: &str) -> Option<Id<ChannelMarker>> {
        match self.get(name)? {
            Value::Channel(value) => Some(*value),
            _ => None,
        }
    }

    /// ユーザーまたはロールの ID を、種類を区別して返します。
    pub fn mentionable(&self, name: &str) -> Option<Mentionable> {
        match self.get(name)? {
            Value::Mentionable(value) => Some(*value),
            _ => None,
        }
    }

    /// 添付ファイルの情報を借ります。ファイル本体はダウンロードしません。
    pub fn attachment(&self, name: &str) -> Option<&'a Attachment> {
        match self.get(name)? {
            Value::Attachment(value) => Some(*value),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq)]
enum Value<'a> {
    String(Cow<'a, str>),
    Integer(i64),
    Number(f64),
    Boolean(bool),
    User(Id<UserMarker>),
    Role(Id<RoleMarker>),
    Channel(Id<ChannelMarker>),
    Mentionable(Mentionable),
    Attachment(&'a Attachment),
}

impl<'a> Value<'a> {
    fn from_text(kind: CommandOptionType, token: Cow<'a, str>) -> Option<Self> {
        Some(match kind {
            CommandOptionType::String => Self::String(token),
            CommandOptionType::Integer => Self::Integer(token.parse().ok()?),
            CommandOptionType::Number => Self::Number(token.parse().ok()?),
            CommandOptionType::Boolean => Self::Boolean(parse_boolean(&token)?),
            CommandOptionType::User => Self::User(parse_id(&token, &["<@!", "<@"])?),
            CommandOptionType::Role => Self::Role(parse_id(&token, &["<@&"])?),
            CommandOptionType::Channel => Self::Channel(parse_id(&token, &["<#"])?),
            CommandOptionType::Mentionable => Self::Mentionable(parse_mentionable(&token)?),
            _ => return None,
        })
    }

    fn from_slash(kind: CommandOptionType, input: &'a CommandOptionValue) -> Option<Self> {
        Some(match (kind, input) {
            (CommandOptionType::String, CommandOptionValue::String(value)) => {
                Self::String(Cow::Borrowed(value))
            }
            (CommandOptionType::Integer, CommandOptionValue::Integer(value)) => {
                Self::Integer(*value)
            }
            (CommandOptionType::Number, CommandOptionValue::Number(value)) => Self::Number(*value),
            (CommandOptionType::Boolean, CommandOptionValue::Boolean(value)) => {
                Self::Boolean(*value)
            }
            (CommandOptionType::User, CommandOptionValue::User(value)) => Self::User(*value),
            (CommandOptionType::Role, CommandOptionValue::Role(value)) => Self::Role(*value),
            (CommandOptionType::Channel, CommandOptionValue::Channel(value)) => {
                Self::Channel(*value)
            }
            _ => return None,
        })
    }

    fn from_choice(kind: CommandOptionType, choice: &'a CommandOptionChoiceValue) -> Option<Self> {
        Some(match (kind, choice) {
            (CommandOptionType::String, CommandOptionChoiceValue::String(value)) => {
                Self::String(Cow::Borrowed(value))
            }
            (CommandOptionType::Integer, CommandOptionChoiceValue::Integer(value)) => {
                Self::Integer(*value)
            }
            (CommandOptionType::Number, CommandOptionChoiceValue::Number(value)) => {
                Self::Number(*value)
            }
            _ => return None,
        })
    }
}

fn parse_boolean(token: &str) -> Option<bool> {
    if ["yes", "y", "true", "t", "1", "enable", "on"]
        .iter()
        .any(|value| token.eq_ignore_ascii_case(value))
    {
        Some(true)
    } else if ["no", "n", "false", "f", "0", "disable", "off"]
        .iter()
        .any(|value| token.eq_ignore_ascii_case(value))
    {
        Some(false)
    } else {
        None
    }
}

fn parse_id<M>(token: &str, mention_prefixes: &[&str]) -> Option<Id<M>> {
    let digits = mention_prefixes
        .iter()
        .find_map(|prefix| token.strip_prefix(*prefix)?.strip_suffix('>'))
        .unwrap_or(token);
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Id::new_checked(digits.parse().ok()?)
}

fn parse_mentionable(token: &str) -> Option<Mentionable> {
    if token.starts_with("<@&") {
        parse_id(token, &["<@&"]).map(Mentionable::Role)
    } else if token.starts_with("<@") {
        parse_id(token, &["<@!", "<@"]).map(Mentionable::User)
    } else {
        None
    }
}

pub(super) struct HybridSchema {
    options: Vec<CommandOption>,
    bounds: Vec<Bounds>,
}

impl HybridSchema {
    pub(super) fn new(options: Vec<CommandOption>) -> Result<Self> {
        crate::slash::validate_options(&options)?;
        let bounds = options
            .iter()
            .map(|option| {
                Bounds::for_option(option).ok_or_else(|| RegistrationError::InvalidHybridOption {
                    name: option.name.clone(),
                })
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Self { options, bounds })
    }

    pub(super) fn options(&self) -> &[CommandOption] {
        &self.options
    }

    pub(super) fn bind_text<'a>(
        &'a self,
        input: &'a str,
        attachments: &'a [Attachment],
    ) -> Result<HybridArguments<'a>> {
        let mut arguments = Arguments::new(input);
        let mut files = attachments.iter();
        let values = self.bind(|option| {
            if option.kind == CommandOptionType::Attachment {
                return Ok(files.next().map(Value::Attachment));
            }
            let Some(token) = arguments.next() else {
                return Ok(None);
            };
            let token = token.map_err(|source| HybridArgumentError::Syntax {
                name: option.name.clone(),
                source,
            })?;
            if option.kind == CommandOptionType::Mentionable
                && parse_id::<GenericMarker>(&token, &[]).is_some()
            {
                return Err(ambiguous_mentionable(option));
            }
            Value::from_text(option.kind, token)
                .map(Some)
                .ok_or_else(|| invalid_value(option))
        })?;
        if !arguments.remaining().is_empty() {
            return Err(HybridArgumentError::UnexpectedArguments.into());
        }
        Ok(values)
    }

    pub(super) fn bind_slash<'a>(
        &'a self,
        input: &'a [CommandDataOption],
        resolved: Option<&'a InteractionDataResolved>,
    ) -> Result<HybridArguments<'a>> {
        let values = self.bind(|option| {
            let Some(value) = input.iter().find(|value| value.name == option.name) else {
                return Ok(None);
            };
            slash_value(option, &value.value, resolved).map(Some)
        })?;
        if values.values.len() != input.len() {
            return Err(HybridArgumentError::UnexpectedArguments.into());
        }
        Ok(values)
    }

    fn bind<'a>(
        &'a self,
        mut next: impl FnMut(
            &'a CommandOption,
        ) -> std::result::Result<Option<Value<'a>>, HybridArgumentError>,
    ) -> Result<HybridArguments<'a>> {
        let mut values = Vec::with_capacity(self.options.len());
        for (option, bounds) in self.options.iter().zip(&self.bounds) {
            if let Some(value) = next(option)? {
                let allowed = option.choices.as_ref().is_none_or(|choices| {
                    choices.is_empty()
                        || choices.iter().any(|choice| {
                            Value::from_choice(option.kind, &choice.value).as_ref() == Some(&value)
                        })
                });
                if !allowed || !bounds.contains(&value) {
                    return Err(HybridArgumentError::OutOfRange {
                        name: option.name.clone(),
                    }
                    .into());
                }
                values.push((option.name.as_str(), value));
            } else if option.required.unwrap_or_default() {
                return Err(HybridArgumentError::Missing {
                    name: option.name.clone(),
                }
                .into());
            }
        }
        Ok(HybridArguments { values })
    }
}

fn slash_value<'a>(
    option: &CommandOption,
    value: &'a CommandOptionValue,
    resolved: Option<&'a InteractionDataResolved>,
) -> std::result::Result<Value<'a>, HybridArgumentError> {
    match (option.kind, value) {
        (CommandOptionType::Attachment, CommandOptionValue::Attachment(id)) => resolved
            .and_then(|data| data.attachments.get(id))
            .filter(|attachment| attachment.id == *id)
            .map(Value::Attachment)
            .ok_or_else(|| invalid_value(option)),
        (CommandOptionType::Mentionable, CommandOptionValue::Mentionable(id)) => {
            resolve_mentionable(*id, resolved)
                .map(Value::Mentionable)
                .map_err(|_| ambiguous_mentionable(option))
        }
        _ => Value::from_slash(option.kind, value).ok_or_else(|| invalid_value(option)),
    }
}

fn ambiguous_mentionable(option: &CommandOption) -> HybridArgumentError {
    HybridArgumentError::AmbiguousMentionable {
        name: option.name.clone(),
    }
}

fn invalid_value(option: &CommandOption) -> HybridArgumentError {
    HybridArgumentError::InvalidValue {
        name: option.name.clone(),
        kind: option.kind,
    }
}

const MAX_INTEGER: i64 = (1 << 53) - 1;
const MAX_NUMBER: f64 = 9_007_199_254_740_992.0;
const MAX_STRING_LENGTH: u16 = 6000;

enum Bounds {
    None,
    Length(RangeInclusive<usize>),
    Integer(RangeInclusive<i64>),
    Number(RangeInclusive<f64>),
}

impl Bounds {
    fn for_option(option: &CommandOption) -> Option<Self> {
        if option.options.is_some() || option.file_types.is_some() || option.channel_types.is_some()
        {
            return None;
        }
        if option.kind != CommandOptionType::String
            && (option.min_length.is_some() || option.max_length.is_some())
        {
            return None;
        }
        if !matches!(
            option.kind,
            CommandOptionType::Integer | CommandOptionType::Number
        ) && (option.min_value.is_some() || option.max_value.is_some())
        {
            return None;
        }

        let bounds = match option.kind {
            CommandOptionType::String => {
                let min = option.min_length.unwrap_or(0);
                let max = option.max_length.unwrap_or(MAX_STRING_LENGTH);
                if min > max || !(1..=MAX_STRING_LENGTH).contains(&max) {
                    return None;
                }
                Self::Length(usize::from(min)..=usize::from(max))
            }
            CommandOptionType::Integer => {
                let integer = |bound, default| match bound {
                    None => Some(default),
                    Some(Bound::Integer(value))
                        if (-MAX_INTEGER..=MAX_INTEGER).contains(&value) =>
                    {
                        Some(value)
                    }
                    _ => None,
                };
                let min = integer(option.min_value, -MAX_INTEGER)?;
                let max = integer(option.max_value, MAX_INTEGER)?;
                if min > max {
                    return None;
                }
                Self::Integer(min..=max)
            }
            CommandOptionType::Number => {
                let number = |bound, default| match bound {
                    None => Some(default),
                    Some(Bound::Number(value)) if (-MAX_NUMBER..=MAX_NUMBER).contains(&value) => {
                        Some(value)
                    }
                    _ => None,
                };
                let min = number(option.min_value, -MAX_NUMBER)?;
                let max = number(option.max_value, MAX_NUMBER)?;
                if min > max {
                    return None;
                }
                Self::Number(min..=max)
            }
            CommandOptionType::Boolean
            | CommandOptionType::User
            | CommandOptionType::Role
            | CommandOptionType::Channel
            | CommandOptionType::Mentionable
            | CommandOptionType::Attachment => Self::None,
            _ => return None,
        };

        if let Some(choices) = &option.choices
            && (matches!(bounds, Self::None)
                || option.autocomplete == Some(true)
                || choices.len() > 25
                || !choices.iter().all(|choice| {
                    Value::from_choice(option.kind, &choice.value)
                        .is_some_and(|value| bounds.contains(&value))
                }))
        {
            return None;
        }
        if matches!(bounds, Self::None) && option.autocomplete == Some(true) {
            return None;
        }
        Some(bounds)
    }

    fn contains(&self, value: &Value<'_>) -> bool {
        match (self, value) {
            (Self::None, _) => true,
            (Self::Length(bounds), Value::String(value)) => bounds.contains(&value.chars().count()),
            (Self::Integer(bounds), Value::Integer(value)) => bounds.contains(value),
            (Self::Number(bounds), Value::Number(value)) => bounds.contains(value),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Error, SlashCommands};
    use serde_json::json;
    use twilight_model::channel::ChannelType;

    fn option(name: &str, value: CommandOptionValue) -> CommandDataOption {
        CommandDataOption {
            name: name.into(),
            value,
        }
    }

    fn attachment(id: u64) -> Result<Attachment> {
        serde_json::from_value(json!({
            "id": id.to_string(), "filename": format!("{id}.txt"), "size": 3,
            "url": "https://example.test/file", "proxy_url": "https://example.test/file"
        }))
        .map_err(Error::handler)
    }

    #[test]
    fn attachments_follow_upload_order_without_consuming_tokens_and_resolve_by_slash_id()
    -> Result<()> {
        let schema = HybridSchema::new(vec![
            SlashCommands::attachment_option("first", "添付", true),
            SlashCommands::integer_option("count", "件数", true),
            SlashCommands::attachment_option("second", "追加添付", false),
            SlashCommands::string_option("note", "備考", false),
        ])?;
        let files = [attachment(7)?, attachment(8)?, attachment(9)?];
        let text = schema.bind_text(r#"3 "hello world""#, &files)?;
        assert!(std::ptr::eq(text.attachment("first").unwrap(), &files[0]));
        assert!(std::ptr::eq(text.attachment("second").unwrap(), &files[1]));
        assert_eq!(text.integer("count"), Some(3));
        assert_eq!(text.string("note"), Some("hello world"));
        assert!(
            schema
                .bind_text("3", &files[..1])?
                .attachment("second")
                .is_none()
        );
        assert!(
            matches!(schema.bind_text("3", &[]), Err(Error::HybridArgument(HybridArgumentError::Missing { name })) if name == "first")
        );
        assert!(matches!(
            schema.bind_text("3 note trailing", &files),
            Err(Error::HybridArgument(
                HybridArgumentError::UnexpectedArguments
            ))
        ));

        let mut resolved: InteractionDataResolved =
            serde_json::from_value(json!({})).map_err(Error::handler)?;
        resolved.attachments = files.iter().cloned().map(|file| (file.id, file)).collect();
        let input = [
            option("second", CommandOptionValue::Attachment(Id::new(7))),
            option("count", CommandOptionValue::Integer(3)),
            option("first", CommandOptionValue::Attachment(Id::new(8))),
        ];
        let slash = schema.bind_slash(&input, Some(&resolved))?;
        assert_eq!(
            slash.attachment("first").map(|file| file.id),
            Some(Id::new(8))
        );
        assert_eq!(
            slash.attachment("second").map(|file| file.id),
            Some(Id::new(7))
        );
        assert_eq!(slash.integer("count"), Some(3));
        assert_eq!(slash.string("note"), None);
        assert!(
            schema
                .bind_slash(&input[1..], Some(&resolved))?
                .attachment("second")
                .is_none()
        );
        assert!(
            matches!(schema.bind_slash(&input, None), Err(Error::HybridArgument(HybridArgumentError::InvalidValue { name, kind: CommandOptionType::Attachment })) if name == "first")
        );
        resolved.attachments.insert(Id::new(8), attachment(99)?);
        assert!(schema.bind_slash(&input, Some(&resolved)).is_err());
        Ok(())
    }

    #[test]
    fn mentionable_requires_an_explicit_text_kind_or_one_matching_resolved_entity() -> Result<()> {
        let schema = HybridSchema::new(vec![SlashCommands::mentionable_option(
            "target", "対象", false,
        )])?;
        assert_eq!(schema.bind_text("", &[])?.mentionable("target"), None);
        assert_eq!(
            schema.bind_text("<@!8>", &[])?.mentionable("target"),
            Some(Mentionable::User(Id::new(8)))
        );
        assert_eq!(
            schema.bind_text("<@&8>", &[])?.mentionable("target"),
            Some(Mentionable::Role(Id::new(8)))
        );
        assert!(
            matches!(schema.bind_text("8", &[]), Err(Error::HybridArgument(HybridArgumentError::AmbiguousMentionable { name })) if name == "target")
        );
        assert!(matches!(
            schema.bind_text("<#8>", &[]),
            Err(Error::HybridArgument(HybridArgumentError::InvalidValue {
                kind: CommandOptionType::Mentionable,
                ..
            }))
        ));

        let mut resolved: InteractionDataResolved = serde_json::from_value(json!({
            "users": {"8": {"id":"8", "username":"member", "discriminator":"0"}},
            "roles": {"9": {"id":"9", "name":"role", "color":0,
                "colors":{"primary_color":0}, "flags":0, "hoist":false, "managed":false,
                "mentionable":true, "permissions":"0", "position":0}}
        }))
        .map_err(Error::handler)?;
        let user = [option(
            "target",
            CommandOptionValue::Mentionable(Id::new(8)),
        )];
        let role = [option(
            "target",
            CommandOptionValue::Mentionable(Id::new(9)),
        )];
        assert_eq!(
            schema
                .bind_slash(&user, Some(&resolved))?
                .mentionable("target"),
            Some(Mentionable::User(Id::new(8)))
        );
        assert_eq!(
            schema
                .bind_slash(&role, Some(&resolved))?
                .mentionable("target"),
            Some(Mentionable::Role(Id::new(9)))
        );
        assert_eq!(schema.bind_slash(&[], None)?.mentionable("target"), None);
        assert!(matches!(
            schema.bind_slash(&user, None),
            Err(Error::HybridArgument(
                HybridArgumentError::AmbiguousMentionable { .. }
            ))
        ));
        let mut duplicate = resolved.roles[&Id::new(9)].clone();
        duplicate.id = Id::new(8);
        resolved.roles.insert(duplicate.id, duplicate);
        assert!(matches!(
            schema.bind_slash(&user, Some(&resolved)),
            Err(Error::HybridArgument(
                HybridArgumentError::AmbiguousMentionable { .. }
            ))
        ));
        Ok(())
    }

    #[test]
    fn both_sources_bind_the_same_named_scalars_and_ids() -> Result<()> {
        let schema = HybridSchema::new(vec![
            SlashCommands::integer_option("count", "個数", true),
            SlashCommands::number_option("ratio", "倍率", true),
            SlashCommands::boolean_option("loud", "有効", true),
            SlashCommands::user_option("user", "利用者", true),
            SlashCommands::role_option("role", "ロール", true),
            SlashCommands::channel_option("channel", "チャンネル", true),
            SlashCommands::string_option("label", "ラベル", true),
            SlashCommands::string_option("note", "備考", false),
        ])?;
        let text = schema.bind_text(r#"3 0.5 YeS <@!11> <@&22> <#33> 「こん にちは」"#, &[])?;
        let payload = [
            option("label", CommandOptionValue::String("こん にちは".into())),
            option("channel", CommandOptionValue::Channel(Id::new(33))),
            option("role", CommandOptionValue::Role(Id::new(22))),
            option("user", CommandOptionValue::User(Id::new(11))),
            option("loud", CommandOptionValue::Boolean(true)),
            option("ratio", CommandOptionValue::Number(0.5)),
            option("count", CommandOptionValue::Integer(3)),
        ];
        let slash = schema.bind_slash(&payload, None)?;
        for args in [text, slash] {
            assert_eq!(args.integer("count"), Some(3));
            assert_eq!(args.number("ratio"), Some(0.5));
            assert_eq!(args.boolean("loud"), Some(true));
            assert_eq!(args.user_id("user"), Some(Id::new(11)));
            assert_eq!(args.role_id("role"), Some(Id::new(22)));
            assert_eq!(args.channel_id("channel"), Some(Id::new(33)));
            assert_eq!(args.string("label"), Some("こん にちは"));
            assert_eq!(args.string("note"), None);
            assert_eq!(args.user_id("role"), None);
        }
        let text = schema.bind_text(r#"3 0.5 off 11 22 33 "" "escaped \"quote\"""#, &[])?;
        assert_eq!(text.string("label"), Some(""));
        assert_eq!(text.string("note"), Some(r#"escaped "quote""#));
        assert_eq!(text.boolean("loud"), Some(false));
        Ok(())
    }

    #[test]
    fn rejects_missing_invalid_and_unexpected_values() -> Result<()> {
        let schema = HybridSchema::new(vec![SlashCommands::user_option("user", "利用者", true)])?;
        assert!(matches!(
            schema.bind_text("", &[]),
            Err(Error::HybridArgument(HybridArgumentError::Missing { .. }))
        ));
        assert!(matches!(
            schema.bind_slash(&[], None),
            Err(Error::HybridArgument(HybridArgumentError::Missing { .. }))
        ));
        for input in ["0", "<@&3>", "<#3>", "not-a-user", "18446744073709551616"] {
            assert!(matches!(
                schema.bind_text(input, &[]),
                Err(Error::HybridArgument(
                    HybridArgumentError::InvalidValue { .. }
                ))
            ));
        }
        assert!(matches!(
            schema.bind_text("\"13", &[]),
            Err(Error::HybridArgument(HybridArgumentError::Syntax {
                source: ArgumentError::UnclosedQuote,
                ..
            }))
        ));
        assert!(matches!(
            schema.bind_slash(
                &[option("user", CommandOptionValue::Role(Id::new(3)))],
                None
            ),
            Err(Error::HybridArgument(
                HybridArgumentError::InvalidValue { .. }
            ))
        ));
        assert!(matches!(
            schema.bind_text("1 2", &[]),
            Err(Error::HybridArgument(
                HybridArgumentError::UnexpectedArguments
            ))
        ));
        for field in ["user", "unexpected"] {
            let payload = [
                option("user", CommandOptionValue::User(Id::new(3))),
                option(field, CommandOptionValue::User(Id::new(4))),
            ];
            assert!(matches!(
                schema.bind_slash(&payload, None),
                Err(Error::HybridArgument(
                    HybridArgumentError::UnexpectedArguments
                ))
            ));
        }
        Ok(())
    }

    #[test]
    fn validates_range_length_and_choices_for_both_sources() -> Result<()> {
        let mut count = SlashCommands::integer_option("count", "個数", true);
        count.min_value = Some(Bound::Integer(1));
        count.max_value = Some(Bound::Integer(5));
        count.choices = Some(vec![SlashCommands::integer_choice("三", 3)]);
        let mut label = SlashCommands::string_option("label", "ラベル", true);
        label.min_length = Some(1);
        label.max_length = Some(2);
        let schema = HybridSchema::new(vec![count, label])?;
        assert!(schema.bind_text("3 日本", &[]).is_ok());
        for (input, expected) in [
            ("0 日", "count"),
            ("4 日", "count"),
            ("3 日本語", "label"),
            ("3 \"\"", "label"),
        ] {
            assert!(
                matches!(schema.bind_text(input, &[]), Err(Error::HybridArgument(HybridArgumentError::OutOfRange {name})) if name == expected)
            );
        }
        for (count, label, expected) in [(4, "日", "count"), (3, "日本語", "label")] {
            let payload = [
                option("count", CommandOptionValue::Integer(count)),
                option("label", CommandOptionValue::String(label.into())),
            ];
            assert!(
                matches!(schema.bind_slash(&payload, None), Err(Error::HybridArgument(HybridArgumentError::OutOfRange {name})) if name == expected)
            );
        }
        let number = HybridSchema::new(vec![SlashCommands::number_option("n", "数", true)])?;
        assert!(number.bind_text("9007199254740992", &[]).is_ok());
        for input in ["NaN", "inf", "-inf", "9007199254740994"] {
            assert!(matches!(
                number.bind_text(input, &[]),
                Err(Error::HybridArgument(
                    HybridArgumentError::OutOfRange { .. }
                ))
            ));
        }
        let integer = HybridSchema::new(vec![SlashCommands::integer_option("n", "整数", true)])?;
        assert!(integer.bind_text("9007199254740991", &[]).is_ok());
        assert!(matches!(
            integer.bind_text("9007199254740992", &[]),
            Err(Error::HybridArgument(
                HybridArgumentError::OutOfRange { .. }
            ))
        ));
        Ok(())
    }

    #[test]
    fn rejects_definitions_that_cannot_be_checked_on_both_sources() {
        let mut channel = SlashCommands::channel_option("channel", "部屋", true);
        channel.channel_types = Some(vec![ChannelType::GuildText]);
        let mut integer = SlashCommands::integer_option("n", "整数", true);
        integer.max_value = Some(Bound::Number(1.5));
        let mut label = SlashCommands::string_option("label", "ラベル", true);
        label.max_length = Some(0);
        let mut choice = SlashCommands::number_option("choice", "選ぶ", true);
        choice.choices = Some(vec![SlashCommands::integer_choice("整数", 3)]);
        for option in [channel, integer, label, choice] {
            assert!(matches!(
                HybridSchema::new(vec![option]),
                Err(Error::Registration(
                    RegistrationError::InvalidHybridOption { .. }
                ))
            ));
        }
        assert!(
            HybridSchema::new(vec![
                SlashCommands::string_option("optional", "省略可", false),
                SlashCommands::string_option("required", "必須", true),
            ])
            .is_err()
        );
        assert!(
            HybridSchema::new(vec![
                SlashCommands::string_option("duplicate", "重複", true),
                SlashCommands::string_option("duplicate", "重複", false),
            ])
            .is_err()
        );
    }
}
