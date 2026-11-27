use twilight_gateway::{Intents, error::ReceiveMessageError};
use twilight_http::{error::Error as HttpError, response::DeserializeBodyError};
use twilight_validate::{
    command::CommandValidationError, component::ComponentValidationError,
    embed::EmbedValidationError, message::MessageValidationError,
};

/// Akari の処理や、利用者が登録したハンドラーから返るエラーです。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// `Bot::new` に空白だけ、または空のトークンが渡されました。
    #[error("Discord Bot トークンが空です")]
    EmptyToken,
    /// `DISCORD_TOKEN` を読み取れなかったか、空でした。
    #[error("DISCORD_TOKEN に Discord Bot トークンを設定してください")]
    InvalidTokenEnvironment,
    /// メッセージを受信するために必要な Intent が不足しています。
    #[error(
        "テキストコマンドやメッセージのハンドラーには、Intent {0:?} が必要です。\n\
         `Bot::add_intents(Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT)` で追加してください。\n\
         MESSAGE_CONTENT は特権 Intent なので、Developer Portal の Bot ページでも有効にします。"
    )]
    MissingIntents(Intents),
    /// コマンドや Interaction ハンドラーの登録内容が無効です。
    #[error("{0}")]
    InvalidRegistration(String),
    /// Discord が返したスラッシュコマンドの登録結果が要求と一致しません。
    #[error("{0}")]
    InvalidCommandSync(String),
    /// 指定のスラッシュコマンドを Discord に登録できませんでした。
    #[error("スラッシュコマンド `/{name}` を登録できませんでした")]
    CommandRegistration {
        /// 登録しようとしたコマンド名です。
        name: String,
        /// Discord へのリクエストで起きたエラーです。
        #[source]
        source: HttpError,
    },
    /// Akari に登録されていないスラッシュコマンドが実行されました。
    #[error("Akari に登録されていないスラッシュコマンドが実行されました")]
    CommandUnavailable,
    /// メッセージやフォームの構成が無効です。
    #[error("{0}")]
    InvalidUi(&'static str),
    /// 現在の Interaction の応答状態では、指定された操作ができません。
    #[error("{0}")]
    InvalidResponseState(&'static str),
    /// Discord が送った選択内容を読み取れませんでした。
    #[error("{0}")]
    InvalidSelection(&'static str),
    /// コマンドを実行したユーザーを取得できませんでした。
    #[error("実行したユーザーを確認できませんでした。")]
    MissingAuthor,
    /// 同時処理枠か待ち行列がいっぱいです。
    #[error("{0}")]
    QueueFull(&'static str),
    /// Gateway が終了し、Bot の処理が止まりました。
    #[error("Discord Gateway の接続が終了しました")]
    GatewayEnded,
    /// Gateway から接続終了の通知を受け取りました。
    #[error("{}", describe_gateway_close(*.0))]
    GatewayClose(u16),
    /// Gateway のイベントを受信できませんでした。
    #[error(transparent)]
    GatewayReceive(#[from] ReceiveMessageError),
    /// Discord REST API のリクエストが失敗しました。
    #[error(transparent)]
    Http(#[from] HttpError),
    /// Discord REST API のレスポンス本文を読み取れませんでした。
    #[error(transparent)]
    HttpBody(#[from] DeserializeBodyError),
    /// Twilight がコマンドの設定を無効と判定しました。
    #[error(transparent)]
    CommandValidation(#[from] CommandValidationError),
    /// Twilight がメッセージを無効と判定しました。
    #[error(transparent)]
    MessageValidation(#[from] MessageValidationError),
    /// Twilight が埋め込みを無効と判定しました。
    #[error(transparent)]
    EmbedValidation(#[from] EmbedValidationError),
    /// Twilight が操作部品を無効と判定しました。
    #[error(transparent)]
    ComponentValidation(#[from] ComponentValidationError),
    /// 利用者が登録したハンドラーから返された、アプリ固有のエラーです。
    #[error(transparent)]
    Handler(#[from] HandlerError),
}

impl Error {
    /// アプリ固有のエラーを包みます。`map_err(Error::handler)?` の形で使えます。
    pub fn handler(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Handler(HandlerError(Box::new(error)))
    }

    /// メッセージだけのアプリ固有のエラーを作ります。`return Err(Error::msg("使い方: !echo 文章"))` の形で使えます。
    pub fn msg(message: impl std::fmt::Display) -> Self {
        Self::Handler(HandlerError(message.to_string().into()))
    }
}

impl From<Box<dyn std::error::Error + Send + Sync>> for Error {
    fn from(error: Box<dyn std::error::Error + Send + Sync>) -> Self {
        handler_error(error)
    }
}

/// ハンドラーが返した、アプリ固有のエラーです。[`Error::Handler`] に入っています。
///
/// 表示と原因の連鎖は、元のエラーと同じです（メッセージが二重に表示されません）。
/// 元のエラーは [`Self::inner`]、[`Self::downcast_ref`]、[`Self::into_inner`] で取り出せます。
pub struct HandlerError(Box<dyn std::error::Error + Send + Sync>);

impl HandlerError {
    /// 元のエラーを借ります。
    pub fn inner(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
        &*self.0
    }

    /// 元のエラーを取り出します。
    pub fn into_inner(self) -> Box<dyn std::error::Error + Send + Sync> {
        self.0
    }

    /// 元のエラーが `T` なら、借りて返します。
    pub fn downcast_ref<T: std::error::Error + 'static>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }
}

impl std::fmt::Display for HandlerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl std::fmt::Debug for HandlerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.0, f)
    }
}

impl std::error::Error for HandlerError {
    /// 元のエラーの原因を返します。元のエラー自身は、表示に含まれています。
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

/// ハンドラーが返したエラーを、Akari の `Error` にする。
///
/// すでに Akari の `Error` なら、包み直さずにそのまま返す。Twilight の REST のエラーは、
/// 対応するバリアントにする。それ以外は `Error::Handler` で包む。
pub(crate) fn handler_error<E>(error: E) -> Error
where
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let boxed: Box<dyn std::error::Error + Send + Sync> = error.into();
    let boxed = match boxed.downcast::<Error>() {
        Ok(error) => return *error,
        Err(boxed) => boxed,
    };
    let boxed = match boxed.downcast::<HttpError>() {
        Ok(error) => return Error::Http(*error),
        Err(boxed) => boxed,
    };
    match boxed.downcast::<DeserializeBodyError>() {
        Ok(error) => Error::HttpBody(*error),
        Err(boxed) => Error::Handler(HandlerError(boxed)),
    }
}

/// 利用者のハンドラーを、Akari の `Error` を返す関数に包む。
///
/// 登録する関数は、`Result<(), E>`（`E` は `Into<Box<dyn Error + Send + Sync>>`）を返せる。
/// 内部では、すべて `Result<(), Error>` に揃える。
macro_rules! adapt_handler {
    ($handler:ident, $arg:ident: $ty:ty) => {
        async move |$arg: $ty| -> $crate::Result<()> {
            $handler($arg).await.map_err($crate::error::handler_error)
        }
    };
}
pub(crate) use adapt_handler;

/// Gateway が接続を閉じたときの理由を、利用者に分かる形にします。
fn describe_gateway_close(code: u16) -> String {
    let hint = match code {
        4004 => Some(
            "Bot トークンが正しくありません。Developer Portal でトークンを確認し、再発行した場合は環境変数も更新してください。",
        ),
        4013 => Some("要求した Intent の指定が正しくありません。"),
        4014 => Some(
            "許可されていない Intent が要求されました。Developer Portal の Bot ページで、Message Content Intent などの特権 Intent を有効にしてください。",
        ),
        _ => None,
    };
    match hint {
        Some(hint) => format!("Discord Gateway が接続を閉じました (コード {code}): {hint}"),
        None => format!("Discord Gateway が接続を閉じました (コード {code})"),
    }
}

#[cfg(test)]
mod tests {
    use std::{error::Error as _, io};

    use super::{Error, handler_error};

    #[test]
    fn boxed_handler_error_keeps_the_original_error() {
        let source: Box<dyn std::error::Error + Send + Sync> = Box::new(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "test source",
        ));
        let error = Error::from(source);
        let Error::Handler(inner) = &error else {
            panic!("Handler を想定");
        };
        assert_eq!(
            inner.downcast_ref::<io::Error>().map(io::Error::kind),
            Some(io::ErrorKind::PermissionDenied)
        );
        assert_eq!(error.to_string(), "test source");
        // 表示に元のメッセージがあるので、原因には同じメッセージを繰り返さない。
        assert!(error.source().is_none());
    }

    #[test]
    fn handler_error_shows_each_message_once_in_the_chain() {
        let error = Error::handler(Outer(Inner));
        let mut chain = vec![error.to_string()];
        let mut source = error.source();
        while let Some(cause) = source {
            chain.push(cause.to_string());
            source = cause.source();
        }
        assert_eq!(chain, ["外側のエラー", "内側のエラー"]);
    }

    #[derive(Debug)]
    struct Inner;
    #[derive(Debug)]
    struct Outer(Inner);
    impl std::fmt::Display for Inner {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("内側のエラー")
        }
    }
    impl std::fmt::Display for Outer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("外側のエラー")
        }
    }
    impl std::error::Error for Inner {}
    impl std::error::Error for Outer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn handler_errors_keep_akari_errors_and_wrap_everything_else() {
        // Akari のエラーは、包み直さない（バリアントを保つ）。
        let error = handler_error(Error::EmptyToken);
        assert!(matches!(error, Error::EmptyToken));
        let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(Error::MissingAuthor);
        assert!(matches!(handler_error(boxed), Error::MissingAuthor));

        // 標準のエラーや、メッセージだけのエラーは、Handler で包む。
        let error = handler_error(io::Error::new(io::ErrorKind::NotFound, "missing"));
        assert!(matches!(&error, Error::Handler(_)));
        assert_eq!(error.to_string(), "missing");
        let error = handler_error("plain text");
        assert!(matches!(&error, Error::Handler(_)));
        assert_eq!(error.to_string(), "plain text");
        let error = handler_error("1x".parse::<i64>().unwrap_err());
        assert!(matches!(&error, Error::Handler(_)));
    }

    #[test]
    fn msg_builds_a_handler_error_from_any_display() {
        let error = Error::msg("使い方: !echo 文章");
        assert!(matches!(&error, Error::Handler(_)));
        assert_eq!(error.to_string(), "使い方: !echo 文章");
        assert_eq!(Error::msg(42).to_string(), "42");
    }
}
