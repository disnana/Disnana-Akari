use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use disnana_akari::model::id::{Id, marker::UserMarker};
use songbird::{CoreEvent, Driver, Event, EventContext, EventHandler};

const MAX_SPEAKERS: usize = 128;

/// 一つの Driver で観測した Discord ユーザーと音声ストリームの対応です。
#[derive(Clone, Default)]
pub(super) struct Speakers(Arc<SpeakerState>);

impl Speakers {
    pub(super) fn install(driver: &mut Driver) -> Self {
        let speakers = Self::default();
        for event in [
            CoreEvent::SpeakingStateUpdate,
            CoreEvent::ClientDisconnect,
            CoreEvent::DriverReconnect,
            CoreEvent::DriverDisconnect,
        ] {
            driver.add_global_event(Event::Core(event), speakers.clone());
        }
        speakers
    }

    pub(super) fn ssrc(&self, user: Id<UserMarker>) -> Option<u32> {
        if self.0.disabled.load(Ordering::Acquire) {
            return None;
        }
        let map = self.0.map.try_lock().ok()?;
        let ssrc = map.by_user.get(&user).copied();
        if self.0.disabled.load(Ordering::Acquire) {
            return None;
        }
        ssrc
    }

    #[cfg(test)]
    pub(super) fn observe_for_test(&self, user: Id<UserMarker>, ssrc: u32) {
        self.update(|map| map.observe(user, ssrc));
    }

    fn update(&self, apply: impl FnOnce(&mut SpeakerMap)) {
        if self.0.disabled.load(Ordering::Acquire) {
            return;
        }
        if let Ok(mut map) = self.0.map.try_lock() {
            apply(&mut map);
        } else {
            // 更新を取りこぼすと過去の対応が残るため、この Driver での識別を止めます。
            self.disable();
        }
    }

    fn disable(&self) {
        self.0.disabled.store(true, Ordering::Release);
    }
}

#[async_trait]
impl EventHandler for Speakers {
    async fn act(&self, context: &EventContext<'_>) -> Option<Event> {
        match context {
            EventContext::SpeakingStateUpdate(event) => {
                if let Some(user) = event.user_id.and_then(|id| Id::new_checked(id.0)) {
                    self.update(|map| map.observe(user, event.ssrc));
                }
            }
            EventContext::ClientDisconnect(event) => {
                if let Some(user) = Id::new_checked(event.user_id.0) {
                    self.update(|map| {
                        map.by_user.remove(&user);
                    });
                }
            }
            EventContext::DriverReconnect(_) | EventContext::DriverDisconnect(_) => {
                // Songbird 0.6.1 は再開時と新規接続時を同じイベントで通知します。
                // 通知後の対応を再利用せず、新しい Driver ができるまで話者別の識別を停止します。
                self.disable();
            }
            _ => {}
        }
        None
    }
}

#[derive(Default)]
struct SpeakerState {
    disabled: AtomicBool,
    map: Mutex<SpeakerMap>,
}

#[derive(Default)]
struct SpeakerMap {
    by_user: HashMap<Id<UserMarker>, u32>,
}

impl SpeakerMap {
    fn observe(&mut self, user: Id<UserMarker>, ssrc: u32) {
        if self.by_user.get(&user) == Some(&ssrc) {
            return;
        }
        // サーバーが SSRC を別の利用者に割り当て直したら、古い対応を取り除きます。
        self.by_user.retain(|_, existing| *existing != ssrc);
        if self.by_user.len() < MAX_SPEAKERS || self.by_user.contains_key(&user) {
            self.by_user.insert(user, ssrc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_or_reassigning_an_ssrc_never_returns_the_previous_mapping() {
        let speakers = Speakers::default();
        let first = Id::new(1);
        let second = Id::new(2);
        speakers.update(|map| map.observe(first, 10));
        speakers.update(|map| map.observe(first, 11));
        speakers.update(|map| map.observe(second, 10));
        assert_eq!(speakers.ssrc(first), Some(11));
        assert_eq!(speakers.ssrc(second), Some(10));
        speakers.update(|map| map.observe(second, 11));
        assert_eq!(speakers.ssrc(first), None);
        assert_eq!(speakers.ssrc(second), Some(11));
    }

    #[test]
    fn speaker_table_stays_bounded_and_can_reuse_a_departed_slot() {
        let mut map = SpeakerMap::default();
        for index in 1..=MAX_SPEAKERS {
            map.observe(Id::new(index as u64), index as u32);
        }
        let extra = Id::new(MAX_SPEAKERS as u64 + 1);
        map.observe(extra, 1000);
        assert_eq!(map.by_user.len(), MAX_SPEAKERS);
        assert_eq!(map.by_user.get(&extra), None);
        map.observe(Id::new(1), 1001);
        assert_eq!(map.by_user.get(&Id::new(1)), Some(&1001));
        map.by_user.remove(&Id::new(1));
        map.observe(extra, 1000);
        assert_eq!(map.by_user.len(), MAX_SPEAKERS);
        assert_eq!(map.by_user.get(&extra), Some(&1000));
    }

    #[test]
    fn disable_and_contended_updates_fail_closed() {
        let speakers = Speakers::default();
        let user = Id::new(1);
        speakers.update(|map| map.observe(user, 101));
        assert_eq!(speakers.ssrc(user), Some(101));

        let guard = speakers.0.map.lock().unwrap();
        assert_eq!(speakers.ssrc(user), None);
        speakers.update(|map| map.observe(user, 102));
        drop(guard);
        speakers.update(|map| map.observe(user, 103));
        assert_eq!(speakers.ssrc(user), None);

        let fresh = Speakers::default();
        fresh.update(|map| map.observe(user, 104));
        fresh.disable();
        assert_eq!(fresh.ssrc(user), None);
    }
}
