use std::{
    collections::{HashMap, hash_map::Entry},
    hash::Hash,
    num::NonZeroU16,
    sync::{Arc, Mutex, PoisonError},
};

use tokio::sync::{AcquireError, OwnedSemaphorePermit, Semaphore};
use twilight_model::id::{Id, marker::UserMarker};

/// コマンドなどを、指定したキーごとに何件まで同時に実行できるかを制限します。
///
/// コマンドごとに作ると、別のコマンドには影響しません。同じインスタンスをハイブリッド
/// ハンドラーで使うと、テキストとスラッシュの両方で枠を共有します。
#[derive(Debug)]
pub struct ConcurrencyLimit<K = Id<UserMarker>> {
    maximum: usize,
    slots: Mutex<HashMap<K, Slot>>,
}

#[derive(Debug)]
struct Slot {
    semaphore: Arc<Semaphore>,
    users: usize,
}

impl<K: Copy + Eq + Hash> ConcurrencyLimit<K> {
    /// 同じキーで同時に実行できる最大件数を指定します。
    pub fn new(maximum: NonZeroU16) -> Self {
        Self {
            maximum: usize::from(maximum.get()),
            slots: Mutex::new(HashMap::new()),
        }
    }

    /// 空いている枠を待たずに取得します。すべて使用中なら `None` です。
    pub fn try_acquire(&self, key: K) -> Option<ConcurrencyPermit<'_, K>> {
        let (semaphore, lease) = self.reserve(key);
        let permit = semaphore.try_acquire_owned().ok()?;
        Some(ConcurrencyPermit {
            _permit: permit,
            _lease: lease,
        })
    }

    /// 枠が空くまで待ちます。待機は同じキーの中で FIFO になり、キャンセルできます。
    ///
    /// Discord の初回応答期限があるコマンドでは、待つ前に `defer` します。
    ///
    /// # エラー
    /// 内部のセマフォが閉じられた場合にエラーを返します。
    pub async fn acquire(&self, key: K) -> Result<ConcurrencyPermit<'_, K>, AcquireError> {
        let (semaphore, lease) = self.reserve(key);
        let permit = semaphore.acquire_owned().await?;
        Ok(ConcurrencyPermit {
            _permit: permit,
            _lease: lease,
        })
    }

    fn reserve(&self, key: K) -> (Arc<Semaphore>, Lease<'_, K>) {
        let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        let slot = slots.entry(key).or_insert_with(|| Slot {
            semaphore: Arc::new(Semaphore::new(self.maximum)),
            users: 0,
        });
        slot.users += 1;
        (Arc::clone(&slot.semaphore), Lease { owner: self, key })
    }
}

/// 実行中の 1 枠です。処理の終了まで変数に保持すると、スコープを抜けた時点で枠が戻ります。
#[must_use = "実行が終わるまで、取得した枠を保持してください"]
pub struct ConcurrencyPermit<'a, K: Copy + Eq + Hash = Id<UserMarker>> {
    _permit: OwnedSemaphorePermit,
    _lease: Lease<'a, K>,
}

struct Lease<'a, K: Copy + Eq + Hash> {
    owner: &'a ConcurrencyLimit<K>,
    key: K,
}

impl<K: Copy + Eq + Hash> Drop for Lease<'_, K> {
    fn drop(&mut self) {
        let mut slots = self
            .owner
            .slots
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Entry::Occupied(mut entry) = slots.entry(self.key) {
            let slot = entry.get_mut();
            slot.users -= 1;
            if slot.users == 0 {
                entry.remove();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_each_key_and_removes_idle_entries() {
        let limiter = ConcurrencyLimit::<u64>::new(NonZeroU16::MIN);
        let first = limiter.try_acquire(1).unwrap();
        assert!(limiter.try_acquire(1).is_none());
        let second = limiter.try_acquire(2).unwrap();
        drop(first);
        assert!(limiter.try_acquire(1).is_some());
        drop(second);
        assert!(limiter.slots.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancellation_releases_waiting_keys_and_new_callers_do_not_skip_waiters() {
        let limiter = ConcurrencyLimit::new(NonZeroU16::MIN);
        let active = limiter.try_acquire(()).unwrap();
        {
            let cancelled = limiter.acquire(());
            tokio::pin!(cancelled);
            tokio::select! {
                biased;
                _ = &mut cancelled => panic!("the active permit occupies the only slot"),
                () = tokio::task::yield_now() => {}
            }
            assert_eq!(limiter.slots.lock().unwrap().get(&()).unwrap().users, 2);
        }
        assert_eq!(limiter.slots.lock().unwrap().get(&()).unwrap().users, 1);

        let waiting = limiter.acquire(());
        tokio::pin!(waiting);
        tokio::select! {
            biased;
            _ = &mut waiting => panic!("the active permit occupies the only slot"),
            () = tokio::task::yield_now() => {}
        }
        drop(active);
        assert!(limiter.try_acquire(()).is_none());
        let resumed = waiting.await.unwrap();
        assert!(limiter.try_acquire(()).is_none());
        drop(resumed);
        assert!(limiter.slots.lock().unwrap().is_empty());
    }
}
