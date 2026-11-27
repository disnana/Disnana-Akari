use twilight_gateway::EventTypeFlags;

use crate::member_cache::{CachedMemberEvent, CachedMemberRemove, CachedMemberUpdate};

use super::{Events, Handler, Reporter, adapt_handler, dispatch};

#[derive(Default)]
pub(super) struct CachedMemberHandlers {
    update: Vec<Box<dyn Handler<CachedMemberUpdate>>>,
    remove: Vec<Box<dyn Handler<CachedMemberRemove>>>,
}

impl Events {
    /// 保存済みのサーバーメンバーが更新されたときに更新前後の状態を受け取ります。
    ///
    /// [`crate::Bot::cache_members`] と `GUILDS`・`GUILD_MEMBERS` Intent が必要です。
    /// キャッシュにないメンバーの更新は [`Events::on_gateway`] で受け取れます。
    pub fn on_member_update<E>(
        &mut self,
        handler: impl AsyncFn(&CachedMemberUpdate) -> std::result::Result<(), E> + Send + Sync + 'static,
    ) where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.cached_members.update.push(Box::new(
            adapt_handler!(handler, event: &CachedMemberUpdate),
        ));
    }

    /// 保存済みのサーバーメンバーが退出したときに最後の保存状態を受け取ります。
    ///
    /// [`crate::Bot::cache_members`] と `GUILDS`・`GUILD_MEMBERS` Intent が必要です。
    /// キャッシュにないメンバーの退出は [`Events::on_gateway`] で受け取れます。
    pub fn on_member_remove<E>(
        &mut self,
        handler: impl AsyncFn(&CachedMemberRemove) -> std::result::Result<(), E> + Send + Sync + 'static,
    ) where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.cached_members.remove.push(Box::new(
            adapt_handler!(handler, event: &CachedMemberRemove),
        ));
    }

    pub(crate) fn cached_member_types(&self) -> EventTypeFlags {
        let mut types = EventTypeFlags::empty();
        types.set(
            EventTypeFlags::MEMBER_UPDATE,
            !self.cached_members.update.is_empty(),
        );
        types.set(
            EventTypeFlags::MEMBER_REMOVE,
            !self.cached_members.remove.is_empty(),
        );
        types
    }

    pub(crate) async fn dispatch_cached_member_event(
        &self,
        event: &CachedMemberEvent,
        reporter: &Reporter,
    ) {
        match event {
            CachedMemberEvent::Update(event) => {
                dispatch(
                    &self.cached_members.update,
                    event,
                    "GUILD_MEMBER_UPDATE",
                    reporter,
                )
                .await
            }
            CachedMemberEvent::Remove(event) => {
                dispatch(
                    &self.cached_members.remove,
                    event,
                    "GUILD_MEMBER_REMOVE",
                    reporter,
                )
                .await
            }
        }
    }
}
