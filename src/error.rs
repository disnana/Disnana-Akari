use twilight_gateway::{Intents, error::ReceiveMessageError};
use twilight_http::{error::Error as HttpError, response::DeserializeBodyError};
use twilight_model::guild::Permissions;
use twilight_validate::{
    command::CommandValidationError, component::ComponentValidationError,
    embed::EmbedValidationError, message::MessageValidationError,
};

/// Akari の処理や、利用者が登録したハンドラーから返るエラーです。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// 実行前の確認（権限など）に通りませんでした。
    #[error(transparent)]
    Check(#[from] CheckFailure),
    /// `Bot::new` に空白だけ、または空のトークンが渡されました。
    #[error("Discord Bot トークンが空です")]
    EmptyToken,
    /// [`Limits`](crate::Limits) の項目に 0 が指定されました。
    #[error("Limits の `{field}` は 1 以上にしてください")]
    InvalidLimits {
        /// 0 が指定された項目の名前です。
        field: &'static str,
    },
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
    /// コマンドやハンドラーの登録内容が無効です。
    #[error(transparent)]
    Registration(#[from] RegistrationError),
    /// Discord が返したスラッシュコマンドの登録結果が要求と一致しません。
    #[error(transparent)]
    CommandSync(#[from] CommandSyncError),
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
    #[error(transparent)]
    ResponseState(#[from] ResponseStateError),
    /// Discord が送った選択内容を読み取れませんでした。
    #[error(transparent)]
    Selection(#[from] SelectionError),
    /// コマンドを実行したユーザーを取得できませんでした。
    #[error("実行したユーザーを確認できませんでした。")]
    MissingAuthor,
    /// 同時処理枠か待ち行列がいっぱいです。
    #[error("{}", .0.message())]
    QueueFull(QueueKind),
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

/// ハンドラーの種類です。登録のエラーで、どの種類の登録かを示します。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum HandlerKind {
    /// テキストコマンドです。
    TextCommand,
    /// スラッシュコマンドです。
    SlashCommand,
    /// ボタンです。
    Button,
    /// 文字列の選択メニューです。
    Select,
    /// ユーザーの選択メニューです。
    UserSelect,
    /// ロールの選択メニューです。
    RoleSelect,
    /// チャンネルの選択メニューです。
    ChannelSelect,
    /// ユーザーとロールの選択メニューです。
    MentionableSelect,
    /// 入力フォームです。
    Modal,
}

impl HandlerKind {
    /// 種類を表す短い日本語の名前を返します。
    pub fn label(self) -> &'static str {
        match self {
            Self::TextCommand => "テキストコマンド",
            Self::SlashCommand => "スラッシュコマンド",
            Self::Button => "ボタン",
            Self::Select => "セレクト",
            Self::UserSelect => "ユーザーセレクト",
            Self::RoleSelect => "ロールセレクト",
            Self::ChannelSelect => "チャンネルセレクト",
            Self::MentionableSelect => "メンショナブルセレクト",
            Self::Modal => "モーダル",
        }
    }
}

impl std::fmt::Display for HandlerKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// コマンドやハンドラーを登録できなかった理由です。
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum RegistrationError {
    /// プレフィックスが空、または空白を含みます。
    #[error("プレフィックスには空白を含まない文字列を指定してください")]
    InvalidPrefix,
    /// コマンド名が空、または空白を含みます。
    #[error("コマンド名には空白を含まない文字列を指定してください")]
    InvalidCommandName,
    /// `custom_id` が 1 文字以上 100 文字以下ではありません。
    #[error("{kind} ID は 1 文字以上 100 文字以下で指定してください")]
    InvalidCustomId {
        /// 登録しようとしたハンドラーの種類です。
        kind: HandlerKind,
    },
    /// 登録されていないコマンドの設定を変えようとしました。
    #[error("コマンド `{name}` は登録されていません")]
    UnknownCommand {
        /// 指定された名前です。
        name: String,
    },
    /// 同じ名前、または同じ `custom_id` が、すでに登録されています。
    #[error("{kind} `{name}` は登録済みです")]
    Duplicate {
        /// 登録しようとしたハンドラーの種類です。
        kind: HandlerKind,
        /// 重複した名前、または `custom_id` です。
        name: String,
    },
}

/// 実行前の確認（権限など）に通らなかった理由です。
///
/// ハンドラーが `ctx.require_permissions(..)?` などで返します。
/// `Bot` は、この種類のエラーを [`Messages::check_failed`](crate::Messages) で本人に知らせます。
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum CheckFailure {
    /// サーバーの中でしか使えないコマンドが、DM などで呼ばれました。
    #[error("このコマンドはサーバーの中でしか使えません")]
    GuildOnly,
    /// 呼び出した人に、必要な権限が足りません。
    #[error("必要な権限が足りません: {0:?}")]
    MissingPermissions(Permissions),
    /// Discord から権限の情報が届かなかったため、確認できません（安全のため、実行させません）。
    #[error("権限を確認できませんでした")]
    PermissionsUnavailable,
}

/// Discord が返したスラッシュコマンドの登録結果が、要求と合わなかった理由です。
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum CommandSyncError {
    /// 返ってきたコマンドの件数が、登録した件数と一致しません。
    #[error("Discord が返したコマンド件数（{returned}）が登録件数（{expected}）と一致しません")]
    CountMismatch {
        /// 登録を求めた件数です。
        expected: usize,
        /// Discord が返した件数です。
        returned: usize,
    },
    /// 返ってきたコマンドが、スラッシュコマンドではありません。
    #[error("`/{name}` の登録結果がスラッシュコマンドではありません")]
    NotChatInput {
        /// コマンド名です。
        name: String,
    },
    /// 返ってきたコマンドに ID がありません。
    #[error("`/{name}` の登録結果に ID がありません")]
    MissingId {
        /// コマンド名です。
        name: String,
    },
    /// 返ってきたコマンドが重複しているか、登録を求めていない名前です。
    #[error("`/{name}` の登録結果が重複しているか、登録要求に含まれていません")]
    UnexpectedName {
        /// コマンド名です。
        name: String,
    },
    /// 返ってきたコマンドの ID が重複しています。
    #[error("`/{name}` の登録結果の ID が重複しています")]
    DuplicateId {
        /// コマンド名です。
        name: String,
    },
}

/// Interaction の応答の状態が、行おうとした操作と合わなかった理由です。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ResponseStateError {
    /// すでに初回応答を送っています。
    #[error("この操作にはすでに応答しました")]
    AlreadyResponded,
    /// 公開の遅延応答の後に、本人にだけ見える応答へは変えられません。
    #[error("遅延応答の公開範囲は変更できません")]
    VisibilityLocked,
    /// 追加メッセージは、最初の応答を送ってからでないと送れません。
    #[error("追加メッセージを送る前に、最初の応答（reply など）を送ってください")]
    FollowupBeforeResponse,
    /// フォームは、スラッシュコマンドの初回応答として開く必要があります。
    #[error("フォームはスラッシュコマンドの初回応答で開いてください")]
    ModalNotFirstResponse,
    /// この操作では、元のメッセージを更新できません。
    #[error("この操作では元のメッセージを更新できません")]
    CannotUpdate,
    /// Components V2 のメッセージの更新には、`update_v2` を使う必要があります。
    #[error("Components V2 のメッセージには update_v2 を使ってください")]
    UseUpdateV2,
    /// `update_v2` には、Components V2 で作られた元メッセージが必要です。
    #[error("Components V2 で作られた元メッセージが必要です")]
    NeedsV2Message,
}

/// Discord が送った選択内容を、読み取れなかった理由です。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum SelectionError {
    /// 選択内容を読み取れませんでした。
    #[error("Discord が送った選択内容を読み取れませんでした")]
    Unreadable,
    /// 選択数が上限（25）を超えています。
    #[error("セレクトの選択数が上限を超えています")]
    TooMany,
    /// 値が Discord の ID ではありません。
    #[error("セレクトの値が Discord ID ではありません")]
    NotAnId,
    /// 同じ値が重複しています。
    #[error("セレクトの値が重複しています")]
    Duplicate,
    /// メンショナブルの解決済みデータがありません。
    #[error("メンショナブルの解決済みデータがありません")]
    MissingResolved,
    /// メンショナブルの値を、ユーザーかロールの一方に特定できません。
    #[error("メンショナブルの値をユーザーまたはロールの一方に特定できません")]
    AmbiguousMention,
}

/// 処理枠か待ち行列がいっぱいだった場所です。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum QueueKind {
    /// `on_ready` や `on_message` などのイベントの待ち行列です。
    Event,
    /// テキストコマンドの処理枠です。
    TextCommand,
    /// スラッシュコマンド、ボタン、選択メニュー、フォームの処理枠です。
    Interaction,
    /// 混雑の通知を返すための待ち行列です。
    BusyNotice,
}

impl QueueKind {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Event => "イベントの待ち行列がいっぱいで、処理できませんでした",
            Self::TextCommand => "処理枠がいっぱいで、コマンドを実行できませんでした",
            Self::Interaction => "処理枠がいっぱいで、操作を実行できませんでした",
            Self::BusyNotice => "混雑の通知の待ち行列がいっぱいで、通知できませんでした",
        }
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
