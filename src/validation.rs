//! Discord に送る前に確認できるメッセージの制限です。

use twilight_model::{channel::message::Embed, http::attachment::Attachment};
use twilight_validate::{embed, message};

use crate::{Error, Result};

/// 通常のメッセージ本文の最大文字数です。
pub const MESSAGE_CONTENT_MAX: usize = twilight_validate::message::MESSAGE_CONTENT_LENGTH_MAX;

/// 一つのメッセージに保持できる添付ファイルの最大数です。
pub const MESSAGE_ATTACHMENTS_MAX: usize = 10;

pub(crate) fn exceeds_character_limit(value: &str, limit: usize) -> bool {
    value.len() > limit
        && (value.len() > limit.saturating_mul(4) || value.chars().nth(limit).is_some())
}

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
    AttachmentLimits::default().validate_uploads(uploads, Some(max_file_bytes))
}

/// チャンネルへのメッセージ作成リクエスト全体の上限です。
pub const CREATE_MESSAGE_REQUEST_MAX_BYTES: u64 = 25 * 1024 * 1024;

/// Discord の上限と併用する、アプリ独自の添付ファイル制限です。
///
/// 既定ではアプリの追加制限を適用しません。各設定の None はその検査を解除します。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[must_use]
pub struct AttachmentLimits {
    files: Option<usize>,
    file_bytes: Option<u64>,
    total_bytes: Option<u64>,
}

impl AttachmentLimits {
    /// 保持分を含むメッセージ全体の添付数を制限します。
    pub const fn max_files(mut self, limit: Option<usize>) -> Self {
        self.files = limit;
        self
    }

    /// 新規ファイルごとのバイト数を制限します。
    pub const fn max_file_bytes(mut self, limit: Option<u64>) -> Self {
        self.file_bytes = limit;
        self
    }

    /// 新しくアップロードするファイルの合計バイト数を制限します。
    pub const fn max_total_bytes(mut self, limit: Option<u64>) -> Self {
        self.total_bytes = limit;
        self
    }

    /// 保持分の件数と、新規ファイルのバイト長を調べます。ファイルは読み込みません。
    ///
    /// discord_file_max には Discord が通知した上限を指定します。不明な場合は None を渡します。
    ///
    /// # エラー
    /// Discord またはアプリの上限を超えると InvalidUi を返します。
    pub fn validate(
        self,
        retained: usize,
        uploads: impl IntoIterator<Item = u64>,
        discord_file_max: Option<u64>,
    ) -> Result<()> {
        self.count(retained, 0)?;
        let mut remaining = self.total_bytes;
        for (index, size) in uploads.into_iter().enumerate() {
            self.count(retained, index + 1)?;
            validate_file_size(size, discord_file_max, self.file_bytes, &mut remaining)?;
        }
        Ok(())
    }

    pub(crate) fn validate_uploads(
        self,
        uploads: &[Attachment],
        discord_file_max: Option<u64>,
    ) -> Result<()> {
        self.count(0, uploads.len())?;
        if self.file_bytes.is_none() && self.total_bytes.is_none() && discord_file_max.is_none() {
            return Ok(());
        }
        let mut remaining = self.total_bytes;
        for file in uploads {
            validate_file_size(
                file.file.len() as u64,
                discord_file_max,
                self.file_bytes,
                &mut remaining,
            )?;
        }
        Ok(())
    }

    pub(crate) fn limits_retained_count(self) -> bool {
        self.files
            .is_some_and(|limit| limit < MESSAGE_ATTACHMENTS_MAX)
    }

    pub(crate) fn count(self, retained: usize, uploaded: usize) -> Result<()> {
        attachment_count(retained, uploaded)?;
        if self
            .files
            .is_some_and(|limit| retained.saturating_add(uploaded) > limit)
        {
            return Err(Error::InvalidUi(
                "添付ファイル数がアプリの設定上限を超えています。",
            ));
        }
        Ok(())
    }

    pub(crate) fn restrict(self, other: Self) -> Self {
        Self {
            files: smaller(self.files, other.files),
            file_bytes: smaller(self.file_bytes, other.file_bytes),
            total_bytes: smaller(self.total_bytes, other.total_bytes),
        }
    }
}

fn smaller<T: Ord>(left: Option<T>, right: Option<T>) -> Option<T> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (value, None) | (None, value) => value,
    }
}

fn validate_file_size(
    size: u64,
    discord_file_max: Option<u64>,
    file_bytes: Option<u64>,
    remaining: &mut Option<u64>,
) -> Result<()> {
    if discord_file_max.is_some_and(|limit| size > limit) {
        return Err(Error::InvalidUi(
            "添付ファイルのサイズが Discord から通知された 1 ファイルの上限を超えています。",
        ));
    }
    if file_bytes.is_some_and(|limit| size > limit) {
        return Err(Error::InvalidUi(
            "添付ファイルのサイズがアプリの設定上限を超えています。",
        ));
    }
    if let Some(bytes) = remaining {
        *bytes = bytes.checked_sub(size).ok_or(Error::InvalidUi(
            "添付ファイルの合計サイズが上限を超えています。",
        ))?;
    }
    Ok(())
}

pub(crate) fn reply_files(
    content: &str,
    files: &[Attachment],
    limits: AttachmentLimits,
    discord_file_max: Option<u64>,
) -> Result<()> {
    if content.is_empty() && files.is_empty() {
        return Err(Error::InvalidUi("本文か添付ファイルを指定してください。"));
    }
    limits.validate_uploads(files, discord_file_max)?;
    message_content(content)?;
    attachment_metadata(files)
}

pub(crate) fn attachment_metadata(files: &[Attachment]) -> Result<()> {
    for (index, file) in files.iter().enumerate() {
        message::attachment(file)?;
        if !valid_attachment_name(&file.filename) {
            return Err(Error::InvalidUi("添付ファイルにはファイル名が必要です。"));
        }
        if files
            .iter()
            .take(index)
            .any(|previous| previous.id == file.id || previous.filename == file.filename)
        {
            return Err(Error::InvalidUi(
                "添付 ID とファイル名は、メッセージ内で重複できません。",
            ));
        }
    }
    Ok(())
}

pub(crate) fn valid_attachment_name(filename: &str) -> bool {
    !filename.trim().is_empty()
        && !matches!(filename, "." | "..")
        && !filename.contains(['/', '\\'])
        && !filename.chars().any(char::is_control)
}

pub(crate) fn channel_message_upload_size(uploads: &[Attachment]) -> Result<()> {
    // 添付データだけでリクエスト上限に達する場合、multipart の情報を収められません。
    let mut remaining = CREATE_MESSAGE_REQUEST_MAX_BYTES - 1;
    for file in uploads {
        remaining = remaining
            .checked_sub(file.file.len() as u64)
            .ok_or(Error::InvalidUi(
                "添付ファイルだけでメッセージ作成リクエストの上限に達しています。",
            ))?;
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

    #[test]
    fn app_upload_policy_combines_limits_and_allows_each_check_to_be_disabled() -> Result<()> {
        let policy = AttachmentLimits::default()
            .max_files(Some(3))
            .max_file_bytes(Some(8))
            .max_total_bytes(Some(10));
        policy.validate(1, [8, 2], Some(20))?;
        assert!(policy.validate(2, [3, 3], None).is_err());
        assert!(policy.validate(0, [4, 7], Some(20)).is_err());
        assert!(policy.validate(0, [8], Some(7)).is_err());
        assert!(policy.validate(0, [9], None).is_err());
        policy.max_file_bytes(None).validate(0, [9], None)?;
        policy.max_total_bytes(None).validate(0, [8, 8], None)?;
        policy.max_files(None).validate(2, [1, 1], None)?;
        let policy = AttachmentLimits::default().max_total_bytes(Some(u64::MAX));
        assert!(policy.validate(0, [u64::MAX, 1], None).is_err());
        assert!(AttachmentLimits::default().validate(10, [0], None).is_err());
        Ok(())
    }

    #[test]
    fn app_upload_policy_stops_reading_at_first_error() {
        let sizes = [3, 2].into_iter().chain(std::iter::once_with(|| {
            panic!("oversized iterator was not stopped")
        }));
        assert!(
            AttachmentLimits::default()
                .max_total_bytes(Some(4))
                .validate(0, sizes, None)
                .is_err()
        );
    }

    #[test]
    fn classic_file_replies_share_limits_and_validate_metadata() -> Result<()> {
        let limit = AttachmentLimits::default().max_file_bytes(Some(4));
        let pdf = Attachment::from_bytes("report.pdf".into(), vec![1, 2, 3, 4], 0);
        reply_files("", std::slice::from_ref(&pdf), limit, Some(10))?;
        assert!(reply_files("", &[], limit, None).is_err());
        assert!(reply_files("", std::slice::from_ref(&pdf), limit, Some(3)).is_err());
        let repeated_id = Attachment::from_bytes("second.pdf".into(), vec![1], 0);
        assert!(reply_files("File", &[pdf, repeated_id], limit, Some(10)).is_err());
        let invalid_name = Attachment::from_bytes("../file".into(), vec![1], 1);
        assert!(reply_files("File", &[invalid_name], limit, None).is_err());
        Ok(())
    }
}
