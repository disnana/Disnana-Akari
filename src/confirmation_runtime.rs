use std::time::Duration;

use tokio::sync::mpsc;
use twilight_http::Client;
use twilight_model::http::interaction::{InteractionResponse, InteractionResponseType};

use crate::{
    Confirmations, Error, ErrorSource, InteractionEvent, Limits, Pending,
    confirmation::ClaimResult, hooks::Hooks, slash::InitialResponder, work_queue,
};

pub(crate) struct ConfirmationRuntime(pub(crate) Option<Confirmations>);

impl ConfirmationRuntime {
    pub(crate) fn worker<'a>(
        &'a self,
        http: &'a Client,
        hooks: &'a Hooks,
        limits: Limits,
    ) -> (
        Option<mpsc::Sender<Pending<InteractionEvent>>>,
        impl Future<Output = ()> + 'a,
    ) {
        let (sender, receiver) = self
            .0
            .as_ref()
            .map(|_| mpsc::channel(limits.queue_capacity))
            .unzip();
        (sender, async move {
            if let (Some(confirmations), Some(receiver)) = (&self.0, receiver) {
                work_queue::respond(receiver, limits.interaction_workers, |pending| {
                    respond(confirmations, pending, http, hooks)
                })
                .await;
            }
        })
    }

    pub(crate) fn activate(&self) {
        if let Some(confirmations) = &self.0 {
            confirmations.activate();
        }
    }

    pub(crate) fn close(&self) {
        if let Some(confirmations) = &self.0 {
            confirmations.close();
        }
    }
}

impl Drop for ConfirmationRuntime {
    fn drop(&mut self) {
        self.close();
    }
}

async fn respond(
    confirmations: &Confirmations,
    pending: Pending<InteractionEvent>,
    http: &Client,
    hooks: &Hooks,
) {
    let (event, timing) = pending.begin();
    let Some(remaining) = Duration::from_secs(3).checked_sub(timing.queue_wait()) else {
        return;
    };
    let initial = InitialResponder {
        http,
        interaction: &event.interaction,
        timing,
        info: &hooks.info,
        permissions: &hooks.permissions,
    };
    let response = async {
        match confirmations.claim(&event.interaction) {
            ClaimResult::Claimed(claim) => {
                let result = initial
                    .send(&InteractionResponse {
                        kind: InteractionResponseType::DeferredUpdateMessage,
                        data: None,
                    })
                    .await;
                claim.finish(result.is_ok());
                result
            }
            ClaimResult::Unavailable => initial.message("この確認は利用できません。", true).await,
        }
    };
    if !matches!(tokio::time::timeout(remaining, response).await, Ok(Ok(()))) {
        hooks.reporter.report(
            ErrorSource::Response,
            Some("確認セッション"),
            &Error::msg("確認操作への初回応答に失敗しました"),
        );
    }
}
