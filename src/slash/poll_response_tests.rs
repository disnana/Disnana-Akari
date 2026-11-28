use std::time::Duration;

use serde_json::{Value, json};
use tokio::{net::TcpListener, time::timeout};
use twilight_http::Client;
use twilight_model::application::interaction::InteractionData;

use crate::{
    Context, Error, PollAnswerInput,
    hooks::Hooks,
    slash::tests::{command_interaction, exchange_response, test_command_message},
    timing::Pending,
};

use super::*;

const CALLBACK: &str = "/interactions/1/token/callback";
const ORIGINAL: &str = "/webhooks/10/token/messages/@original";

fn poll_json() -> Value {
    json!({
        "allowed_mentions": {"parse":[]},
        "poll":{
            "question":{"text":"次の予定"},
            "answers":[{"poll_media":{"text":"土曜日"}},{"poll_media":{"text":"日曜日"}}],
            "duration":24,"allow_multiselect":false,"layout_type":1
        }
    })
}

async fn read(
    listener: &TcpListener,
    method: &str,
    route: &str,
    response: Option<&str>,
) -> Result<Value> {
    let body = exchange_response(listener, method, Some(route), response)
        .await
        .map_err(Error::handler)?;
    serde_json::from_str(&body).map_err(Error::handler)
}

async fn serve_poll_responses(listener: &TcpListener) -> Result<()> {
    let response = json!({
        "id":"9","channel_id":"2","type":0,
        "author":{"id":"6","username":"bot","discriminator":"0"},
        "content":"","timestamp":"2026-01-01T00:00:00+00:00",
        "attachments":[],"embeds":[],"mention_everyone":false,"mention_roles":[],
        "mentions":[],"pinned":false,"tts":false,
        "poll":{
            "question":{"text":"次の予定"},
            "answers":[{"answer_id":1,"poll_media":{"text":"土曜日"}},{"answer_id":2,"poll_media":{"text":"日曜日"}}],
            "expiry":"2026-01-02T00:00:00+00:00","allow_multiselect":false,"layout_type":1
        }
    }).to_string();
    let mut expected = poll_json();
    expected["message_reference"] =
        json!({"type":0,"message_id":"1","channel_id":"2","fail_if_not_exists":true});
    assert_eq!(
        read(listener, "POST", "/channels/2/messages", None).await?,
        expected
    );
    let callback = read(listener, "POST", CALLBACK, None).await?;
    assert_eq!(callback, json!({"type":4,"data":poll_json()}));
    assert_eq!(
        read(listener, "POST", "/webhooks/10/token", Some(&response)).await?,
        poll_json()
    );
    let deferred = read(listener, "POST", CALLBACK, None).await?;
    assert_eq!(deferred["type"], 5);
    assert_eq!(
        read(listener, "PATCH", ORIGINAL, Some(&response)).await?,
        poll_json()
    );
    let deferred = read(listener, "POST", CALLBACK, None).await?;
    assert_eq!(deferred["type"], 5);
    assert_eq!(deferred["data"]["flags"], 64);
    let private = read(listener, "PATCH", ORIGINAL, None).await?;
    assert_eq!(private["content"], "非公開");
    assert!(private.get("poll").is_none());
    Ok(())
}

#[tokio::test]
async fn replies_to_text_and_finishes_public_interactions_with_poll_requests() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(Error::handler)?;
    let http = Client::builder()
        .proxy(
            listener.local_addr().map_err(Error::handler)?.to_string(),
            true,
        )
        .build();
    let hooks = Hooks::default();
    let mut interaction = command_interaction(None, None, 5);
    interaction.token = "token".into();
    let Some(InteractionData::ApplicationCommand(data)) = &interaction.data else {
        return Err(Error::msg("試験用コマンドデータがありません"));
    };
    let (_, timing) = Pending::new((), None).begin();
    let initial = InitialResponder {
        http: &http,
        interaction: &interaction,
        timing,
        info: &hooks.info,
        permissions: &hooks.permissions,
    };
    let message = test_command_message("!poll")?;
    let answers = [
        PollAnswerInput::new("土曜日"),
        PollAnswerInput::new("日曜日"),
    ];
    let draft = PollDraft::new("次の予定", &answers).map_err(Error::handler)?;
    let request = async {
        Context {
            http: &http,
            message: &message,
            timing,
            permissions: &hooks.permissions,
        }
        .reply_poll(&draft)
        .await?;
        let mut state = ResponseState::Pending;
        let mut ctx = SlashContext {
            initial,
            data,
            response_state: &mut state,
        };
        ctx.reply_poll(&draft).await?;
        assert!(matches!(
            ctx.reply_poll(&draft).await,
            Err(Error::ResponseState(ResponseStateError::AlreadyResponded))
        ));
        let followup = ctx.followups()?.send_poll(&draft).await?;
        assert_eq!(followup.id().get(), 9);

        let mut state = ResponseState::Pending;
        let mut ctx = SlashContext {
            initial,
            data,
            response_state: &mut state,
        };
        ctx.defer().await?;
        ctx.reply_poll(&draft).await?;
        assert_eq!(state, ResponseState::Replied);

        let mut state = ResponseState::Pending;
        let mut ctx = SlashContext {
            initial,
            data,
            response_state: &mut state,
        };
        ctx.defer_ephemeral().await?;
        assert!(matches!(
            ctx.reply_poll(&draft).await,
            Err(Error::ResponseState(
                ResponseStateError::PollRequiresPublicResponse
            ))
        ));
        ctx.reply_ephemeral("非公開").await?;
        assert_eq!(state, ResponseState::Replied);
        Ok::<(), Error>(())
    };
    let ((), ()) = timeout(Duration::from_secs(8), async {
        tokio::try_join!(serve_poll_responses(&listener), request)
    })
    .await
    .map_err(Error::handler)??;
    Ok(())
}
