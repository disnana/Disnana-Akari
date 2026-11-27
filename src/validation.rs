//! Discord に送る前に確認できるメッセージの制限です。

use twilight_model::{channel::message::Embed, http::attachment::Attachment};
use twilight_validate::{embed, message};

use crate::{Error, Result};

/// 通常のメッセージ本文の最大文字数です。
pub const MESSAGE_CONTENT_MAX: usize = twilight_validate::message::MESSAGE_CONTENT_LENGTH_MAX;

/// 一つのメッセージに保持できる添付ファイルの最大数です。
pub const MESSAGE_ATTACHMENTS_MAX: usize = 10;

/// 本文が Discord の文字数制限に収まるか調べます。
///
/// 文字数が上限を超えた時点で検証を終えます。
///
/// # エラー
///
/// 本文が上限を超える場合は [`Error::MessageValidation`] を返します。
pub fn message_content(content: &str) -> Result<()> {
    if content.len() <= MESSAGE_CONTENT_MAX {
        return Ok(());
    }

    let Some((offset, character)) = content.char_indices().nth(MESSAGE_CONTENT_MAX) else {
        return Ok(());
    };
    // 上限超過を示す最小の範囲を渡し、従来の Twilight エラー型を保つ。
    let prefix = content
        .get(..offset + character.len_utf8())
        .unwrap_or(content);
    twilight_validate::message::content(prefix)?;
    Ok(())
}

/// Embed の個数、合計文字数、各項目の制限を送信前に確認します。
///
/// # エラー
///
/// Discord の個数・文字数・書式の制限に合わない場合は失敗します。
pub fn embeds(embeds: &[Embed]) -> Result<()> {
    if embeds.len() > message::EMBED_COUNT_LIMIT {
        return Err(Error::InvalidUi("Embed は 1 メッセージに 10 個までです。"));
    }
    let mut remaining = embed::EMBED_TOTAL_LENGTH;
    for embed in embeds {
        if embed.fields.len() > embed::FIELD_COUNT {
            return Err(Error::InvalidUi(
                "Embed の項目は 1 個の Embed に 25 個までです。",
            ));
        }
        let text = embed
            .title
            .as_deref()
            .into_iter()
            .chain(embed.description.as_deref())
            .chain(embed.author.as_ref().map(|author| author.name.as_str()))
            .chain(embed.footer.as_ref().map(|footer| footer.text.as_str()))
            .chain(
                embed
                    .fields
                    .iter()
                    .flat_map(|field| [field.name.as_str(), field.value.as_str()]),
            );
        for part in text {
            let count = part.chars().take(remaining + 1).count();
            remaining = remaining.checked_sub(count).ok_or(Error::InvalidUi(
                "Embed の合計は 6000 文字以下にしてください。",
            ))?;
        }
    }
    message::embeds(embeds)?;
    Ok(())
}

/// 保持する添付と追加する添付の合計が、1メッセージの上限に収まるか調べます。
///
/// # エラー
///
/// 合計が [`MESSAGE_ATTACHMENTS_MAX`] を超える場合は [`Error::InvalidUi`] を返します。
pub fn attachment_count(retained: usize, uploaded: usize) -> Result<()> {
    if retained.saturating_add(uploaded) > MESSAGE_ATTACHMENTS_MAX {
        return Err(Error::InvalidUi(
            "メッセージの添付ファイルは保持分を含めて 10 個までです。",
        ));
    }
    Ok(())
}

/// Discord が通知した、1ファイルあたりのバイト上限を各添付と照合します。
///
/// ファイルのバイト長だけを確認します。
///
/// # エラー
///
/// ファイル数の上限を超えた場合、または上限より大きいファイルがある場合は
/// [`Error::InvalidUi`] を返します。
pub fn attachment_sizes(uploads: &[Attachment], max_file_bytes: u64) -> Result<()> {
    attachment_count(0, uploads.len())?;
    if uploads.iter().any(|attachment| {
        u64::try_from(attachment.file.len()).map_or(true, |size| size > max_file_bytes)
    }) {
        return Err(Error::InvalidUi(
            "添付ファイルのサイズが Discord から通知された 1 ファイルの上限を超えています。",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_preflight_counts_unicode_characters_and_stops_at_the_limit() -> Result<()> {
        message_content(&"a".repeat(MESSAGE_CONTENT_MAX))?;
        message_content(&"🦀".repeat(MESSAGE_CONTENT_MAX))?;
        assert!(matches!(
            message_content(&"🦀".repeat(MESSAGE_CONTENT_MAX + 1)),
            Err(Error::MessageValidation(_))
        ));
        Ok(())
    }

    #[test]
    fn attachment_preflight_handles_overflow_and_individual_byte_limits() -> Result<()> {
        attachment_count(8, 2)?;
        assert!(attachment_count(8, 3).is_err());
        assert!(attachment_count(usize::MAX, 1).is_err());
        let uploads = [
            Attachment::from_bytes("a.txt".into(), vec![1, 2], 0),
            Attachment::from_bytes("b.txt".into(), vec![3, 4], 1),
        ];
        attachment_sizes(&uploads, 2)?;
        assert!(attachment_sizes(&uploads, 1).is_err());
        Ok(())
    }
}
