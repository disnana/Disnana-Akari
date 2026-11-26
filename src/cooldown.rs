use std::{
    collections::{HashMap, VecDeque},
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

use twilight_model::id::{Id, marker::UserMarker};

/// 同じユーザーによる次のコマンド実行まで、指定した間隔を空けます。
///
/// 一つのインスタンスが一つの制限を表します。コマンドや呼び出し方ごとに
/// インスタンスを作ると、それぞれ独立して制限できます。
#[derive(Debug)]
pub struct Cooldown {
    interval: Duration,
    active: Mutex<ActiveUsers>,
}

impl Cooldown {
    /// ユーザーごとに適用する間隔を指定します。`Duration::ZERO` なら常に通過します。
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            active: Mutex::new(ActiveUsers::default()),
        }
    }

    /// 実行できる場合は使用を記録し、`Ok(())` を返します。
    ///
    /// # エラー
    ///
    /// まだ実行できない場合は、残りの待ち時間を `Err` で返します。
    /// 拒否された呼び出しで待ち時間が延びることはありません。
    pub fn check(&self, user: Id<UserMarker>) -> Result<(), Duration> {
        if self.interval.is_zero() {
            return Ok(());
        }

        let mut active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        active.check(user, self.interval, Instant::now())
    }
}

#[derive(Debug, Default)]
struct ActiveUsers {
    by_user: HashMap<Id<UserMarker>, Instant>,
    accepted: VecDeque<(Id<UserMarker>, Instant)>,
}

impl ActiveUsers {
    fn check(
        &mut self,
        user: Id<UserMarker>,
        interval: Duration,
        now: Instant,
    ) -> Result<(), Duration> {
        // 全ユーザーに同じ間隔を適用するので、先に通過したユーザーから期限が切れる。
        while let Some(&(old_user, accepted_at)) = self.accepted.front() {
            if now.saturating_duration_since(accepted_at) < interval {
                break;
            }
            self.accepted.pop_front();
            self.by_user.remove(&old_user);
        }

        if let Some(&accepted_at) = self.by_user.get(&user) {
            return Err(interval.saturating_sub(now.saturating_duration_since(accepted_at)));
        }

        self.by_user.insert(user, now);
        self.accepted.push_back((user, now));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{ActiveUsers, Cooldown, Duration, Id, Instant};

    #[test]
    fn rejected_attempt_does_not_extend_wait_or_affect_another_user() {
        let mut active = ActiveUsers::default();
        let start = Instant::now();
        let interval = Duration::from_secs(3);
        let next_second = start + Duration::from_secs(1);
        let first = Id::new(1);
        let second = Id::new(2);

        assert_eq!(active.check(first, interval, start), Ok(()));
        assert_eq!(
            active.check(first, interval, next_second),
            Err(Duration::from_secs(2))
        );
        assert_eq!(active.check(second, interval, next_second), Ok(()));
        assert_eq!(active.check(first, interval, start + interval), Ok(()));
        assert_eq!(active.accepted.len(), 2);
        assert_eq!(active.by_user.len(), 2);
    }

    #[test]
    fn instances_are_independent_and_zero_never_limits() {
        let user = Id::new(1);
        let text = Cooldown::new(Duration::from_secs(3));
        let slash = Cooldown::new(Duration::from_secs(1));
        let off = Cooldown::new(Duration::ZERO);

        assert_eq!(text.check(user), Ok(()));
        assert_eq!(slash.check(user), Ok(()));
        assert_eq!(off.check(user), Ok(()));
        assert_eq!(off.check(user), Ok(()));
    }
}
