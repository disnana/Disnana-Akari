use twilight_model::id::{
    Id,
    marker::{ChannelMarker, GenericMarker, RoleMarker, UserMarker},
};

use crate::Mentionable;

/// Discord Bot のテキスト入力を、指定した型に変換します。
///
/// `bool`、`Id<UserMarker>`、`Id<RoleMarker>`、`Id<ChannelMarker>`、`Id<GenericMarker>`、
/// [`Mentionable`] に標準の変換を用意しています。標準の変換は文字列の形式だけを調べ、通信しません。
pub trait DiscordArgument: Sized {
    /// 1 個の値に変換します。形式や種類が合わなければ `None` を返します。
    fn parse_discord_argument(value: &str) -> Option<Self>;
}

/// Discord 用の規則で、入力を指定した型に変換できなかったことを表すエラーです。
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("Discord の引数として読み取れません")]
pub struct InvalidDiscordArgument;

impl DiscordArgument for bool {
    fn parse_discord_argument(value: &str) -> Option<Self> {
        if ["yes", "y", "true", "t", "1", "enable", "on"]
            .iter()
            .any(|token| value.eq_ignore_ascii_case(token))
        {
            Some(true)
        } else if ["no", "n", "false", "f", "0", "disable", "off"]
            .iter()
            .any(|token| value.eq_ignore_ascii_case(token))
        {
            Some(false)
        } else {
            None
        }
    }
}

impl DiscordArgument for Id<UserMarker> {
    fn parse_discord_argument(value: &str) -> Option<Self> {
        parse_id(value, &["<@!", "<@"])
    }
}

impl DiscordArgument for Id<RoleMarker> {
    fn parse_discord_argument(value: &str) -> Option<Self> {
        parse_id(value, &["<@&"])
    }
}

impl DiscordArgument for Id<ChannelMarker> {
    fn parse_discord_argument(value: &str) -> Option<Self> {
        parse_id(value, &["<#"])
    }
}

impl DiscordArgument for Id<GenericMarker> {
    fn parse_discord_argument(value: &str) -> Option<Self> {
        parse_id(value, &[])
    }
}

impl DiscordArgument for Mentionable {
    fn parse_discord_argument(value: &str) -> Option<Self> {
        if value.starts_with("<@&") {
            Id::<RoleMarker>::parse_discord_argument(value).map(Self::Role)
        } else if value.starts_with("<@") {
            Id::<UserMarker>::parse_discord_argument(value).map(Self::User)
        } else {
            None
        }
    }
}

fn parse_id<M>(value: &str, prefixes: &[&str]) -> Option<Id<M>> {
    let digits = prefixes
        .iter()
        .find_map(|prefix| value.strip_prefix(*prefix)?.strip_suffix('>'))
        .unwrap_or(value);
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Id::new_checked(digits.parse().ok()?)
}

#[cfg(test)]
mod tests {
    use super::super::{ArgumentError, ArgumentValueError, Arguments};
    use super::*;

    #[test]
    fn booleans_reuse_discord_spellings_without_changing_from_str() {
        let mut args = Arguments::new("YeS DISAble 1 0 maybe ON \" yes \"");
        assert_eq!(args.parse_discord::<bool>(), Ok(Some(true)));
        assert_eq!(args.parse_discord::<bool>(), Ok(Some(false)));
        assert_eq!(args.parse_discord::<bool>(), Ok(Some(true)));
        assert_eq!(args.parse_discord::<bool>(), Ok(Some(false)));
        assert_eq!(
            args.parse_discord::<bool>(),
            Err(ArgumentValueError::Invalid(InvalidDiscordArgument))
        );
        assert_eq!(args.parse_discord::<bool>(), Ok(Some(true)));
        assert_eq!(
            args.parse_discord::<bool>(),
            Err(ArgumentValueError::Invalid(InvalidDiscordArgument))
        );
        assert_eq!(args.parse_discord::<bool>(), Ok(None));
        assert!(Arguments::new("yes").parse_next::<bool>().is_err());
    }

    #[test]
    fn snowflakes_require_nonzero_ascii_digits_and_the_expected_mention_kind() {
        assert_eq!(
            Id::<UserMarker>::parse_discord_argument("<@23>"),
            Some(Id::new(23))
        );
        assert_eq!(
            Id::<UserMarker>::parse_discord_argument("<@!23>"),
            Some(Id::new(23))
        );
        assert_eq!(
            Id::<RoleMarker>::parse_discord_argument("<@&23>"),
            Some(Id::new(23))
        );
        assert_eq!(
            Id::<ChannelMarker>::parse_discord_argument("<#23>"),
            Some(Id::new(23))
        );
        assert_eq!(
            Id::<ChannelMarker>::parse_discord_argument("0023"),
            Some(Id::new(23))
        );
        assert_eq!(
            Id::<GenericMarker>::parse_discord_argument("23"),
            Some(Id::new(23))
        );
        assert_eq!(Id::<GenericMarker>::parse_discord_argument("<#23>"), None);
        for invalid in [
            "<@&23>",
            "<#23>",
            "<@23>>",
            "<@23",
            "",
            "0",
            "+23",
            "２３",
            "18446744073709551616",
        ] {
            assert_eq!(
                Id::<UserMarker>::parse_discord_argument(invalid),
                None,
                "{invalid}"
            );
        }
    }

    #[test]
    fn mentionable_requires_the_kind_and_syntax_errors_stay_distinct() {
        let mut args = Arguments::new(r#"「<@!23>」 "<@&23>" 23 <#23> 「閉じない"#);
        assert_eq!(
            args.parse_discord::<Mentionable>(),
            Ok(Some(Mentionable::User(Id::new(23))))
        );
        assert_eq!(
            args.parse_discord::<Mentionable>(),
            Ok(Some(Mentionable::Role(Id::new(23))))
        );
        for _ in 0..2 {
            assert_eq!(
                args.parse_discord::<Mentionable>(),
                Err(ArgumentValueError::Invalid(InvalidDiscordArgument))
            );
        }
        assert_eq!(
            args.parse_discord::<Mentionable>(),
            Err(ArgumentValueError::Syntax(ArgumentError::UnclosedQuote))
        );
        assert_eq!(args.parse_discord::<Mentionable>(), Ok(None));
    }
}
