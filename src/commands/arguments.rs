use std::{borrow::Cow, str::FromStr};

/// クォート付きのテキスト引数を読み取れなかった理由です。
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ArgumentError {
    /// 開きクォートに対応する閉じクォートがありません。
    #[error("引数のクォートが閉じられていません")]
    UnclosedQuote,
    /// 閉じクォートの直後に空白がありません。
    #[error("クォートで囲んだ引数の後ろに空白が必要です")]
    MissingSeparator,
}

/// テキスト引数を型付きの値として読み取れなかった理由です。
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ArgumentValueError<E> {
    /// 必須の引数がありません。
    #[error("必須の引数がありません")]
    Missing,
    /// クォートなどの構文が無効です。
    #[error(transparent)]
    Syntax(#[from] ArgumentError),
    /// 入力を対象の型へ変換できませんでした。
    #[error("引数を変換できません: {0}")]
    Invalid(E),
}

/// テキスト引数を必要になった分だけ読み取ります。
///
/// 通常は空白で区切ります。空白を含む一つの値は、二重引用符・一重引用符・
/// スマートクォート・かぎ括弧で囲めます。囲んだ中では、バックスラッシュで
/// 同じ種類のクォートやバックスラッシュをエスケープできます。
/// アンエスケープが必要な場合だけ、新しい文字列を確保します。
#[derive(Clone, Debug)]
pub struct Arguments<'a> {
    remaining: &'a str,
}

impl<'a> Arguments<'a> {
    /// 文字列から最初の引数を読む準備をします。
    pub fn new(text: &'a str) -> Self {
        Self { remaining: text }
    }

    /// まだ読み取っていない部分を取得します。先頭の区切り空白は省きます。
    pub fn remaining(&self) -> &'a str {
        self.remaining.trim_start()
    }

    /// 次の引数を型付きの値として読みます。引数がなければ None を返します。
    ///
    /// # エラー
    /// クォートの構文エラー、または値が対象の型に変換できない場合に失敗します。
    pub fn parse_next<T: FromStr>(&mut self) -> Result<Option<T>, ArgumentValueError<T::Err>> {
        self.convert_next(|value| value.parse())
    }

    /// 次の引数を必須の型付きの値として読みます。
    ///
    /// # エラー
    /// 引数が存在しない場合や、構文・型の変換に失敗した場合にエラーを返します。
    pub fn parse_required<T: FromStr>(&mut self) -> Result<T, ArgumentValueError<T::Err>> {
        self.parse_next()?.ok_or(ArgumentValueError::Missing)
    }

    /// 次の引数に独自の変換を適用します。引数がなければ変換関数は呼びません。
    ///
    /// 文字列はアンエスケープが必要な場合だけ確保し、それ以外は元の入力を借ります。
    ///
    /// # エラー
    /// 構文エラーか変換関数のエラーを返します。変換関数が失敗した場合も、その引数は読み取り済みになります。
    pub fn convert_next<T, E>(
        &mut self,
        convert: impl FnOnce(Cow<'a, str>) -> Result<T, E>,
    ) -> Result<Option<T>, ArgumentValueError<E>> {
        self.next()
            .transpose()?
            .map(|value| convert(value).map_err(ArgumentValueError::Invalid))
            .transpose()
    }
}

impl<'a> Iterator for Arguments<'a> {
    type Item = Result<Cow<'a, str>, ArgumentError>;

    fn next(&mut self) -> Option<Self::Item> {
        let remaining = self.remaining();
        let first = remaining.chars().next()?;
        let Some(closing) = closing_quote(first) else {
            let (value, rest) = remaining
                .split_once(char::is_whitespace)
                .unwrap_or((remaining, ""));
            self.remaining = rest;
            return Some(Ok(Cow::Borrowed(value)));
        };

        let text = remaining.strip_prefix(first)?;
        match quoted(text, first, closing) {
            Ok((value, rest)) => {
                self.remaining = rest;
                Some(Ok(value))
            }
            Err(error) => {
                self.remaining = "";
                Some(Err(error))
            }
        }
    }
}

impl std::iter::FusedIterator for Arguments<'_> {}

fn closing_quote(opening: char) -> Option<char> {
    match opening {
        '"' | '\'' => Some(opening),
        '“' => Some('”'),
        '‘' => Some('’'),
        '「' => Some('」'),
        '『' => Some('』'),
        _ => None,
    }
}

fn quoted(text: &str, opening: char, closing: char) -> Result<(Cow<'_, str>, &str), ArgumentError> {
    let mut chars = text.char_indices().peekable();
    let mut decoded = None::<String>;
    let mut segment_start = 0;
    while let Some((index, ch)) = chars.next() {
        if ch == closing {
            let rest = text.get(index + ch.len_utf8()..).unwrap_or_default();
            if rest.starts_with(|ch: char| !ch.is_whitespace()) {
                return Err(ArgumentError::MissingSeparator);
            }
            let segment = text.get(segment_start..index).unwrap_or_default();
            if let Some(mut decoded) = decoded {
                decoded.push_str(segment);
                return Ok((Cow::Owned(decoded), rest));
            }
            return Ok((Cow::Borrowed(segment), rest));
        }
        if ch == '\\'
            && let Some(&(escaped_index, escaped)) = chars.peek()
            && (escaped == '\\' || escaped == opening || escaped == closing)
        {
            let decoded = decoded.get_or_insert_with(String::new);
            decoded.push_str(text.get(segment_start..index).unwrap_or_default());
            decoded.push(escaped);
            segment_start = escaped_index + escaped.len_utf8();
            chars.next();
        }
    }
    Err(ArgumentError::UnclosedQuote)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_whitespace_and_borrows_plain_and_quoted_values() {
        let mut args = Arguments::new(r#"   25　「二つ の言葉」  ""  'x y' remaining text"#);
        assert_eq!(args.next(), Some(Ok(Cow::Borrowed("25"))));
        assert!(matches!(
            args.next(),
            Some(Ok(Cow::Borrowed("二つ の言葉")))
        ));
        assert_eq!(args.next(), Some(Ok(Cow::Borrowed(""))));
        assert_eq!(args.next(), Some(Ok(Cow::Borrowed("x y"))));
        assert_eq!(args.remaining(), "remaining text");
    }

    #[test]
    fn typed_arguments_distinguish_missing_syntax_and_conversion_errors() {
        let mut args = Arguments::new(r#"25 invalid 「閉じない"#);
        assert_eq!(args.parse_required::<u32>(), Ok(25));
        assert!(matches!(
            args.parse_next::<u32>(),
            Err(ArgumentValueError::Invalid(_))
        ));
        assert_eq!(
            args.parse_next::<u32>(),
            Err(ArgumentValueError::Syntax(ArgumentError::UnclosedQuote))
        );
        assert_eq!(args.parse_next::<u32>(), Ok(None));
        assert_eq!(
            args.parse_required::<u32>(),
            Err(ArgumentValueError::Missing)
        );
    }

    #[test]
    fn custom_conversion_keeps_the_borrow_for_plain_input() {
        let mut args = Arguments::new("value");
        let converted = args.convert_next(Ok::<_, std::convert::Infallible>);
        assert_eq!(converted, Ok(Some(Cow::Borrowed("value"))));
        let mut called = false;
        let absent = args.convert_next(|_| {
            called = true;
            Ok::<(), ()>(())
        });
        assert_eq!(absent, Ok(None));
        assert!(!called);
    }

    #[test]
    fn decodes_known_escapes_only_and_stops_after_a_syntax_error() {
        let mut args = Arguments::new(r#""a \"word\" \\ C:\folder" "missing"space following"#);
        assert!(
            matches!(args.next(), Some(Ok(Cow::Owned(value))) if value == r#"a "word" \ C:\folder"#)
        );
        assert_eq!(args.next(), Some(Err(ArgumentError::MissingSeparator)));
        assert_eq!(args.next(), None);
        assert_eq!(
            Arguments::new("「閉じない").next(),
            Some(Err(ArgumentError::UnclosedQuote))
        );
    }
}
