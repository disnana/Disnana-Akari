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

/// エラーの報告先です。`tracing` のイベントとして発行し、フックが設定されていれば、フックにも渡します。
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
        emit_error(&report);
        if let Some(hook) = &self.hook {
            hook(&report);
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

    /// 応答にかかった時間を測る必要があるか。フックがあるか、デバッグのログが有効なときだけ測る。
    pub(crate) fn measures_timing(&self) -> bool {
        self.has_hook() || tracing::enabled!(target: LOG_TARGET, tracing::Level::DEBUG)
    }

    pub(crate) fn report(&self, event: InfoEvent) {
        emit_info(&event);
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

/// ログのイベントの `target`。利用者は、これでフィルターできる（例: `RUST_LOG=disnana_akari=info`）。
const LOG_TARGET: &str = "disnana_akari";

/// エラーを、`tracing` のイベントとして発行する。出力先は、利用者が購読先で決める。
fn emit_error(report: &ErrorReport<'_>) {
    let source = report.source.label();
    let name = report.name.unwrap_or_default();
    match report.source {
        // 処理を捨てたことや、自動の応答の失敗は、続行できる問題として警告にする。
        ErrorSource::QueueFull | ErrorSource::Response => {
            tracing::warn!(target: LOG_TARGET, source, name, "{report}");
        }
        _ => tracing::error!(target: LOG_TARGET, source, name, "{report}"),
    }
}

/// ログに載せるミリ秒。`u64` に収まらない長さ（約 5.8 億年）は、最大値にする。
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// 実行時の情報を、`tracing` のイベントとして発行する。
fn emit_info(event: &InfoEvent) {
    match event {
        InfoEvent::GatewayReady => {
            tracing::info!(target: LOG_TARGET, "Gateway のセッションが準備できました");
        }
        InfoEvent::GatewayResumed => {
            tracing::info!(target: LOG_TARGET, "Gateway のセッションを再開しました");
        }
        InfoEvent::SlashCommandsSynced { guild_id, count } => {
            tracing::info!(
                target: LOG_TARGET,
                guild_id = guild_id.map(|id| id.get()),
                count,
                "スラッシュコマンドを同期しました"
            );
        }
        InfoEvent::InteractionResponded {
            interaction_type,
            response_type,
            elapsed,
            rest_elapsed,
        } => {
            tracing::debug!(
                target: LOG_TARGET,
                interaction_type = ?interaction_type,
                response_type = ?response_type,
                elapsed_ms = millis(*elapsed),
                rest_elapsed_ms = millis(*rest_elapsed),
                "Interaction に初回応答しました"
            );
        }
    }
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

    /// 発行されたイベントを集めるだけの購読先。テストで、依存を増やさずにログを確かめる。
    ///
    /// `tracing` は、発行箇所ごとに「購読先がいるか」をプロセス全体で覚える。テストはスレッドを
    /// 並べて走るので、スレッドごとの購読先（`with_default`）を使うと、別のテストが先に同じ発行箇所を
    /// 通ったときに、イベントを取りこぼす。そのため、購読先はプロセスに 1 回だけ置き、
    /// 集める先をスレッドごとに分ける。
    mod capture {
        use std::{cell::RefCell, fmt, sync::Once};

        use tracing::{
            Event, Level, Metadata, Subscriber,
            field::{Field, Visit},
            span::{Attributes, Id, Record},
        };

        pub(super) type Seen = Vec<(Level, String, String)>;

        thread_local! {
            static SINK: RefCell<Option<Seen>> = const { RefCell::new(None) };
        }

        struct Message(String, Vec<String>);

        impl Visit for Message {
            fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
                if field.name() == "message" {
                    self.0 = format!("{value:?}");
                } else {
                    self.1.push(format!("{}={value:?}", field.name()));
                }
            }
        }

        struct Router;

        impl Subscriber for Router {
            fn enabled(&self, _: &Metadata<'_>) -> bool {
                true
            }
            fn new_span(&self, _: &Attributes<'_>) -> Id {
                Id::from_u64(1)
            }
            fn record(&self, _: &Id, _: &Record<'_>) {}
            fn record_follows_from(&self, _: &Id, _: &Id) {}
            fn event(&self, event: &Event<'_>) {
                SINK.with(|sink| {
                    // 集めているスレッドのイベントだけ残す。ほかのスレッドのものは捨てる。
                    if let Some(seen) = sink.borrow_mut().as_mut() {
                        let mut message = Message(String::new(), Vec::new());
                        event.record(&mut message);
                        seen.push((
                            *event.metadata().level(),
                            event.metadata().target().to_owned(),
                            format!("{} [{}]", message.0, message.1.join(", ")),
                        ));
                    }
                });
            }
            fn enter(&self, _: &Id) {}
            fn exit(&self, _: &Id) {}
        }

        /// `run` の間に、このスレッドで発行されたイベントを集めて返す。
        pub(super) fn collect(run: impl FnOnce()) -> Seen {
            static INSTALL: Once = Once::new();
            INSTALL.call_once(|| {
                tracing::subscriber::set_global_default(Router)
                    .expect("テストの購読先は、プロセスで 1 回だけ置く");
            });
            SINK.with(|sink| *sink.borrow_mut() = Some(Vec::new()));
            run();
            SINK.with(|sink| sink.borrow_mut().take().unwrap_or_default())
        }
    }

    #[test]
    fn errors_and_info_are_emitted_as_tracing_events_at_matching_levels() {
        use tracing::Level;

        use super::{InfoEvent, InfoReporter};

        let seen = capture::collect(|| {
            // フックがなくても、ログには出る（出力先は、購読先が決める）。
            let reporter = Reporter::default();
            reporter.report(
                ErrorSource::Button,
                Some("akari:demo"),
                &Error::msg("壊れました"),
            );
            reporter.report(ErrorSource::QueueFull, Some("READY"), &Error::GatewayEnded);
            let info = InfoReporter::default();
            info.report(InfoEvent::GatewayReady);
            info.report(InfoEvent::SlashCommandsSynced {
                guild_id: None,
                count: 3,
            });
        });

        let levels: Vec<_> = seen.iter().map(|(level, ..)| *level).collect();
        assert_eq!(
            levels,
            [Level::ERROR, Level::WARN, Level::INFO, Level::INFO]
        );
        assert!(seen.iter().all(|(_, target, _)| target == "disnana_akari"));
        assert!(
            seen[0].2.contains("ボタン `akari:demo`: 壊れました"),
            "{}",
            seen[0].2
        );
        assert!(seen[0].2.contains("source=\"ボタン\""), "{}", seen[0].2);
        assert!(seen[3].2.contains("count=3"), "{}", seen[3].2);
    }

    #[test]
    fn nothing_is_written_when_no_subscriber_is_set() {
        // 購読先がなければ、イベントは捨てられる。パニックもしない。
        Reporter::default().report(ErrorSource::Gateway, None, &Error::GatewayEnded);
        super::InfoReporter::default().report(super::InfoEvent::GatewayResumed);
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
