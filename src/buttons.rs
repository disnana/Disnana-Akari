use std::{
    collections::{HashMap, hash_map::Entry},
    future::Future,
    pin::Pin,
};

use twilight_http::Client;
use twilight_model::{
    application::interaction::{Interaction, InteractionData, InteractionType},
    channel::{Message, message::component::ComponentType},
    http::interaction::{InteractionResponse, InteractionResponseType},
    id::{Id, marker::UserMarker},
};

use crate::{Result, Timing, slash::send_initial_message};

#[derive(Default)]
pub(crate) struct Buttons {
    handlers: HashMap<String, Box<dyn ButtonHandler>>,
}

impl Buttons {
    pub(crate) fn register(
        &mut self,
        custom_id: &str,
        handler: impl AsyncFn(&mut ButtonContext<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<()> {
        if !(1..=100).contains(&custom_id.chars().count()) {
            return Err("ボタン ID は 1 文字以上 100 文字以下で指定してください".into());
        }
        match self.handlers.entry(custom_id.into()) {
            Entry::Vacant(entry) => {
                entry.insert(Box::new(handler));
                Ok(())
            }
            Entry::Occupied(_) => Err(format!("ボタン ID `{custom_id}` は登録済みです").into()),
        }
    }

    pub(crate) fn has_handlers(&self) -> bool {
        !self.handlers.is_empty()
    }

    pub(crate) fn accepts(&self, interaction: &Interaction) -> bool {
        button_id(interaction).is_some_and(|custom_id| self.handlers.contains_key(custom_id))
    }

    pub(crate) async fn dispatch(&self, http: &Client, interaction: &Interaction, timing: Timing) {
        let Some(custom_id) = button_id(interaction) else {
            return;
        };
        let Some(handler) = self.handlers.get(custom_id) else {
            return;
        };
        let mut ctx = ButtonContext {
            http,
            interaction,
            custom_id,
            timing,
            acknowledged: false,
        };
        if let Err(error) = handler.call(&mut ctx).await {
            eprintln!("ボタンの処理に失敗しました ({error})");
            if !ctx.acknowledged
                && ctx
                    .reply_ephemeral("ボタンを処理できませんでした。")
                    .await
                    .is_err()
            {
                eprintln!("ボタンのエラー応答に失敗しました");
            }
        } else if !ctx.acknowledged && ctx.acknowledge().await.is_err() {
            eprintln!("ボタンの確認応答に失敗しました");
        }
    }
}

fn button_id(interaction: &Interaction) -> Option<&str> {
    if interaction.kind != InteractionType::MessageComponent {
        return None;
    }
    let Some(InteractionData::MessageComponent(data)) = &interaction.data else {
        return None;
    };
    (data.component_type == ComponentType::Button).then_some(data.custom_id.as_str())
}

/// ボタンが一回押されたときにハンドラーへ渡す情報です。
pub struct ButtonContext<'a> {
    http: &'a Client,
    interaction: &'a Interaction,
    custom_id: &'a str,
    timing: Timing,
    acknowledged: bool,
}

impl<'a> ButtonContext<'a> {
    /// ボタン作成時に設定した ID を返します。
    pub fn custom_id(&self) -> &'a str {
        self.custom_id
    }

    /// ボタンを押したユーザーの ID を返します。
    pub fn author_id(&self) -> Option<Id<UserMarker>> {
        self.interaction.author_id()
    }

    /// ボタンが付いた Discord メッセージを借ります。
    pub fn message(&self) -> Option<&'a Message> {
        self.interaction.message.as_ref()
    }

    /// Gateway の往復時間と、ボタン操作を処理するまでの時間を返します。
    pub fn timing(&self) -> Timing {
        self.timing
    }

    /// ボタンを押した本人にだけ表示する初回応答を送ります。
    ///
    /// # エラー
    ///
    /// 応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn reply_ephemeral(&mut self, content: &str) -> Result<()> {
        if self.acknowledged {
            return Err("このボタン操作にはすでに応答しました".into());
        }
        send_initial_message(self.http, self.interaction, content, true).await?;
        self.acknowledged = true;
        Ok(())
    }

    /// メッセージを追加せずに操作を受け付けたことを Discord に伝えます。
    ///
    /// # エラー
    ///
    /// 応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn acknowledge(&mut self) -> Result<()> {
        if self.acknowledged {
            return Err("このボタン操作にはすでに応答しました".into());
        }
        let response = InteractionResponse {
            kind: InteractionResponseType::DeferredUpdateMessage,
            data: None,
        };
        self.http
            .interaction(self.interaction.application_id)
            .create_response(self.interaction.id, &self.interaction.token, &response)
            .await?;
        self.acknowledged = true;
        Ok(())
    }
}

trait ButtonHandler: Send + Sync {
    fn call<'a, 'ctx>(
        &'a self,
        ctx: &'a mut ButtonContext<'ctx>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>>;
}

impl<F> ButtonHandler for F
where
    F: AsyncFn(&mut ButtonContext<'_>) -> Result<()> + Send + Sync,
{
    fn call<'a, 'ctx>(
        &'a self,
        ctx: &'a mut ButtonContext<'ctx>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        Box::pin(self(ctx))
    }
}

#[cfg(test)]
mod tests {
    use super::{ButtonContext, Buttons, Result};

    async fn click(_: &mut ButtonContext<'_>) -> Result<()> {
        Ok(())
    }

    #[test]
    fn rejects_duplicate_and_invalid_button_ids() -> Result<()> {
        let mut buttons = Buttons::default();
        assert!(!buttons.has_handlers());
        assert!(buttons.register("", click).is_err());
        assert!(buttons.register(&"a".repeat(101), click).is_err());
        buttons.register("akari:ui:ping", click)?;
        assert!(buttons.has_handlers());
        assert!(buttons.register("akari:ui:ping", click).is_err());
        Ok(())
    }
}
