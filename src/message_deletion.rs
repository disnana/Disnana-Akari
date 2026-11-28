use std::{
    collections::HashSet,
    time::{SystemTime, SystemTimeError, UNIX_EPOCH},
};

use twilight_http::{Client, request::AuditLogReason};
use twilight_model::id::{
    Id,
    marker::{ChannelMarker, MessageMarker},
};

const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;
const BULK_MAX_AGE_MS: u128 = 14 * 24 * 60 * 60 * 1000;
const BULK_MAX_MESSAGES: usize = 100;

/// 指定したチャンネルのメッセージを ID で削除します。
///
/// 0 件なら通信せず、1 件なら通常の削除を行います。2〜100 件は 1 回の一括削除を行います。
/// 一括削除はサーバー内のチャンネル専用で、Bot に `MANAGE_MESSAGES` が必要です。
/// 1 件の場合も他のユーザーの投稿を削除するには `MANAGE_MESSAGES` が必要です。
/// 履歴を取得したり、失敗した一括削除を個別の削除に切り替えたりすることはありません。
///
/// # エラー
///
/// 一括削除で ID が重複する場合、100 件を超える場合、または作成から 14 日を超える
/// メッセージがある場合は送信前にエラーを返します。権限と削除の可否は Discord が判定します。
/// 境界付近では通信中に 14 日を超え、Discord が拒否する場合があります。
pub async fn delete_messages(
    http: &Client,
    channel: Id<ChannelMarker>,
    messages: &[Id<MessageMarker>],
    reason: Option<&str>,
) -> Result<(), MessageDeletionError> {
    match messages {
        [] => {}
        [message] => {
            let mut request = http.delete_message(channel, *message);
            if let Some(reason) = reason {
                request = request.reason(reason);
            }
            request.await?;
        }
        messages => {
            validate_bulk(messages, SystemTime::now())?;
            let mut request = http.delete_messages(channel, messages);
            if let Some(reason) = reason {
                request = request.reason(reason);
            }
            request.await?;
        }
    }
    Ok(())
}

fn validate_bulk(
    messages: &[Id<MessageMarker>],
    now: SystemTime,
) -> Result<(), MessageDeletionError> {
    if messages.len() > BULK_MAX_MESSAGES {
        return Err(MessageDeletionError::TooMany {
            count: messages.len(),
        });
    }
    let mut unique = HashSet::with_capacity(messages.len());
    for &message in messages {
        if !unique.insert(message) {
            return Err(MessageDeletionError::Duplicate { message });
        }
    }

    let now_ms = now.duration_since(UNIX_EPOCH)?.as_millis();
    let cutoff = now_ms.saturating_sub(BULK_MAX_AGE_MS);
    for &message in messages {
        let created_ms = DISCORD_EPOCH_MS + (message.get() >> 22);
        if u128::from(created_ms) < cutoff {
            return Err(MessageDeletionError::TooOld { message });
        }
    }
    Ok(())
}

/// メッセージを削除できなかった理由です。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MessageDeletionError {
    /// 一括削除で指定できる件数の上限を超えています。
    #[error("一括削除は 100 件までです（{count} 件）")]
    TooMany {
        /// 指定された件数です。
        count: usize,
    },
    /// 同じメッセージが複数回指定されています。
    #[error("一括削除に同じメッセージ ID が重複しています")]
    Duplicate {
        /// 重複したメッセージの ID です。
        message: Id<MessageMarker>,
    },
    /// 一括削除の対象に作成から 14 日を超えるメッセージがあります。
    #[error("作成から 14 日を超えるメッセージは一括削除できません")]
    TooOld {
        /// 古すぎるメッセージの ID です。
        message: Id<MessageMarker>,
    },
    /// 一括削除の事前確認に使うシステム時刻が UNIX エポックより前でした。
    #[error("一括削除の事前確認に使う現在時刻を読み取れません")]
    Clock(#[from] SystemTimeError),
    /// Discord へのリクエストまたは監査理由の設定に失敗しました。
    #[error("メッセージを削除できませんでした")]
    Http(#[from] twilight_http::Error),
}

#[cfg(test)]
mod tests;
