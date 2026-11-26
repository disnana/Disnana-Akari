use std::{future::Future, pin::Pin};

use twilight_model::channel::Message;

pub use twilight_model::gateway::payload::incoming::Ready;

use crate::{
    ButtonContext, ChannelSelectContext, MentionableSelectContext, ModalContext, Result,
    RoleSelectContext, SelectContext, UserSelectContext, interactions::InteractionHandlers,
};

/// Gateway イベントごとに非同期ハンドラーを登録します。
#[derive(Default)]
pub struct Events {
    ready: Vec<Box<dyn Handler<Ready>>>,
    message: Vec<Box<dyn Handler<Message>>>,
    pub(crate) interaction_handlers: InteractionHandlers,
}

impl Events {
    /// 空のイベントハンドラー一覧を作ります。
    pub fn new() -> Self {
        Self::default()
    }

    /// 新しい Gateway セッションが準備できたときに呼ばれる関数を追加します。
    pub fn on_ready(
        &mut self,
        handler: impl AsyncFn(&Ready) -> Result<()> + Send + Sync + 'static,
    ) {
        self.ready.push(Box::new(handler));
    }

    /// サーバーで人が送信した新着メッセージを受け取る関数を追加します。
    pub fn on_message(
        &mut self,
        handler: impl AsyncFn(&Message) -> Result<()> + Send + Sync + 'static,
    ) {
        self.message.push(Box::new(handler));
    }

    /// 指定したカスタム ID のボタンを押したときに呼ぶ関数を登録します。
    ///
    /// # エラー
    ///
    /// ID の長さが 1 文字未満か 100 文字超、または登録済みの場合に失敗します。
    pub fn on_button(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut ButtonContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        self.interaction_handlers
            .register_button(custom_id, handler)
    }

    /// 指定した ID の文字列 Select Menu が選択されたときに呼ぶ関数を登録します。
    ///
    /// # エラー
    /// ID が無効、または同じ Select Menu ID を登録済みの場合に失敗します。
    pub fn on_select(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut SelectContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        self.interaction_handlers
            .register_select(custom_id, handler)
    }

    /// 指定した ID のユーザー選択を処理する関数を登録します。
    ///
    /// # エラー
    /// ID が無効、または同じユーザー Select ID を登録済みの場合に失敗します。
    pub fn on_user_select(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut UserSelectContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        self.interaction_handlers
            .register_user_select(custom_id, handler)
    }

    /// 指定した ID のロール選択を処理する関数を登録します。
    ///
    /// # エラー
    /// ID が無効、または同じロール Select ID を登録済みの場合に失敗します。
    pub fn on_role_select(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut RoleSelectContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        self.interaction_handlers
            .register_role_select(custom_id, handler)
    }

    /// 指定した ID のチャンネル選択を処理する関数を登録します。
    ///
    /// # エラー
    /// ID が無効、または同じチャンネル Select ID を登録済みの場合に失敗します。
    pub fn on_channel_select(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut ChannelSelectContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        self.interaction_handlers
            .register_channel_select(custom_id, handler)
    }

    /// 指定した ID のメニューでユーザーやロールを選んだときに呼ぶ関数を登録します。
    ///
    /// # エラー
    /// ID が無効、または同じ Mentionable Select ID を登録済みの場合に失敗します。
    pub fn on_mentionable_select(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut MentionableSelectContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        self.interaction_handlers
            .register_mentionable_select(custom_id, handler)
    }

    /// 指定した ID の Modal が送信されたときに呼ぶ関数を登録します。
    ///
    /// # エラー
    /// ID が無効、または同じ Modal ID を登録済みの場合に失敗します。
    pub fn on_modal(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut ModalContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        self.interaction_handlers.register_modal(custom_id, handler)
    }

    pub(crate) fn has_message_handlers(&self) -> bool {
        !self.message.is_empty()
    }

    pub(crate) async fn ready(&self, event: &Ready) {
        dispatch(&self.ready, event, "READY").await;
    }

    pub(crate) async fn message(&self, event: &Message) {
        dispatch(&self.message, event, "MESSAGE_CREATE").await;
    }
}

async fn dispatch<T>(handlers: &[Box<dyn Handler<T>>], event: &T, event_name: &str) {
    for handler in handlers {
        if let Err(error) = handler.call(event).await {
            eprintln!("イベント {event_name} の処理に失敗しました ({error})");
        }
    }
}

trait Handler<T>: Send + Sync {
    fn call<'a>(&'a self, event: &'a T) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>>;
}

impl<T, F> Handler<T> for F
where
    F: AsyncFn(&T) -> Result<()> + Send + Sync,
{
    fn call<'a>(&'a self, event: &'a T) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        Box::pin(self(event))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{Handler, dispatch};

    #[tokio::test]
    async fn keeps_registration_order_after_a_handler_error() {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let first = Arc::clone(&observed);
        let second = Arc::clone(&observed);
        let handlers: Vec<Box<dyn Handler<i32>>> = vec![
            Box::new(async move |value: &i32| {
                first.lock().unwrap().push(*value);
                Err("最初のイベントハンドラーのエラー".into())
            }),
            Box::new(async move |value: &i32| {
                second.lock().unwrap().push(*value + 1);
                Ok(())
            }),
        ];
        dispatch(&handlers, &10, "TEST").await;
        assert_eq!(*observed.lock().unwrap(), vec![10, 11]);
    }
}
