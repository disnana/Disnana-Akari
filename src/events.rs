use std::{future::Future, pin::Pin};

use twilight_model::channel::Message;

pub use twilight_model::gateway::payload::incoming::Ready;

use crate::{
    ButtonContext, ChannelSelectContext, ErrorSource, MentionableSelectContext, ModalContext,
    Result, RoleSelectContext, SelectContext, UserSelectContext, error::adapt_handler,
    hooks::Reporter, interactions::InteractionHandlers,
};

/// Gateway イベントごとに非同期ハンドラーを登録します。
#[derive(Default)]
pub struct Events {
    ready: Vec<Box<dyn Handler<Ready>>>,
    message: Vec<Box<dyn Handler<Message>>>,
    pub(crate) interaction_handlers: InteractionHandlers,
}

impl std::fmt::Debug for Events {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Events")
            .field("ready_handlers", &self.ready.len())
            .field("message_handlers", &self.message.len())
            .field("interaction_handlers", &self.interaction_handlers)
            .finish()
    }
}

impl Events {
    /// 空のイベントハンドラー一覧を作ります。
    pub fn new() -> Self {
        Self::default()
    }

    /// 新しい Gateway セッションが準備できたときに呼ばれる関数を追加します。
    pub fn on_ready<E>(
        &mut self,
        handler: impl AsyncFn(&Ready) -> std::result::Result<(), E> + Send + Sync + 'static,
    ) where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.ready
            .push(Box::new(adapt_handler!(handler, event: &Ready)));
    }

    /// サーバーで人が送信した新着メッセージを受け取る関数を追加します。
    pub fn on_message<E>(
        &mut self,
        handler: impl AsyncFn(&Message) -> std::result::Result<(), E> + Send + Sync + 'static,
    ) where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.message
            .push(Box::new(adapt_handler!(handler, event: &Message)));
    }

    /// 指定したカスタム ID のボタンを押したときに呼ぶ関数を登録します。
    ///
    /// # エラー
    ///
    /// ID の長さが 1 文字未満か 100 文字超、または登録済みの場合に失敗します。
    pub fn on_button<E>(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut ButtonContext<'_>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.interaction_handlers.register_button(
            custom_id,
            adapt_handler!(handler, ctx: &mut ButtonContext<'_>),
        )
    }

    /// 指定した ID の文字列 Select Menu が選択されたときに呼ぶ関数を登録します。
    ///
    /// # エラー
    /// ID が無効、または同じ Select Menu ID を登録済みの場合に失敗します。
    pub fn on_select<E>(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut SelectContext<'_>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.interaction_handlers.register_select(
            custom_id,
            adapt_handler!(handler, ctx: &mut SelectContext<'_>),
        )
    }

    /// 指定した ID のユーザー選択を処理する関数を登録します。
    ///
    /// # エラー
    /// ID が無効、または同じユーザー Select ID を登録済みの場合に失敗します。
    pub fn on_user_select<E>(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut UserSelectContext<'_>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.interaction_handlers.register_user_select(
            custom_id,
            adapt_handler!(handler, ctx: &mut UserSelectContext<'_>),
        )
    }

    /// 指定した ID のロール選択を処理する関数を登録します。
    ///
    /// # エラー
    /// ID が無効、または同じロール Select ID を登録済みの場合に失敗します。
    pub fn on_role_select<E>(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut RoleSelectContext<'_>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.interaction_handlers.register_role_select(
            custom_id,
            adapt_handler!(handler, ctx: &mut RoleSelectContext<'_>),
        )
    }

    /// 指定した ID のチャンネル選択を処理する関数を登録します。
    ///
    /// # エラー
    /// ID が無効、または同じチャンネル Select ID を登録済みの場合に失敗します。
    pub fn on_channel_select<E>(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut ChannelSelectContext<'_>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.interaction_handlers.register_channel_select(
            custom_id,
            adapt_handler!(handler, ctx: &mut ChannelSelectContext<'_>),
        )
    }

    /// 指定した ID のメニューでユーザーやロールを選んだときに呼ぶ関数を登録します。
    ///
    /// # エラー
    /// ID が無効、または同じ Mentionable Select ID を登録済みの場合に失敗します。
    pub fn on_mentionable_select<E>(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut MentionableSelectContext<'_>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.interaction_handlers.register_mentionable_select(
            custom_id,
            adapt_handler!(handler, ctx: &mut MentionableSelectContext<'_>),
        )
    }

    /// 指定した ID の Modal が送信されたときに呼ぶ関数を登録します。
    ///
    /// # エラー
    /// ID が無効、または同じ Modal ID を登録済みの場合に失敗します。
    pub fn on_modal<E>(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut ModalContext<'_>) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.interaction_handlers.register_modal(
            custom_id,
            adapt_handler!(handler, ctx: &mut ModalContext<'_>),
        )
    }

    pub(crate) fn has_message_handlers(&self) -> bool {
        !self.message.is_empty()
    }

    pub(crate) async fn ready(&self, event: &Ready, reporter: &Reporter) {
        dispatch(&self.ready, event, "READY", reporter).await;
    }

    pub(crate) async fn message(&self, event: &Message, reporter: &Reporter) {
        dispatch(&self.message, event, "MESSAGE_CREATE", reporter).await;
    }
}

async fn dispatch<T>(
    handlers: &[Box<dyn Handler<T>>],
    event: &T,
    event_name: &str,
    reporter: &Reporter,
) {
    for handler in handlers {
        if let Err(error) = handler.call(event).await {
            reporter.report(ErrorSource::Event, Some(event_name), &error);
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
    use crate::hooks::Reporter;

    #[tokio::test]
    async fn keeps_registration_order_after_a_handler_error() {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let first = Arc::clone(&observed);
        let second = Arc::clone(&observed);
        let handlers: Vec<Box<dyn Handler<i32>>> = vec![
            Box::new(async move |value: &i32| {
                first.lock().unwrap().push(*value);
                Err(crate::Error::Handler(
                    "最初のイベントハンドラーのエラー".into(),
                ))
            }),
            Box::new(async move |value: &i32| {
                second.lock().unwrap().push(*value + 1);
                Ok(())
            }),
        ];
        dispatch(&handlers, &10, "TEST", &Reporter::default()).await;
        assert_eq!(*observed.lock().unwrap(), vec![10, 11]);
    }
}
