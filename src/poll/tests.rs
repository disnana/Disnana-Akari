use std::time::Duration;

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::timeout,
};

use super::*;

#[test]
fn accepts_documented_limits_and_rejects_invalid_drafts() {
    let question = "質問".repeat(150);
    let answer = "猫".repeat(55);
    let answers = [PollAnswerInput::new(&answer); 10];
    let poll = PollDraft::new(&question, &answers).unwrap();
    assert_eq!(poll.duration, 24);
    assert!(!poll.allow_multiselect);
    assert!(poll.duration_hours(1).is_ok());
    assert!(poll.duration_hours(768).is_ok());
    for hours in [0, 769] {
        assert!(matches!(
            poll.duration_hours(hours),
            Err(PollError::Duration)
        ));
    }
    for question in ["　".to_owned(), question + "猫"] {
        assert!(matches!(
            PollDraft::new(&question, &answers),
            Err(PollError::Question)
        ));
    }
    for count in [0, 1, 11] {
        let answers = vec![PollAnswerInput::new("A"); count];
        assert!(matches!(
            PollDraft::new("Q", &answers),
            Err(PollError::AnswerCount)
        ));
    }
    for answer in [" ".to_owned(), answer + "猫"] {
        let answers = [PollAnswerInput::new("A"), PollAnswerInput::new(&answer)];
        assert!(matches!(
            PollDraft::new("Q", &answers),
            Err(PollError::AnswerText { index: 1 })
        ));
    }
    let bad_emoji = RequestReactionType::Unicode { name: " " };
    let answers = [
        PollAnswerInput::new("A").emoji(bad_emoji),
        PollAnswerInput::new("B"),
    ];
    assert!(matches!(
        PollDraft::new("Q", &answers),
        Err(PollError::AnswerEmoji { index: 0 })
    ));
}

async fn respond(listener: &TcpListener, response: &Value) {
    let (stream, _) = listener.accept().await.unwrap();
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    stream.read_line(&mut line).await.unwrap();
    assert_eq!(
        line.trim_end(),
        "POST /api/v10/channels/4/messages HTTP/1.1"
    );
    let mut length = None;
    let mut authenticated = false;
    loop {
        line.clear();
        assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
        if line == "\r\n" {
            break;
        }
        let (header, value) = line.trim_end().split_once(':').unwrap();
        if header.eq_ignore_ascii_case("content-length") {
            length = Some(value.trim().parse().unwrap());
        } else if header.eq_ignore_ascii_case("authorization") {
            assert_eq!(value.trim(), "Bot local-test");
            authenticated = true;
        }
    }
    assert!(authenticated);
    let mut request = vec![0; length.unwrap()];
    stream.read_exact(&mut request).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&request).unwrap(),
        json!({
            "allowed_mentions":{"parse":[]},
            "poll":{
                "question":{"text":"どれ？"},
                "answers":[
                    {"poll_media":{"text":"猫","emoji":{"name":"🐈"}}},
                    {"poll_media":{"text":"犬","emoji":{"id":"42"}}}
                ],
                "duration":48, "allow_multiselect":true, "layout_type":1
            }
        })
    );
    let body = response.to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .get_mut()
        .write_all(response.as_bytes())
        .await
        .unwrap();
}

#[tokio::test]
async fn sends_creation_fields_and_uses_discord_answer_ids() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = Client::builder()
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .token("local-test".into())
        .build();
    let answers = [
        PollAnswerInput::new("猫").emoji(crate::parse_reaction_emoji("🐈").unwrap()),
        PollAnswerInput::new("犬").emoji(crate::parse_reaction_emoji("<a:dog:42>").unwrap()),
    ];
    let draft = PollDraft::new("どれ？", &answers)
        .unwrap()
        .duration_hours(48)
        .unwrap()
        .allow_multiselect(true);
    let mut response = json!({
        "id":"5", "channel_id":"4", "type":0,
        "author":{"id":"6", "username":"bot", "discriminator":"0"},
        "content":"", "timestamp":"2026-01-01T00:00:00+00:00",
        "attachments":[], "embeds":[], "mention_everyone":false, "mention_roles":[],
        "mentions":[], "pinned":false, "tts":false,
        "poll":{
            "question":{"text":"どれ？"},
            "answers":[
                {"answer_id":7,"poll_media":{"text":"猫","emoji":{"name":"🐈"}}},
                {"answer_id":9,"poll_media":{"text":"犬","emoji":{"id":"42"}}}
            ],
            "expiry":"2026-01-03T00:00:00+00:00", "allow_multiselect":true, "layout_type":1
        }
    });
    timeout(Duration::from_secs(5), async {
        let ((), sent) = tokio::join!(respond(&listener, &response), draft.send(&http, Id::new(4)));
        let answers = sent.unwrap().poll.unwrap().answers;
        assert_eq!(
            answers
                .iter()
                .map(|answer| answer.answer_id)
                .collect::<Vec<_>>(),
            [7, 9]
        );

        response["poll"] = Value::Null;
        let ((), sent) = tokio::join!(respond(&listener, &response), draft.send(&http, Id::new(4)));
        assert!(matches!(sent, Err(PollError::InvalidResponse)));
    })
    .await
    .unwrap();
}
