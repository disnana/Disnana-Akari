use std::{cmp::Reverse, future::Future, panic::AssertUnwindSafe, pin::Pin, sync::Arc};

use futures_util::FutureExt as _;
use twilight_model::channel::Message;

use crate::{
    RegistrationError, Result,
    error::{handler_error, panic_error},
};

/// メッセージに使えるプレフィックスの候補です。
///
/// 空の候補（[`Default`]）を動的なプレフィックス判定で返すと、その投稿ではコマンドを実行しません。
/// 複数の候補が一致した場合は最も長いものを使います。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommandPrefixes(Arc<[Box<str>]>);

impl CommandPrefixes {
    /// 使えるプレフィックスを設定します。
    ///
    /// # エラー
    ///
    /// 空文字列や、空白を含む候補を拒否します。候補が 0 件の一覧は指定できます。
    pub fn new(prefixes: &[&str]) -> Result<Self> {
        if prefixes
            .iter()
            .any(|prefix| prefix.is_empty() || prefix.chars().any(char::is_whitespace))
        {
            return Err(RegistrationError::InvalidPrefix.into());
        }
        let mut prefixes: Vec<Box<str>> = prefixes.iter().copied().map(Into::into).collect();
        prefixes.sort_unstable_by_key(|prefix| Reverse(prefix.len()));
        Ok(Self(prefixes.into()))
    }

    pub(super) fn match_len(&self, content: &str) -> Option<usize> {
        self.0
            .iter()
            .find(|prefix| content.starts_with(prefix.as_ref()))
            .map(|prefix| prefix.len())
    }
}

#[derive(Clone)]
pub(super) enum PrefixPolicy {
    Fixed(CommandPrefixes),
    Dynamic(Arc<dyn PrefixResolver>),
}

impl std::fmt::Debug for PrefixPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fixed(prefixes) => prefixes.fmt(f),
            Self::Dynamic(_) => f.write_str("Dynamic"),
        }
    }
}

impl PrefixPolicy {
    pub(super) fn dynamic<E>(
        resolver: impl AsyncFn(&Message) -> std::result::Result<CommandPrefixes, E>
        + Send
        + Sync
        + 'static,
    ) -> Self
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        Self::Dynamic(Arc::new(
            async move |message: &Message| match AssertUnwindSafe(resolver(message))
                .catch_unwind()
                .await
            {
                Ok(result) => result.map_err(handler_error),
                Err(payload) => Err(panic_error(payload)),
            },
        ))
    }
}

pub(super) trait PrefixResolver: Send + Sync {
    fn resolve<'a>(
        &'a self,
        message: &'a Message,
    ) -> Pin<Box<dyn Future<Output = Result<CommandPrefixes>> + 'a>>;
}

impl<F> PrefixResolver for F
where
    F: AsyncFn(&Message) -> Result<CommandPrefixes> + Send + Sync,
{
    fn resolve<'a>(
        &'a self,
        message: &'a Message,
    ) -> Pin<Box<dyn Future<Output = Result<CommandPrefixes>> + 'a>> {
        Box::pin(self(message))
    }
}
