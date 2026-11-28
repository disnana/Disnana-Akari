use twilight_model::id::Id;

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
