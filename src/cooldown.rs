use std::{
    collections::{HashMap, VecDeque},
    hash::Hash,
    num::NonZeroU32,
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

use twilight_model::id::{Id, marker::UserMarker};

/// 指定した期間内に実行できる回数を、キーごとに制限します。
///
/// 一つのインスタンスが一つの制限を表します。コマンドや呼び出し方ごとに
/// インスタンスを作ると、それぞれ独立して制限できます。
#[derive(Debug)]
pub struct Cooldown<K = Id<UserMarker>> {
    interval: Duration,
    rate: NonZeroU32,
    active: Mutex<ActiveKeys<K>>,
}

impl Cooldown {
    /// ユーザーごとに適用する間隔を指定します。`Duration::ZERO` なら常に通過します。
    pub fn new(interval: Duration) -> Self {
        Self::per_key(NonZeroU32::MIN, interval)
    }
}

impl<K: Copy + Eq + Hash> Cooldown<K> {
    /// 同じキーで、最初の実行から `interval` の間に `rate` 回まで通します。
    ///
    /// キーにはユーザー ID、チャンネル ID、複数の ID のタプルなどを使えます。
    /// `Duration::ZERO` なら常に通過します。
    pub fn per_key(rate: NonZeroU32, interval: Duration) -> Self {
        Self {
            interval,
            rate,
            active: Mutex::new(ActiveKeys {
                by_key: HashMap::new(),
                started: VecDeque::new(),
            }),
        }
    }

    /// 実行できる場合は使用を記録し、`Ok(())` を返します。
    ///
    /// # エラー
    ///
    /// まだ実行できない場合は、残りの待ち時間を `Err` で返します。
    /// 拒否された呼び出しで待ち時間が延びることはありません。
    pub fn check(&self, key: K) -> Result<(), Duration> {
        if self.interval.is_zero() {
            return Ok(());
        }

        let mut active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        active.check(key, self.rate, self.interval, Instant::now())
    }
}

#[derive(Debug)]
struct ActiveKeys<K> {
    by_key: HashMap<K, Window>,
    started: VecDeque<(K, Instant)>,
}

#[derive(Debug)]
struct Window {
    started_at: Instant,
    remaining: u32,
}

impl<K: Copy + Eq + Hash> ActiveKeys<K> {
    fn check(
        &mut self,
        key: K,
        rate: NonZeroU32,
        interval: Duration,
        now: Instant,
    ) -> Result<(), Duration> {
        // 全キーの期間は同じ長さなので、先に始まった期間から期限が切れる。
        while let Some(&(old_key, started_at)) = self.started.front() {
            if now.saturating_duration_since(started_at) < interval {
                break;
            }
            self.started.pop_front();
            self.by_key.remove(&old_key);
        }

        if let Some(window) = self.by_key.get_mut(&key) {
            if window.remaining == 0 {
                return Err(
                    interval.saturating_sub(now.saturating_duration_since(window.started_at))
                );
            }
            window.remaining -= 1;
            return Ok(());
        }

        self.by_key.insert(
            key,
            Window {
                started_at: now,
                remaining: rate.get() - 1,
            },
        );
        self.started.push_back((key, now));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Cooldown, Duration, Id, Instant, NonZeroU32};

    #[test]
    fn rejected_attempt_does_not_extend_wait_or_affect_another_user() {
        let cooldown = Cooldown::new(Duration::from_secs(3));
        let mut active = cooldown.active.lock().unwrap();
        let start = Instant::now();
        let interval = Duration::from_secs(3);
        let next_second = start + Duration::from_secs(1);
        let first = Id::new(1);
        let second = Id::new(2);

        let rate = NonZeroU32::MIN;
        assert_eq!(active.check(first, rate, interval, start), Ok(()));
        assert_eq!(
            active.check(first, rate, interval, next_second),
            Err(Duration::from_secs(2))
        );
        assert_eq!(active.check(second, rate, interval, next_second), Ok(()));
        assert_eq!(
            active.check(first, rate, interval, start + interval),
            Ok(())
        );
        assert_eq!(active.started.len(), 2);
        assert_eq!(active.by_key.len(), 2);
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

    #[test]
    fn burst_consumes_only_the_current_keys_window() {
        let rate = NonZeroU32::new(2).unwrap();
        let interval = Duration::from_secs(5);
        let scoped = Cooldown::<(u64, u64)>::per_key(rate, interval);
        let mut active = scoped.active.lock().unwrap();
        let started = Instant::now();
        let key = (1, 2);
        let other = (1, 3);
        assert_eq!(active.check(key, rate, interval, started), Ok(()));
        let after_two_seconds = started + Duration::from_secs(2);
        assert_eq!(active.check(key, rate, interval, after_two_seconds), Ok(()));
        assert_eq!(
            active.check(other, rate, interval, after_two_seconds),
            Ok(())
        );
        assert_eq!(
            active.check(key, rate, interval, after_two_seconds),
            Err(Duration::from_secs(3))
        );
        assert_eq!(
            active.check(key, rate, interval, started + interval),
            Ok(())
        );
        assert_eq!(
            active.check(other, rate, interval, started + interval),
            Ok(())
        );
        assert_eq!(
            active.check(other, rate, interval, started + interval),
            Err(Duration::from_secs(2))
        );
        assert_eq!(active.started.len(), 2);
    }
}
