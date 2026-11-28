use std::time::Instant;

use serde::Serialize;
use twilight_http::{request::Request, response::marker::EmptyBody, routing::Route};
use twilight_model::{channel::Message, http::interaction::InteractionResponseType};

use crate::{PollDraft, ResponseStateError, Result, poll::PollMessagePayload};

use super::{InitialResponder, ReplyAction, ResponseState, SlashContext};

impl SlashContext<'_> {
    /// 公開の初回応答に投票を付けます。公開の `defer` 後は待機表示を投票で完成させます。
    ///
    /// # エラー
    /// 非公開の遅延応答、応答済み、または Discord が受け付けない場合に失敗します。
    pub async fn reply_poll(&mut self, poll: &PollDraft<'_>) -> Result<()> {
        self.response_state.reply_poll(self.initial, poll).await
    }
}

impl ResponseState {
    pub(crate) async fn reply_poll(
        &mut self,
        initial: InitialResponder<'_>,
        poll: &PollDraft<'_>,
    ) -> Result<()> {
        if matches!(self, Self::Deferred { ephemeral: true }) {
            return Err(ResponseStateError::PollRequiresPublicResponse.into());
        }
        match self.reply_action(false)? {
            ReplyAction::Initial => initial.send_poll(poll).await?,
            ReplyAction::EditDeferred => {
                let interaction = initial.interaction;
                let request = Request::builder(&Route::UpdateInteractionOriginal {
                    application_id: interaction.application_id.get(),
                    interaction_token: &interaction.token,
                })
                .use_authorization_token(false)
                .json(&poll.message_payload())
                .build()?;
                initial.http.request::<Message>(request).await?;
            }
        }
        *self = Self::Replied;
        Ok(())
    }
}

impl InitialResponder<'_> {
    async fn send_poll(self, poll: &PollDraft<'_>) -> Result<()> {
        let started_at = self.info.measures_timing().then(Instant::now);
        let kind = InteractionResponseType::ChannelMessageWithSource;
        let request = Request::builder(&Route::InteractionCallback {
            interaction_id: self.interaction.id.get(),
            interaction_token: &self.interaction.token,
            with_response: false,
        })
        .use_authorization_token(false)
        .json(&PollCallback {
            kind,
            data: poll.message_payload(),
        })
        .build()?;
        let result = self.http.request::<EmptyBody>(request).await;
        self.report_response(kind, started_at, result)
    }
}

#[derive(Serialize)]
struct PollCallback<'a> {
    #[serde(rename = "type")]
    kind: InteractionResponseType,
    data: PollMessagePayload<'a>,
}

#[cfg(test)]
#[path = "poll_response_tests.rs"]
mod tests;
