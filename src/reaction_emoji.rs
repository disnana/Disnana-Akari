use twilight_http::request::channel::reaction::RequestReactionType;
use twilight_model::id::Id;

/// Unicode 絵文字またはカスタム絵文字の表記を、Twilight のリアクション指定に変換します。
///
/// カスタム絵文字は `<:name:id>`、`<a:name:id>`、`name:id` を受け付けます。
/// アニメーションかどうかはリアクションのリクエストには不要です。
/// Unicode の絵文字として Discord が使えるかは API が判定します。
///
/// # エラー
///
/// 空の指定、無効なカスタム絵文字の名前や ID、空白や制御文字を含む指定は失敗します。
pub fn parse_reaction_emoji(input: &str) -> Result<RequestReactionType<'_>, ReactionEmojiError> {
    let input = input.trim();
    let emoji = if let Some(mention) = input
        .strip_prefix("<:")
        .or_else(|| input.strip_prefix("<a:"))
    {
        custom(
            mention
                .strip_suffix('>')
                .ok_or(ReactionEmojiError::Invalid)?,
        )?
    } else if input.contains(':') {
        custom(input)?
    } else {
        let emoji = RequestReactionType::Unicode { name: input };
        if input.is_ascii() {
            return Err(ReactionEmojiError::Invalid);
        }
        emoji
    };
    if !valid_reaction_emoji(&emoji) {
        return Err(ReactionEmojiError::Invalid);
    }
    Ok(emoji)
}

fn custom(input: &str) -> Result<RequestReactionType<'_>, ReactionEmojiError> {
    let (name, id) = input.split_once(':').ok_or(ReactionEmojiError::Invalid)?;
    if !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ReactionEmojiError::Invalid);
    }
    let id = id
        .parse::<Id<_>>()
        .map_err(|_| ReactionEmojiError::Invalid)?;
    Ok(RequestReactionType::Custom {
        id,
        name: Some(name),
    })
}

pub(crate) fn valid_reaction_emoji(emoji: &RequestReactionType<'_>) -> bool {
    match emoji {
        RequestReactionType::Custom { name, .. } => name.is_none_or(|name| {
            (1..=32).contains(&name.len())
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        }),
        RequestReactionType::Unicode { name } => {
            !name.is_empty() && !name.chars().any(|ch| ch.is_control() || ch.is_whitespace())
        }
    }
}

/// リアクション用の絵文字を読み取れなかった理由です。
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ReactionEmojiError {
    /// 指定された表記をリアクションに変換できません。
    #[error("リアクションに使う絵文字の指定が無効です")]
    Invalid,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unicode_and_typed_custom_reactions() {
        assert_eq!(
            parse_reaction_emoji(" ❤️ ").unwrap(),
            RequestReactionType::Unicode { name: "❤️" }
        );
        for source in ["<:wave_1:42>", "<a:wave_1:42>", "wave_1:42"] {
            assert_eq!(
                parse_reaction_emoji(source).unwrap(),
                RequestReactionType::Custom {
                    name: Some("wave_1"),
                    id: Id::new(42)
                }
            );
        }
    }

    #[test]
    fn rejects_malformed_names_ids_and_multiple_arguments() {
        for source in [
            "",
            "text",
            "🐈 🐕",
            "<:wave:0>",
            "<:wave:42",
            "<a::42>",
            "two-words:42",
            "wave:+42",
        ] {
            assert_eq!(
                parse_reaction_emoji(source),
                Err(ReactionEmojiError::Invalid)
            );
        }
    }
}
