use std::collections::{HashMap, hash_map::Entry};

use crate::{
    HandlerKind, RegistrationError, Result,
    ui::{DynamicIdRef, invalid_dynamic_namespace, valid_dynamic_namespace},
};

/// 固定 ID と名前空間の表はそれぞれハンドラーの登録件数にのみ比例します。
pub(super) struct Routes<T: ?Sized> {
    exact: HashMap<String, Box<T>>,
    dynamic: HashMap<String, Box<T>>,
}

impl<T: ?Sized> Default for Routes<T> {
    fn default() -> Self {
        Self {
            exact: HashMap::new(),
            dynamic: HashMap::new(),
        }
    }
}

impl<T: ?Sized> Routes<T> {
    pub(super) fn len(&self) -> usize {
        self.exact.len() + self.dynamic.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.exact.is_empty() && self.dynamic.is_empty()
    }

    pub(super) fn register_exact(
        &mut self,
        kind: HandlerKind,
        id: &str,
        handler: Box<T>,
    ) -> Result<()> {
        if !(1..=100).contains(&id.chars().count()) {
            return Err(RegistrationError::InvalidCustomId { kind }.into());
        }
        register(&mut self.exact, kind, id, handler)
    }

    pub(super) fn register_dynamic(
        &mut self,
        kind: HandlerKind,
        namespace: &str,
        handler: Box<T>,
    ) -> Result<()> {
        if !valid_dynamic_namespace(namespace) {
            return Err(invalid_dynamic_namespace());
        }
        register(&mut self.dynamic, kind, namespace, handler)
    }

    pub(super) fn resolve<'a>(&self, id: &'a str) -> Option<(&T, Option<DynamicIdRef<'a>>)> {
        if let Some(handler) = self.exact.get(id) {
            return Some((handler.as_ref(), None));
        }
        if self.dynamic.is_empty() {
            return None;
        }
        let dynamic_id = DynamicIdRef::parse(id)?;
        let handler = self.dynamic.get(dynamic_id.namespace())?;
        Some((handler.as_ref(), Some(dynamic_id)))
    }
}

fn register<T: ?Sized>(
    handlers: &mut HashMap<String, Box<T>>,
    kind: HandlerKind,
    key: &str,
    handler: Box<T>,
) -> Result<()> {
    match handlers.entry(key.to_owned()) {
        Entry::Vacant(entry) => {
            entry.insert(handler);
            Ok(())
        }
        Entry::Occupied(_) => Err(RegistrationError::Duplicate {
            kind,
            name: key.to_owned(),
        }
        .into()),
    }
}

macro_rules! register_dynamic {
    ($method:ident, $field:ident, $kind:ident, $context:ident) => {
        pub(crate) fn $method(
            &mut self,
            namespace: &str,
            handler: impl AsyncFn(&mut super::$context<'_>) -> Result<()> + Send + Sync + 'static,
        ) -> Result<()> {
            self.$field
                .register_dynamic(HandlerKind::$kind, namespace, Box::new(handler))
        }
    };
}

impl super::InteractionHandlers {
    register_dynamic!(register_dynamic_button, buttons, Button, ButtonContext);
    register_dynamic!(register_dynamic_select, selects, Select, SelectContext);
    register_dynamic!(
        register_dynamic_user_select,
        user_selects,
        UserSelect,
        UserSelectContext
    );
    register_dynamic!(
        register_dynamic_role_select,
        role_selects,
        RoleSelect,
        RoleSelectContext
    );
    register_dynamic!(
        register_dynamic_channel_select,
        channel_selects,
        ChannelSelect,
        ChannelSelectContext
    );
    register_dynamic!(
        register_dynamic_mentionable_select,
        mentionable_selects,
        MentionableSelect,
        MentionableSelectContext
    );
    register_dynamic!(register_dynamic_modal, modals, Modal, ModalContext);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_has_priority_and_never_exposes_dynamic_value() {
        let mut routes = Routes::default();
        routes
            .register_dynamic(HandlerKind::Button, "ticket", Box::new(2))
            .unwrap();
        routes
            .register_exact(HandlerKind::Button, "ticket:9", Box::new(1))
            .unwrap();
        assert_eq!(routes.resolve("ticket:9"), Some((&1, None)));
        let (handler, id) = routes.resolve("ticket:10").unwrap();
        assert_eq!(*handler, 2);
        assert_eq!(id.unwrap().value(), "10");
        assert!(routes.resolve("ticket:").is_none());
        assert!(routes.resolve("ticket-other:10").is_none());
        assert!(routes.resolve("ticket:line\n").is_none());
        assert!(
            routes
                .register_dynamic(HandlerKind::Button, "ticket", Box::new(3))
                .is_err()
        );
        assert_eq!(*routes.resolve("ticket:10").unwrap().0, 2);
        assert!(
            routes
                .register_dynamic(HandlerKind::Button, "bad:namespace", Box::new(3))
                .is_err()
        );
    }
}
