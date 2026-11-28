use std::num::NonZeroU16;

use serde::Deserialize;
use serde_json::value::RawValue;
use twilight_http::{Client, Error as HttpError, response::DeserializeBodyError};
use twilight_model::{
    guild::audit_log::{AuditLog, AuditLogEntry, AuditLogEventType},
    id::{
        Id,
        marker::{AuditLogEntryMarker, GuildMarker, UserMarker},
    },
};

use crate::rest_pagination::advance_after;

const MAX_PAGE_SIZE: u16 = 100;

/// Discord REST でサーバーの監査ログを必要なページだけ取得します。
///
/// Bot に `VIEW_AUDIT_LOG` が必要です。各ページには監査ログの項目と、そのページで参照される
/// ユーザーなどの関連情報が含まれます。読み取り中に項目が追加されることがあります。
pub struct AuditLogPages<'http> {
    http: &'http Client,
    guild: Id<GuildMarker>,
    cursor: Cursor,
    actor: Option<Id<UserMarker>>,
    action: Option<AuditLogEventType>,
    page_size: u16,
    remaining: Option<usize>,
    exhausted: bool,
}

#[derive(Clone, Copy)]
enum Cursor {
    Before(Option<Id<AuditLogEntryMarker>>),
    After(Option<Id<AuditLogEntryMarker>>),
}

impl<'http> AuditLogPages<'http> {
    /// 通信せずに取得を準備します。既定では最新の項目から、古い方向へ進みます。
    /// 1 ページあたり最大 100 件です。総件数を制限する場合は [`Self::limit`] を指定します。
    #[must_use]
    pub fn new(http: &'http Client, guild: Id<GuildMarker>) -> Self {
        Self {
            http,
            guild,
            cursor: Cursor::Before(None),
            actor: None,
            action: None,
            page_size: MAX_PAGE_SIZE,
            remaining: None,
            exhausted: false,
        }
    }

    /// この項目より古い項目を取得します。指定した ID は含まれません。
    /// 以前に指定した取得位置は置き換わります。
    #[must_use]
    pub fn before(mut self, entry: Id<AuditLogEntryMarker>) -> Self {
        self.cursor = Cursor::Before(Some(entry));
        self
    }

    /// この項目より新しい項目を取得します。指定した ID は含まれません。
    /// 以前に指定した取得位置は置き換わります。
    #[must_use]
    pub fn after(mut self, entry: Id<AuditLogEntryMarker>) -> Self {
        self.cursor = Cursor::After(Some(entry));
        self
    }

    /// 取得できる最も古い項目から、新しい方向へ進みます。
    /// 以前に指定した取得位置は置き換わります。
    #[must_use]
    pub fn oldest_first(mut self) -> Self {
        self.cursor = Cursor::After(None);
        self
    }

    /// 指定したユーザーが実行した操作だけを取得します。操作の対象者は絞り込みません。
    #[must_use]
    pub fn actor(mut self, user: Id<UserMarker>) -> Self {
        self.actor = Some(user);
        self
    }

    /// 指定した種類の操作だけを取得します。
    #[must_use]
    pub fn action(mut self, action: AuditLogEventType) -> Self {
        self.action = Some(action);
        self
    }

    /// 返す有効な監査ログ項目の総件数の上限を設定します。0 件の場合はリクエストを送りません。
    #[must_use]
    pub fn limit(mut self, limit: usize) -> Self {
        self.remaining = Some(limit);
        self
    }

    /// リクエスト 1 回あたりの上限を変更します。100 件を超える場合は 100 件とします。
    #[must_use]
    pub fn page_size(mut self, size: NonZeroU16) -> Self {
        self.page_size = size.get().min(MAX_PAGE_SIZE);
        self
    }

    /// 次の監査ログページを、関連情報と一緒に取得します。
    ///
    /// `after` または `oldest_first` では項目を古い順に、それ以外では新しい順に返します。
    /// Discord が操作の種類を `null` とした項目は読み飛ばします。そのような項目だけのページも、
    /// 関連情報とともに `Some` で返します。Discord 側の項目が空か、総件数に達した場合は `None` です。
    ///
    /// # エラー
    ///
    /// REST リクエストや応答の読み取りに失敗した場合、または応答に重複した ID、範囲外の ID、
    /// 要求件数を超える項目、あるいは指定のフィルターに合わない項目がある場合に失敗します。
    /// エラー時はカーソルを進めません。
    pub async fn next_page(&mut self) -> Result<Option<AuditLog>, AuditLogPagesError> {
        let limit = self
            .remaining
            .and_then(|remaining| u16::try_from(remaining).ok())
            .map_or(self.page_size, |remaining| self.page_size.min(remaining));
        if self.exhausted || limit == 0 {
            return Ok(None);
        }
        let mut request = self.http.audit_log(self.guild).limit(limit);
        request = match self.cursor {
            Cursor::Before(None) => request,
            Cursor::Before(Some(id)) => request.before(id.get()),
            Cursor::After(after) => request.after(after.map_or(0, Id::get)),
        };
        if let Some(actor) = self.actor {
            request = request.user_id(actor);
        }
        if let Some(action) = self.action {
            request = request.action_type(action);
        }
        let bytes = request.await?.bytes().await?;
        let (mut page, mut raw_ids) = decode_page(&bytes)?;
        let limit = usize::from(limit);
        if page.entries.iter().any(|entry| !self.matches(entry)) {
            return Err(AuditLogPagesError::InvalidPage);
        }
        let cursor = if let Some(ids) = &mut raw_ids {
            let cursor = self.cursor.advance(ids, limit, |id| *id)?;
            self.cursor
                .advance(&mut page.entries, limit, |entry| entry.id)?;
            cursor
        } else {
            self.cursor
                .advance(&mut page.entries, limit, |entry| entry.id)?
        };
        let Some(cursor) = cursor else {
            self.exhausted = true;
            return Ok(None);
        };
        self.cursor = cursor;
        if let Some(remaining) = &mut self.remaining {
            *remaining -= page.entries.len();
        }
        Ok(Some(page))
    }

    fn matches(&self, entry: &AuditLogEntry) -> bool {
        entry.guild_id.is_none_or(|guild| guild == self.guild)
            && self.actor.is_none_or(|actor| entry.user_id == Some(actor))
            && self.action.is_none_or(|action| entry.action_type == action)
    }
}

type EntryId = Id<AuditLogEntryMarker>;

#[derive(Deserialize)]
struct AuditRows<'a> {
    #[serde(borrow)]
    audit_log_entries: &'a RawValue,
}

#[derive(Deserialize)]
struct SkippableEntry<'a> {
    id: EntryId,
    #[serde(borrow)]
    action_type: &'a RawValue,
}

fn decode_page(bytes: &[u8]) -> Result<(AuditLog, Option<Vec<EntryId>>), AuditLogPagesError> {
    let strict_error = match serde_json::from_slice(bytes) {
        Ok(page) => return Ok((page, None)),
        Err(error) => error,
    };

    // Only the compatibility path retains raw entries to advance the cursor past null actions.
    let envelope: AuditRows<'_> = serde_json::from_slice(bytes)?;
    let raw_array = envelope.audit_log_entries.get();
    let raw_entries: Vec<&RawValue> = serde_json::from_str(raw_array)?;
    let mut ids = Vec::with_capacity(raw_entries.len());
    let mut entries = Vec::with_capacity(raw_entries.len());
    let mut skipped = false;
    for raw in raw_entries {
        match serde_json::from_str::<AuditLogEntry>(raw.get()) {
            Ok(entry) => {
                ids.push(entry.id);
                entries.push(entry);
            }
            Err(error) => {
                let candidate: SkippableEntry<'_> = serde_json::from_str(raw.get())?;
                if candidate.action_type.get() != "null" {
                    return Err(error.into());
                }
                ids.push(candidate.id);
                skipped = true;
            }
        }
    }
    if !skipped {
        return Err(strict_error.into());
    }
    let mut page = decode_side_tables(bytes, raw_array)?;
    page.entries = entries;
    Ok((page, Some(ids)))
}

fn decode_side_tables(bytes: &[u8], raw_array: &str) -> Result<AuditLog, AuditLogPagesError> {
    // `RawValue` here borrows the entry array directly from `bytes`. Only replace that
    // array so Twilight still interprets and validates every side table itself.
    let start = raw_array.as_ptr().addr().checked_sub(bytes.as_ptr().addr());
    let (head, tail) = start
        .and_then(|start| bytes.split_at_checked(start))
        .ok_or(AuditLogPagesError::InvalidPage)?;
    let tail = tail
        .get(raw_array.len()..)
        .ok_or(AuditLogPagesError::InvalidPage)?;
    let mut corrected = Vec::with_capacity(head.len() + 2 + tail.len());
    corrected.extend_from_slice(head);
    corrected.extend_from_slice(b"[]");
    corrected.extend_from_slice(tail);
    Ok(serde_json::from_slice(&corrected)?)
}

impl Cursor {
    fn advance<T>(
        self,
        entries: &mut [T],
        limit: usize,
        id: impl Fn(&T) -> EntryId,
    ) -> Result<Option<Self>, AuditLogPagesError> {
        let after = match self {
            Self::After(after) => after,
            Self::Before(_) => None,
        };
        let newest =
            advance_after(entries, limit, after, &id).ok_or(AuditLogPagesError::InvalidPage)?;
        let Some(newest) = newest else {
            return Ok(None);
        };
        match self {
            Self::After(_) => Ok(Some(Self::After(Some(newest)))),
            Self::Before(before) => {
                if before.is_some_and(|bound| newest >= bound) {
                    return Err(AuditLogPagesError::InvalidPage);
                }
                let oldest = entries
                    .first()
                    .map(id)
                    .ok_or(AuditLogPagesError::InvalidPage)?;
                entries.reverse();
                Ok(Some(Self::Before(Some(oldest))))
            }
        }
    }
}

/// REST でサーバーの監査ログをページ取得できなかった理由です。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AuditLogPagesError {
    /// Discord へのリクエストが失敗しました。
    #[error("Discord の監査ログを取得できませんでした")]
    Http(HttpError),
    /// Discord の応答本文を受信できませんでした。
    #[error("Discord の監査ログの応答を読み取れませんでした")]
    HttpBody(DeserializeBodyError),
    /// Discord の応答を監査ログとして解釈できませんでした。
    #[error("Discord の監査ログの応答形式が無効です")]
    Decode(#[from] serde_json::Error),
    /// 応答がカーソル、件数、ID の一意性、または指定フィルターの条件を満たしません。
    #[error("Discord の監査ログのページが無効です")]
    InvalidPage,
}

impl From<HttpError> for AuditLogPagesError {
    fn from(error: HttpError) -> Self {
        Self::Http(error)
    }
}

impl From<DeserializeBodyError> for AuditLogPagesError {
    fn from(error: DeserializeBodyError) -> Self {
        Self::HttpBody(error)
    }
}

#[cfg(test)]
mod tests;
