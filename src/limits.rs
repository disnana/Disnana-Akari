use crate::{Error, Result};

/// 同時に処理する数と、待ち行列の大きさです。
///
/// 処理枠がすべて使われているとき、新しい呼び出しは混雑として扱われます
/// （[`Messages`](crate::Messages) と [`Bot::on_error`](crate::Bot::on_error) を参照）。
/// 既定値は、小〜中規模の Bot 向けです。
///
/// ```
/// use disnana_akari::Limits;
///
/// let limits = Limits::default().text_workers(16).queue_capacity(128);
/// assert_eq!(limits.text_workers, 16);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Limits {
    /// テキストコマンドを同時に処理する数です。既定は 8 です。
    pub text_workers: usize,
    /// スラッシュコマンド、ボタン、選択メニュー、フォームを同時に処理する数です。既定は 8 です。
    pub interaction_workers: usize,
    /// 混雑の通知を同時に返す数です。既定は 8 です。
    pub notice_workers: usize,
    /// Gateway から受け取った分を、処理の前にためておく数です。既定は 64 です。
    pub queue_capacity: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            text_workers: 8,
            interaction_workers: 8,
            notice_workers: 8,
            queue_capacity: 64,
        }
    }
}

impl Limits {
    /// テキストコマンドを同時に処理する数を変えます。
    pub fn text_workers(mut self, count: usize) -> Self {
        self.text_workers = count;
        self
    }

    /// スラッシュコマンドやボタンなどを同時に処理する数を変えます。
    pub fn interaction_workers(mut self, count: usize) -> Self {
        self.interaction_workers = count;
        self
    }

    /// 混雑の通知を同時に返す数を変えます。
    pub fn notice_workers(mut self, count: usize) -> Self {
        self.notice_workers = count;
        self
    }

    /// 処理の前にためておく数を変えます。
    pub fn queue_capacity(mut self, count: usize) -> Self {
        self.queue_capacity = count;
        self
    }

    /// 0 の項目がないか確認します。
    pub(crate) fn validate(&self) -> Result<()> {
        let fields = [
            ("text_workers", self.text_workers),
            ("interaction_workers", self.interaction_workers),
            ("notice_workers", self.notice_workers),
            ("queue_capacity", self.queue_capacity),
        ];
        match fields.into_iter().find(|(_, value)| *value == 0) {
            Some((field, _)) => Err(Error::InvalidLimits { field }),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_zero_is_rejected_by_name() {
        assert!(Limits::default().validate().is_ok());
        let limits = Limits::default().interaction_workers(0);
        assert!(matches!(
            limits.validate(),
            Err(Error::InvalidLimits {
                field: "interaction_workers"
            })
        ));
    }
}
