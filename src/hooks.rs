//! 実行時の通知先と、Akari が自動で送る文言の設定です。

use std::{fmt, sync::Arc, time::Duration};

use twilight_model::{
    application::interaction::InteractionType,
    http::interaction::InteractionResponseType,
    id::{Id, marker::GuildMarker},
};

use crate::Error;

type ErrorHook = dyn Fn(&ErrorReport<'_>) + Send + Sync;

/// エラーが起きた場所です。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum ErrorSource {
    /// テキストコマンドの関数が返したエラーです。
    TextCommand,
    /// スラッシュコマンドの関数が返したエラーです。
    SlashCommand,
    /// ボタンの関数が返したエラーです。
    Button,
    /// 選択メニューの関数が返したエラーです。
    Select,
    /// 入力フォームの関数が返したエラーです。
    Modal,
    /// `Bot::run` に渡したメッセージ用の関数が返したエラーです。
    Message,
    /// `Events::on_ready` や `Events::on_message` の関数が返したエラーです。
    Event,
    /// Discord Gateway との接続で起きたエラーです。
    Gateway,
    /// Akari が自動で送った応答が、Discord に受け付けられなかったときのエラーです。
    Response,
    /// 同時に処理できる数か、待てる数がいっぱいで、イベントを処理できなかったことです。
    QueueFull,
}

impl ErrorSource {
    /// 場所を表す短い日本語の名前を返します。
    pub fn label(self) -> &'static str {
        match self {
            Self::TextCommand => "テキストコマンド",
            Self::SlashCommand => "スラッシュコマンド",
            Self::Button => "ボタン",
            Self::Select => "選択メニュー",
            Self::Modal => "入力フォーム",
            Self::Message => "メッセージの処理",
            Self::Event => "イベント",
            Self::Gateway => "Gateway",
            Self::Response => "自動応答",
            Self::QueueFull => "処理待ち",
        }
    }
}

impl fmt::Display for ErrorSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// 一件のエラーの報告です。[`Bot::on_error`](crate::Bot::on_error) に渡した関数が受け取ります。
#[non_exhaustive]
pub struct ErrorReport<'a> {
    /// エラーが起きた場所です。
    pub source: ErrorSource,
    /// コマンド名、`custom_id`、イベント名など、原因を特定する手がかりです。分からないときは `None` です。
    pub name: Option<&'a str>,
    /// エラーの内容です。ハンドラーが返した型付きエラーを、そのまま渡します。
    pub error: &'a Error,
}

impl fmt::Display for ErrorReport<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.source)?;
        if let Some(name) = self.name {
            write!(f, " `{name}`")?;
        }
        write!(f, ": {}", self.error)?;
        // 原因の連鎖も、「: 原因」の形で続ける（`Http` などは、原因に本当の理由がある）。
        let mut cause = std::error::Error::source(self.error);
        while let Some(inner) = cause {
            write!(f, ": {inner}")?;
            cause = inner.source();
        }
        Ok(())
    }
}

impl fmt::Debug for ErrorReport<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ErrorReport")
            .field("source", &self.source)
            .field("name", &self.name)
            .field("error", &format_args!("{}", self.error))
            .finish()
    }
}

/// Akari が利用者へ自動で送る文言です。
///
/// 各項目は `Some(文言)` なら、その文言を送ります。`None` なら、何も送りません。
/// どの場合も、エラー自体は [`Bot::on_error`](crate::Bot::on_error) に報告します。
///
/// スラッシュコマンド、ボタン、選択メニュー、入力フォームは、Discord が受信後 3 秒以内の応答を求めます。
/// 該当する項目を `None` にすると、応答が済んでいない操作は、Discord 上で「失敗」と表示されます。
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Messages {
    /// テキストコマンドの関数がエラーを返したときに、元の投稿へ返信する文言です。既定では送りません。
    pub text_command_failed: Option<String>,
    /// テキストコマンドの処理枠がいっぱいのときに、元の投稿へ返信する文言です。既定では送りません。
    pub text_command_busy: Option<String>,
    /// スラッシュコマンド、ボタン、選択メニュー、入力フォームの処理枠がいっぱいのときに、本人にだけ送る文言です。
    pub interaction_busy: Option<String>,
    /// スラッシュコマンドの関数がエラーを返したか、応答しなかったときに送る文言です。
    pub slash_command_failed: Option<String>,
    /// Akari に登録していないスラッシュコマンドが実行されたときに、本人にだけ送る文言です。
    pub slash_command_unavailable: Option<String>,
    /// 実行前の確認（権限など）に通らなかったときに、本人にだけ送る文言です。
    /// ハンドラーが `require_*` で返した [`CheckFailure`](crate::CheckFailure) に使います。
    /// 公開の遅延応答がある場合は、固定文言で待機表示を終了してから、本人への追加応答で送ります。
    /// テキストコマンドでは、元の投稿への返信になります（送らない場合は `None` にします）。
    pub check_failed: Option<String>,
    /// ボタンの関数がエラーを返したときに、本人にだけ送る文言です。
    pub button_failed: Option<String>,
    /// 選択メニューの関数がエラーを返したときや、選択内容を読み取れなかったときに、本人にだけ送る文言です。
    pub select_failed: Option<String>,
    /// 入力フォームの関数がエラーを返したときに、本人にだけ送る文言です。
    pub modal_failed: Option<String>,
    /// 入力フォームの関数が、成功したのに何も応答しなかったときに、本人にだけ送る文言です。
    pub modal_accepted: Option<String>,
}

impl Default for Messages {
    fn default() -> Self {
        let text = |text: &str| Some(text.to_owned());
        Self {
            text_command_failed: None,
            text_command_busy: None,
            interaction_busy: text("現在混雑しています。少し待ってからもう一度お試しください。"),
            slash_command_failed: text("コマンドを処理できませんでした。"),
            slash_command_unavailable: text("このコマンドは現在利用できません。"),
            check_failed: text("このコマンドを使う権限がありません。"),
            button_failed: text("ボタンを処理できませんでした。"),
            select_failed: text("選択内容を処理できませんでした。"),
            modal_failed: text("入力内容を処理できませんでした。"),
            modal_accepted: text("入力を受け付けました。"),
        }
    }
}

/// エラーの報告先です。フックが未設定のときは、標準エラー出力に書きます。
#[derive(Clone, Default)]
pub(crate) struct Reporter {
    hook: Option<Arc<ErrorHook>>,
}

impl Reporter {
    pub(crate) fn new(hook: impl Fn(&ErrorReport<'_>) + Send + Sync + 'static) -> Self {
        Self {
            hook: Some(Arc::new(hook)),
        }
    }

    pub(crate) fn report(&self, source: ErrorSource, name: Option<&str>, error: &Error) {
        let report = ErrorReport {
            source,
            name,
            error,
        };
        match &self.hook {
            Some(hook) => hook(&report),
            None => write_stderr(&report),
        }
    }

    pub(crate) fn has_hook(&self) -> bool {
        self.hook.is_some()
    }
}

/// [`Bot::on_info`](crate::Bot::on_info) が通知する実行時の情報です。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum InfoEvent {
    /// Gateway で新しいセッションが準備できました。
    GatewayReady,
    /// Gateway で既存のセッションを再開しました。
    GatewayResumed,
    /// スラッシュコマンドの同期が完了しました。
    #[non_exhaustive]
    SlashCommandsSynced {
        /// 同期先のサーバーです。グローバルコマンドの場合は `None` です。
        guild_id: Option<Id<GuildMarker>>,
        /// 今回の同期で確認したコマンド数です。
        count: usize,
    },
    /// Discord が Interaction の初回応答を受け付けました。
    #[non_exhaustive]
    InteractionResponded {
        /// スラッシュコマンド、コンポーネント、フォームなどの種類です。
        interaction_type: InteractionType,
        /// 初回メッセージ、遅延応答、更新、フォーム表示などの種類です。
        response_type: InteractionResponseType,
        /// Akari が Gateway イベントを受け取ってから、REST 応答を受け取るまでの時間です。
        elapsed: Duration,
        /// 初回応答の REST リクエストにかかった時間です。レート制限の待ち時間を含みます。
        rest_elapsed: Duration,
    },
}

type InfoHook = dyn Fn(&InfoEvent) + Send + Sync;

#[derive(Clone, Default)]
pub(crate) struct InfoReporter(Option<Arc<InfoHook>>);

impl InfoReporter {
    pub(crate) fn new(hook: impl Fn(&InfoEvent) + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(hook)))
    }

    pub(crate) fn has_hook(&self) -> bool {
        self.0.is_some()
    }

    pub(crate) fn report(&self, event: InfoEvent) {
        if let Some(hook) = &self.0 {
            hook(&event);
        }
    }
}

/// 実行中の各処理に渡す通知先と文言です。
#[derive(Clone, Default)]
pub(crate) struct Hooks {
    pub(crate) reporter: Reporter,
    pub(crate) info: InfoReporter,
    pub(crate) messages: Messages,
}

/// `Bot::on_error` が未設定のときの既定の出力先。
#[allow(
    clippy::print_stderr,
    reason = "既定の出力先として、標準エラー出力に書く。ライブラリが出力するのは、ここだけ。"
)]
fn write_stderr(report: &ErrorReport<'_>) {
    eprintln!("[akari] {report}");
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::Error;

    use super::{ErrorSource, Hooks, Messages, Reporter};

    #[test]
    fn report_text_continues_with_the_cause_chain() {
        use std::fmt;

        #[derive(Debug)]
        struct Reason;
        #[derive(Debug)]
        struct Failure(Reason);
        impl fmt::Display for Reason {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("本当の理由")
            }
        }
        impl fmt::Display for Failure {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("失敗しました")
            }
        }
        impl std::error::Error for Reason {}
        impl std::error::Error for Failure {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }

        let error = Error::handler(Failure(Reason));
        let report = super::ErrorReport {
            source: ErrorSource::Button,
            name: Some("akari:demo"),
            error: &error,
        };
        assert_eq!(
            report.to_string(),
            "ボタン `akari:demo`: 失敗しました: 本当の理由"
        );
    }

    #[test]
    fn hook_receives_the_source_name_and_error_instead_of_stderr() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let reporter = Reporter::new(move |report| {
            sink.lock().unwrap().push((
                report.source,
                report.name.map(str::to_owned),
                report.error.to_string(),
                report.to_string(),
            ));
        });

        reporter.report(
            ErrorSource::Button,
            Some("akari:demo"),
            &Error::msg("壊れました"),
        );
        reporter.report(ErrorSource::Gateway, None, &Error::GatewayEnded);

        let seen = seen.lock().unwrap();
        assert_eq!(
            seen[0],
            (
                ErrorSource::Button,
                Some("akari:demo".to_owned()),
                "壊れました".to_owned(),
                "ボタン `akari:demo`: 壊れました".to_owned()
            )
        );
        assert_eq!(seen[1].3, "Gateway: Discord Gateway の接続が終了しました");
    }

    #[test]
    fn defaults_stay_quiet_where_discord_does_not_require_a_response() {
        let messages = Messages::default();
        // テキストコマンドには、応答の決まりがないので、既定では何も送らない。
        assert_eq!(messages.text_command_failed, None);
        assert_eq!(messages.text_command_busy, None);
        // Interaction は 3 秒以内の応答が決まりなので、既定の文言を持つ。
        for text in [
            &messages.interaction_busy,
            &messages.slash_command_failed,
            &messages.slash_command_unavailable,
            &messages.button_failed,
            &messages.select_failed,
            &messages.modal_failed,
            &messages.modal_accepted,
        ] {
            assert!(text.as_deref().is_some_and(|text| !text.is_empty()));
        }
        assert!(Hooks::default().messages == messages);
    }

    #[test]
    fn gateway_close_explains_the_codes_people_hit_first() {
        assert!(
            Error::GatewayClose(4014)
                .to_string()
                .contains("Message Content Intent")
        );
        assert!(Error::GatewayClose(4004).to_string().contains("トークン"));
        assert_eq!(
            Error::GatewayClose(1000).to_string(),
            "Discord Gateway が接続を閉じました (コード 1000)"
        );
    }
}
