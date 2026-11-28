use crate::{Result, UiSessions, ui_sessions::UI_SESSION_NAMESPACE};

use super::Events;

pub(super) struct UiSessionLifetime(UiSessions);

impl Drop for UiSessionLifetime {
    fn drop(&mut self) {
        self.0.close();
    }
}

impl UiSessions {
    /// ボタン、各セレクト、元メッセージを持つモーダル送信をまとめて登録します。
    ///
    /// Bot の停止時はこの登録に紐づくセッションを終了します。一つの登録先で利用してください。
    ///
    /// # エラー
    ///
    /// 同じ名前空間を使用するハンドラーがあれば失敗し、一件も追加しません。
    pub fn register(&self, events: &mut Events) -> Result<()> {
        events
            .interaction_handlers
            .ensure_ui_session_namespace(UI_SESSION_NAMESPACE)?;
        self.register_button(events)?;
        self.register_select(events)?;
        self.register_user_select(events)?;
        self.register_role_select(events)?;
        self.register_channel_select(events)?;
        self.register_mentionable_select(events)?;
        self.register_modal(events)?;
        events.ui_session_lifetime = Some(UiSessionLifetime(self.clone()));
        Ok(())
    }
}

impl Events {
    pub(crate) fn close_ui_sessions(&self) {
        if let Some(lifetime) = &self.ui_session_lifetime {
            lifetime.0.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Error, HandlerKind, RegistrationError};

    #[test]
    fn scope_registration_checks_every_kind_before_adding_routes() {
        let mut events = Events::new();
        events
            .on_dynamic_modal(UI_SESSION_NAMESPACE, async |_| -> Result<()> { Ok(()) })
            .unwrap();
        let sessions = UiSessions::new(crate::UiSessionLimits::default()).unwrap();
        assert!(matches!(
            sessions.register(&mut events),
            Err(Error::Registration(RegistrationError::Duplicate {
                kind: HandlerKind::Modal,
                ..
            }))
        ));
        events
            .on_dynamic_button(UI_SESSION_NAMESPACE, async |_| -> Result<()> { Ok(()) })
            .unwrap();
        assert!(events.ui_session_lifetime.is_none());
    }
}
