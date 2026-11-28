use serde::Serialize;
use twilight_http::{
    Client,
    request::{Request, channel::reaction::RequestReactionType},
    routing::Route,
};
use twilight_model::{
    channel::{Message, message::AllowedMentions},
    id::{
        Id,
        marker::{ChannelMarker, EmojiMarker},
    },
    poll::PollLayoutType,
};

use crate::reaction_emoji::valid_reaction_emoji;

/// 投稿する投票の選択肢です。選択肢の ID は Discord が割り当てます。
#[derive(Clone, Copy, Debug)]
#[must_use]
pub struct PollAnswerInput<'a> {
    text: &'a str,
    emoji: Option<RequestReactionType<'a>>,
}

impl<'a> PollAnswerInput<'a> {
    /// 選択肢の本文を指定します。文字数は投票を作る時に確認します。
    pub const fn new(text: &'a str) -> Self {
        Self { text, emoji: None }
    }

    /// Unicode 絵文字または ID 付きのカスタム絵文字を付けます。
    pub const fn emoji(mut self, emoji: RequestReactionType<'a>) -> Self {
        self.emoji = Some(emoji);
        self
    }
}

/// Discord のチャンネルに投稿する、検証済みの投票です。
#[derive(Clone, Copy, Debug)]
#[must_use]
pub struct PollDraft<'a> {
    question: &'a str,
    answers: &'a [PollAnswerInput<'a>],
    duration: u16,
    allow_multiselect: bool,
}

impl<'a> PollDraft<'a> {
    /// 質問と選択肢から投票を作ります。既定は 24 時間、単一選択です。
    ///
    /// # エラー
    ///
    /// 質問が空か 300 文字を超える、回答数が 2～10 件の範囲外、
    /// または回答が空か 55 文字を超える場合に失敗します。絵文字の形式も確認します。
    pub fn new(question: &'a str, answers: &'a [PollAnswerInput<'a>]) -> Result<Self, PollError> {
        if !valid_text(question, 300) {
            return Err(PollError::Question);
        }
        if !(2..=10).contains(&answers.len()) {
            return Err(PollError::AnswerCount);
        }
        for (index, answer) in answers.iter().enumerate() {
            if !valid_text(answer.text, 55) {
                return Err(PollError::AnswerText { index });
            }
            if answer
                .emoji
                .as_ref()
                .is_some_and(|emoji| !valid_reaction_emoji(emoji))
            {
                return Err(PollError::AnswerEmoji { index });
            }
        }
        Ok(Self {
            question,
            answers,
            duration: 24,
            allow_multiselect: false,
        })
    }

    /// 投票の受付時間を時間単位で指定します。
    ///
    /// # エラー
    ///
    /// 1～768 時間の範囲外では失敗します。
    pub fn duration_hours(mut self, hours: u16) -> Result<Self, PollError> {
        if !(1..=32 * 24).contains(&hours) {
            return Err(PollError::Duration);
        }
        self.duration = hours;
        Ok(self)
    }

    /// ユーザーが複数の選択肢に投票できるようにします。
    pub const fn allow_multiselect(mut self, allow: bool) -> Self {
        self.allow_multiselect = allow;
        self
    }

    /// 投票を投稿し、Discord が割り当てた回答 ID を含むメッセージを返します。
    ///
    /// ギルドでは Bot に投稿権限と `SEND_POLLS` が必要です。コマンドの利用者に求める
    /// 条件はアプリで確認します。この操作はチャンネルへの投稿です。
    ///
    /// # エラー
    ///
    /// REST が失敗した場合、または応答を読み取れない場合にエラーを返します。
    /// 応答の読取失敗時点で Discord 側では投稿済みの場合があります。
    pub async fn send(
        &self,
        http: &Client,
        channel: Id<ChannelMarker>,
    ) -> Result<Message, PollError> {
        let poll = PollPayload {
            question: Question {
                text: self.question,
            },
            answers: self
                .answers
                .iter()
                .map(|answer| AnswerPayload {
                    poll_media: AnswerMedia {
                        text: answer.text,
                        emoji: answer.emoji.as_ref().map(EmojiPayload::from),
                    },
                })
                .collect(),
            duration: self.duration,
            allow_multiselect: self.allow_multiselect,
            layout_type: PollLayoutType::Default,
        };
        // Twilight 0.18 の CreateMessage::poll は応答用の Poll を受け取る。
        // 作成専用の本体を通常の CreateMessage ルートとレート制限で送る。
        let request = Request::builder(&Route::CreateMessage {
            channel_id: channel.get(),
        })
        .json(&CreateMessage {
            poll,
            allowed_mentions: AllowedMentions::default(),
        })
        .build()?;
        let message = http.request::<Message>(request).await?.model().await?;
        if message.channel_id != channel || message.poll.is_none() {
            return Err(PollError::InvalidResponse);
        }
        Ok(message)
    }
}

fn valid_text(text: &str, max: usize) -> bool {
    !crate::validation::exceeds_character_limit(text, max) && !text.trim().is_empty()
}

#[derive(Serialize)]
struct CreateMessage<'a> {
    poll: PollPayload<'a>,
    allowed_mentions: AllowedMentions,
}

#[derive(Serialize)]
struct PollPayload<'a> {
    question: Question<'a>,
    answers: Vec<AnswerPayload<'a>>,
    duration: u16,
    allow_multiselect: bool,
    layout_type: PollLayoutType,
}

#[derive(Serialize)]
struct Question<'a> {
    text: &'a str,
}

#[derive(Serialize)]
struct AnswerPayload<'a> {
    poll_media: AnswerMedia<'a>,
}

#[derive(Serialize)]
struct AnswerMedia<'a> {
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    emoji: Option<EmojiPayload<'a>>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum EmojiPayload<'a> {
    Custom { id: Id<EmojiMarker> },
    Unicode { name: &'a str },
}

impl<'a> From<&RequestReactionType<'a>> for EmojiPayload<'a> {
    fn from(emoji: &RequestReactionType<'a>) -> Self {
        match *emoji {
            RequestReactionType::Custom { id, .. } => Self::Custom { id },
            RequestReactionType::Unicode { name } => Self::Unicode { name },
        }
    }
}

/// 投票の検証または投稿が失敗した理由です。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PollError {
    /// 質問が空か 300 文字を超えています。
    #[error("投票の質問は 1～300 文字で指定してください")]
    Question,
    /// 選択肢が 2～10 件の範囲外です。
    #[error("投票の選択肢は 2～10 件で指定してください")]
    AnswerCount,
    /// 選択肢が空か 55 文字を超えています。
    #[error("投票の選択肢（位置 {index}）は 1～55 文字で指定してください")]
    AnswerText {
        /// 無効な選択肢の位置です。最初の選択肢は 0 です。
        index: usize,
    },
    /// 選択肢の絵文字指定の形式が無効です。
    #[error("投票の選択肢（位置 {index}）の絵文字指定が無効です")]
    AnswerEmoji {
        /// 無効な選択肢の位置です。最初の選択肢は 0 です。
        index: usize,
    },
    /// 受付時間が 1～768 時間の範囲外です。
    #[error("投票の受付時間は 1～768 時間で指定してください")]
    Duration,
    /// Discord の REST リクエストに失敗しました。
    #[error("Discord に投票を投稿できませんでした")]
    Http(#[from] twilight_http::Error),
    /// Discord が返したメッセージの読み取りに失敗しました。
    #[error("Discord が返した投票を読み取れませんでした")]
    HttpBody(#[from] twilight_http::response::DeserializeBodyError),
    /// 応答に投票がないか、投稿先のチャンネルが指定と一致しません。
    #[error("Discord の応答に指定先の投票がありません")]
    InvalidResponse,
}

#[cfg(test)]
mod tests;
