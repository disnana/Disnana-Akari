use std::{
    collections::{HashMap, hash_map::Entry},
    future::Future,
    pin::Pin,
};

use twilight_http::Client;
use twilight_model::{
    application::interaction::{Interaction, InteractionData, InteractionType},
    channel::{
        Message,
        message::{AllowedMentions, component::ComponentType},
    },
    http::interaction::{InteractionResponse, InteractionResponseData, InteractionResponseType},
    id::{
        Id,
        marker::{GuildMarker, UserMarker},
    },
};

use crate::{
    Result, Timing,
    slash::{edit_ui_response, send_followup_message, send_initial_message},
    ui::UiMessage,
};

const BUTTON_FAILED: &str = "ボタンを処理できませんでした。";

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
            response: ButtonResponse::Pending,
        };
        if let Err(error) = handler.call(&mut ctx).await {
            eprintln!("ボタンの処理に失敗しました ({error})");
            let fallback = match ctx.response {
                ButtonResponse::Pending => ctx.reply_ephemeral(BUTTON_FAILED).await,
                ButtonResponse::DeferredUpdate => ctx.followup_ephemeral(BUTTON_FAILED).await,
                ButtonResponse::Sent => Ok(()),
            };
            if fallback.is_err() {
                eprintln!("ボタンのエラー応答に失敗しました");
            }
        } else if ctx.response == ButtonResponse::Pending && ctx.acknowledge().await.is_err() {
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
    response: ButtonResponse,
}

impl<'a> ButtonContext<'a> {
    /// Akari が使用している Twilight の REST クライアントを借ります。
    pub fn http(&self) -> &'a Client {
        self.http
    }

    /// ボタン作成時に設定した ID を返します。
    pub fn custom_id(&self) -> &'a str {
        self.custom_id
    }

    /// ボタンを押したユーザーの ID を返します。
    pub fn author_id(&self) -> Option<Id<UserMarker>> {
        self.interaction.author_id()
    }

    /// ボタンが押されたサーバーの ID を返します。DM では `None` です。
    pub fn guild_id(&self) -> Option<Id<GuildMarker>> {
        self.interaction.guild_id
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
        self.response.require_pending()?;
        send_initial_message(self.http, self.interaction, content, true).await?;
        self.response = ButtonResponse::Sent;
        Ok(())
    }

    /// メッセージを変更せずに操作を受け付けます。[`Self::defer_update`] の別名です。
    ///
    /// # エラー
    ///
    /// 応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn acknowledge(&mut self) -> Result<()> {
        self.defer_update().await
    }

    /// ボタン操作を先に受け付け、元のメッセージを後から更新できるようにします。
    /// 時間のかかる処理を始める前に呼び、その後 [`Self::update_message`] で反映します。
    ///
    /// # エラー
    /// 初回応答済み、または Discord が受け付けなかった場合に失敗します。
    pub async fn defer_update(&mut self) -> Result<()> {
        self.response.require_pending()?;
        let response = InteractionResponse {
            kind: InteractionResponseType::DeferredUpdateMessage,
            data: None,
        };
        self.http
            .interaction(self.interaction.application_id)
            .create_response(self.interaction.id, &self.interaction.token, &response)
            .await?;
        self.response = ButtonResponse::DeferredUpdate;
        Ok(())
    }

    /// ボタンが付いたメッセージの本文、Embed、ボタンを置き換えます。
    /// 省略した本文や Embed、ボタンは取り除かれます。`defer_update` 後にも一度呼べます。
    ///
    /// # エラー
    /// 更新できない応答状態、無効なメッセージ、または Discord が受け付けない場合に失敗します。
    pub async fn update_message(&mut self, message: &UiMessage) -> Result<()> {
        let action = self.response.update_action()?;
        message.validate()?;
        match action {
            UpdateAction::Initial => {
                self.http
                    .interaction(self.interaction.application_id)
                    .create_response(
                        self.interaction.id,
                        &self.interaction.token,
                        &update_response(message),
                    )
                    .await?;
            }
            UpdateAction::Edit => edit_ui_response(self.http, self.interaction, message).await?,
        }
        self.response = ButtonResponse::Sent;
        Ok(())
    }

    /// 操作を受け付けた後、全員に見える追加メッセージを送ります。
    ///
    /// # エラー
    /// 初回応答前、または Discord が受け付けなかった場合に失敗します。
    pub async fn followup(&self, content: &str) -> Result<()> {
        self.send_followup(content, false).await
    }

    /// 操作を受け付けた後、ボタンを押した本人にだけ追加メッセージを送ります。
    ///
    /// # エラー
    /// 初回応答前、または Discord が受け付けなかった場合に失敗します。
    pub async fn followup_ephemeral(&self, content: &str) -> Result<()> {
        self.send_followup(content, true).await
    }

    async fn send_followup(&self, content: &str, ephemeral: bool) -> Result<()> {
        if self.response == ButtonResponse::Pending {
            return Err("追加メッセージを送る前にボタン操作に応答してください".into());
        }
        send_followup_message(self.http, self.interaction, content, ephemeral).await
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonResponse {
    Pending,
    DeferredUpdate,
    Sent,
}

impl ButtonResponse {
    fn require_pending(self) -> Result<()> {
        if self == Self::Pending {
            Ok(())
        } else {
            Err("このボタン操作にはすでに初回応答を送りました".into())
        }
    }

    fn update_action(self) -> Result<UpdateAction> {
        match self {
            Self::Pending => Ok(UpdateAction::Initial),
            Self::DeferredUpdate => Ok(UpdateAction::Edit),
            Self::Sent => Err("このボタン操作では元のメッセージを更新できません".into()),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum UpdateAction {
    Initial,
    Edit,
}

fn update_response(message: &UiMessage) -> InteractionResponse {
    InteractionResponse {
        kind: InteractionResponseType::UpdateMessage,
        data: Some(InteractionResponseData {
            allowed_mentions: Some(AllowedMentions::default()),
            content: Some(message.content.clone().unwrap_or_default()),
            embeds: Some(message.embeds.clone()),
            components: Some(message.components.clone()),
            ..Default::default()
        }),
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
    use super::{
        ButtonContext, ButtonResponse, Buttons, InteractionResponseType, Result, UiMessage,
        UpdateAction, update_response,
    };
    use crate::ui::{Button, Embed};

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

    #[test]
    fn selects_an_update_path_only_before_reply_or_after_deferring() -> Result<()> {
        assert_eq!(
            ButtonResponse::Pending.update_action()?,
            UpdateAction::Initial
        );
        assert_eq!(
            ButtonResponse::DeferredUpdate.update_action()?,
            UpdateAction::Edit
        );
        assert!(ButtonResponse::DeferredUpdate.require_pending().is_err());
        assert!(ButtonResponse::Sent.require_pending().is_err());
        assert!(ButtonResponse::Sent.update_action().is_err());
        Ok(())
    }

    #[test]
    fn update_payload_replaces_all_three_fields_and_suppresses_mentions() {
        let message = UiMessage::new().embed(Embed::new("新しいタイトル"));
        let response = update_response(&message);
        assert_eq!(response.kind, InteractionResponseType::UpdateMessage);
        let data = response.data.unwrap();
        assert_eq!(data.content.as_deref(), Some(""));
        assert_eq!(data.embeds.unwrap().len(), 1);
        assert!(data.components.unwrap().is_empty());
        let mentions = data.allowed_mentions.unwrap();
        assert!(mentions.parse.is_empty());
        assert!(!mentions.replied_user);

        let buttons_only = UiMessage::new().buttons(vec![Button::primary("refresh", "更新")]);
        let data = update_response(&buttons_only).data.unwrap();
        assert!(data.embeds.unwrap().is_empty());
        assert_eq!(data.components.unwrap().len(), 1);
    }
}
