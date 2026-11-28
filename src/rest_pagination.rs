use std::num::NonZeroU16;

use twilight_model::{
    id::{Id, marker::UserMarker},
    user::User,
};

// 上限を確かめてから ID の昇順にそろえ、排他的なカーソルの進行を検証する。
pub(crate) fn advance_after<T, M>(
    items: &mut [T],
    limit: usize,
    cursor: Option<Id<M>>,
    id: impl Fn(&T) -> Id<M>,
) -> Option<Option<Id<M>>> {
    if items.len() > limit {
        return None;
    }
    if items.is_empty() {
        return Some(None);
    }
    items.sort_unstable_by_key(&id);
    let mut last = cursor;
    for item in items {
        let next = id(item);
        if last.is_some_and(|last| next <= last) {
            return None;
        }
        last = Some(next);
    }
    Some(last)
}

pub(crate) struct UserPageCursor {
    pub(crate) after: Option<Id<UserMarker>>,
    pub(crate) remaining: Option<usize>,
    page_size: u8,
    finished: bool,
}

pub(crate) struct InvalidUserPage;

impl UserPageCursor {
    pub(crate) const fn new() -> Self {
        Self {
            after: None,
            remaining: None,
            page_size: 100,
            finished: false,
        }
    }

    pub(crate) fn set_page_size(&mut self, size: NonZeroU16) {
        self.page_size = size.get().min(100) as u8;
    }

    pub(crate) fn request_size(&self) -> Option<u8> {
        if self.finished || self.remaining == Some(0) {
            return None;
        }
        let size = self
            .remaining
            .map_or(usize::from(self.page_size), |remaining| {
                remaining.min(usize::from(self.page_size))
            });
        Some(size as u8)
    }

    pub(crate) fn accept(
        &mut self,
        mut users: Vec<User>,
        limit: u8,
    ) -> Result<Option<Vec<User>>, InvalidUserPage> {
        let cursor = advance_after(&mut users, usize::from(limit), self.after, |user| user.id)
            .ok_or(InvalidUserPage)?;
        let Some(cursor) = cursor else {
            self.finished = true;
            return Ok(None);
        };
        self.after = Some(cursor);
        if let Some(remaining) = &mut self.remaining {
            *remaining -= users.len();
        }
        Ok(Some(users))
    }
}
