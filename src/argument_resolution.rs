use twilight_http::{
    Client, Error as HttpError, api_error::ApiError, error::ErrorType,
    response::DeserializeBodyError,
};
use twilight_model::{
    channel::Channel,
    guild::Role,
    id::{
        Id,
        marker::{ChannelMarker, GuildMarker, RoleMarker},
    },
};

use crate::DiscordArgument;

mod member;
pub use member::resolve as resolve_member;

const MAX_NAME_CHARS: usize = 100;
const UNKNOWN_CHANNEL: u64 = 10_003;
const UNKNOWN_ROLE: u64 = 10_011;

/// テキスト入力から Discord の対象を特定できなかった理由です。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ArgumentResolutionError {
    /// サーバーでのコンテキストが必要です。
    #[error("サーバーの中で対象を指定してください")]
    GuildOnly,
    /// 空の名前、長さの上限、または ID やメンションの形式が無効です。
    #[error("対象の名前や ID の形式が無効です")]
    InvalidInput,
    /// このサーバーで対象を特定できませんでした。
    #[error("このサーバーに対象が見つかりません")]
    NotFound,
    /// 同じ名前の候補が複数あり、対象を一意に特定できませんでした。
    #[error("対象を一意に特定できません。ID またはメンションで指定してください")]
    Ambiguous,
    /// Discord API への問い合わせが失敗しました。
    ///
    /// 自動ログに応答本文を出さないため、内部診断にはこの値を直接参照します。
    #[error("Discord API への問い合わせに失敗しました")]
    Http(HttpError),
    /// Discord API の応答本文を読み取れませんでした。
    ///
    /// 自動ログに応答本文を出さないため、内部診断にはこの値を直接参照します。
    #[error("Discord API の応答を読み取れませんでした")]
    HttpBody(DeserializeBodyError),
}

impl From<HttpError> for ArgumentResolutionError {
    fn from(error: HttpError) -> Self {
        Self::Http(error)
    }
}

impl From<DeserializeBodyError> for ArgumentResolutionError {
    fn from(error: DeserializeBodyError) -> Self {
        Self::HttpBody(error)
    }
}

/// 指定したサーバーのロールを、名前、ID、またはロールのメンションから特定します。
///
/// 名前は大文字・小文字を区別して完全一致で探します。ID とメンションは、そのサーバーの
/// ロールを直接取得します。
///
/// # エラー
///
/// 入力形式が無効な場合、見つからない場合、名前に複数の候補がある場合、または通信に失敗した場合に
/// [`ArgumentResolutionError`] を返します。
pub async fn resolve_role(
    http: &Client,
    guild: Id<GuildMarker>,
    argument: &str,
) -> Result<Role, ArgumentResolutionError> {
    match lookup::<Id<RoleMarker>>(argument)? {
        Lookup::Id(id) => {
            let role = http
                .role(guild, id)
                .await
                .map_err(|error| unknown_or_http(error, UNKNOWN_ROLE))?
                .model()
                .await?;
            (role.id == id)
                .then_some(role)
                .ok_or(ArgumentResolutionError::NotFound)
        }
        Lookup::Name(name) => only_match(http.roles(guild).await?.models().await?, |role| {
            role.name == name
        }),
    }
}

/// 指定したサーバーのチャンネルを、名前、ID、またはチャンネルのメンションから特定します。
///
/// 名前は大文字・小文字を区別して完全一致で探します。名前の一覧にはスレッドが含まれないため、
/// スレッドは ID またはメンションで指定してください。ID の直接取得では、応答の ID とサーバーを
/// 確認します。入力は名前または ID として読み取り、URL として取得することはありません。
///
/// # エラー
///
/// 入力形式が無効な場合、見つからない場合、名前に複数の候補がある場合、または通信に失敗した場合に
/// [`ArgumentResolutionError`] を返します。ID の直接取得でアクセスが拒否されたときも
/// [`ArgumentResolutionError::NotFound`] を返します。
pub async fn resolve_channel(
    http: &Client,
    guild: Id<GuildMarker>,
    argument: &str,
) -> Result<Channel, ArgumentResolutionError> {
    match lookup::<Id<ChannelMarker>>(argument)? {
        Lookup::Id(id) => {
            let channel = http
                .channel(id)
                .await
                .map_err(channel_error)?
                .model()
                .await?;
            (channel.id == id && channel.guild_id == Some(guild))
                .then_some(channel)
                .ok_or(ArgumentResolutionError::NotFound)
        }
        Lookup::Name(name) => only_match(
            http.guild_channels(guild).await?.models().await?,
            |channel| {
                channel.guild_id.is_none_or(|id| id == guild)
                    && !channel.kind.is_thread()
                    && channel.name.as_deref() == Some(name)
            },
        ),
    }
}

enum Lookup<'a, T> {
    Id(T),
    Name(&'a str),
}

fn lookup<T: DiscordArgument>(argument: &str) -> Result<Lookup<'_, T>, ArgumentResolutionError> {
    if argument.chars().take(MAX_NAME_CHARS + 1).count() > MAX_NAME_CHARS
        || argument.trim().is_empty()
    {
        return Err(ArgumentResolutionError::InvalidInput);
    }
    if let Some(id) = T::parse_discord_argument(argument) {
        return Ok(Lookup::Id(id));
    }
    if argument.starts_with("<@")
        || argument.starts_with("<#")
        || argument.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(ArgumentResolutionError::InvalidInput);
    }
    Ok(Lookup::Name(argument))
}

fn only_match<T>(
    items: impl IntoIterator<Item = T>,
    matches: impl FnMut(&T) -> bool,
) -> Result<T, ArgumentResolutionError> {
    let mut found = items.into_iter().filter(matches);
    let first = found.next().ok_or(ArgumentResolutionError::NotFound)?;
    if found.next().is_some() {
        return Err(ArgumentResolutionError::Ambiguous);
    }
    Ok(first)
}

fn unknown_or_http(error: HttpError, code: u64) -> ArgumentResolutionError {
    if matches!(
        error.kind(),
        ErrorType::Response { status, error: ApiError::General(body), .. }
            if status.get() == 404 && body.code == code
    ) {
        ArgumentResolutionError::NotFound
    } else {
        error.into()
    }
}

fn channel_error(error: HttpError) -> ArgumentResolutionError {
    if matches!(error.kind(), ErrorType::Response { status, .. } if status.get() == 403) {
        ArgumentResolutionError::NotFound
    } else {
        unknown_or_http(error, UNKNOWN_CHANNEL)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::access::tests::{Mock, role_json};

    const GUILD: Id<GuildMarker> = Id::new(20);

    fn role(id: u64, name: &str) -> Value {
        let mut value: Value = serde_json::from_str(&role_json(id, 1)).unwrap();
        value["name"] = json!(name);
        value
    }

    #[tokio::test]
    async fn direct_role_lookup_uses_the_guild_route_and_validates_the_returned_id() {
        let server = Mock::start_with_errors(
            vec![
                ("/guilds/20/roles/35".into(), role(35, "Helper").to_string()),
                (
                    "/guilds/20/roles/36".into(),
                    role(37, "Mismatch").to_string(),
                ),
            ],
            vec![(
                "/guilds/20/roles/39",
                "404 Not Found",
                r#"{"code":10011,"message":"Unknown Role"}"#,
            )],
        )
        .await;
        let http = server.client();
        assert_eq!(
            resolve_role(&http, GUILD, "<@&35>").await.unwrap().id,
            Id::new(35)
        );
        for input in ["36", "39"] {
            assert!(matches!(
                resolve_role(&http, GUILD, input).await,
                Err(ArgumentResolutionError::NotFound)
            ));
        }
        assert_eq!(server.count("GET /guilds/20/roles"), 0);
        assert_eq!(server.count("GET /guilds/20/roles/35"), 1);
    }

    #[tokio::test]
    async fn direct_channel_lookup_rejects_other_guilds_and_supports_threads() {
        let server = Mock::start_with_errors(
            vec![
                (
                    "/channels/30".into(),
                    json!({"id":"30","type":11,"guild_id":"20","name":"thread"}).to_string(),
                ),
                (
                    "/channels/31".into(),
                    json!({"id":"31","type":0,"guild_id":"25","name":"other-guild"}).to_string(),
                ),
                (
                    "/channels/32".into(),
                    json!({"id":"33","type":0,"guild_id":"20","name":"wrong-id"}).to_string(),
                ),
                (
                    "/channels/34".into(),
                    json!({"id":"34","type":1}).to_string(),
                ),
            ],
            vec![
                (
                    "/channels/35",
                    "403 Forbidden",
                    r#"{"code":50001,"message":"Missing Access"}"#,
                ),
                (
                    "/channels/36",
                    "404 Not Found",
                    r#"{"code":10003,"message":"Unknown Channel"}"#,
                ),
            ],
        )
        .await;
        let http = server.client();
        assert!(
            resolve_channel(&http, GUILD, "<#30>")
                .await
                .unwrap()
                .kind
                .is_thread()
        );
        for input in ["31", "32", "34", "35", "36"] {
            assert!(matches!(
                resolve_channel(&http, GUILD, input).await,
                Err(ArgumentResolutionError::NotFound)
            ));
        }
        assert_eq!(server.count("GET /guilds/20/channels"), 0);
        assert_eq!(server.count("GET /channels/30"), 1);
    }

    #[tokio::test]
    async fn name_matching_is_exact_checks_ambiguity_and_stays_within_the_guild() {
        let server = Mock::start(vec![
            (
                "/guilds/20/roles".into(),
                json!([role(51, "Helper"), role(52, "Helper"), role(53, "helper")]).to_string(),
            ),
            (
                "/guilds/20/channels".into(),
                json!([
                    {"id":"31","type":0,"guild_id":"20","name":"same"},
                    {"id":"32","type":0,"guild_id":"20","name":"same"},
                    {"id":"33","type":0,"guild_id":"20","name":"Case"},
                    {"id":"34","type":11,"guild_id":"20","name":"Case"},
                    {"id":"35","type":0,"guild_id":"21","name":"Case"}
                ])
                .to_string(),
            ),
        ])
        .await;
        let http = server.client();
        assert!(matches!(
            resolve_role(&http, GUILD, "Helper").await,
            Err(ArgumentResolutionError::Ambiguous)
        ));
        assert_eq!(
            resolve_role(&http, GUILD, "helper").await.unwrap().id,
            Id::new(53)
        );
        assert!(matches!(
            resolve_channel(&http, GUILD, "same").await,
            Err(ArgumentResolutionError::Ambiguous)
        ));
        assert_eq!(
            resolve_channel(&http, GUILD, "Case").await.unwrap().id,
            Id::new(33)
        );
        for input in ["case", "https://example.invalid/not-a-channel"] {
            assert!(matches!(
                resolve_channel(&http, GUILD, input).await,
                Err(ArgumentResolutionError::NotFound)
            ));
        }
        assert_eq!(server.count("GET /guilds/20/channels"), 4);
        assert_eq!(server.count("GET /guilds/20/roles"), 2);
    }

    #[tokio::test]
    async fn invalid_input_does_not_query_and_http_diagnostics_are_not_rendered() {
        let server = Mock::start_with_errors(
            vec![],
            vec![(
                "/guilds/20/roles",
                "403 Forbidden",
                r#"{"code":50013,"message":"SECRET_HTTP_DETAIL"}"#,
            )],
        )
        .await;
        let http = server.client();
        for input in [
            "",
            "　",
            "0",
            "<#20>",
            "<@&",
            &"壱".repeat(MAX_NAME_CHARS + 1),
        ] {
            assert!(matches!(
                resolve_role(&http, GUILD, input).await,
                Err(ArgumentResolutionError::InvalidInput)
            ));
        }
        assert_eq!(server.count("GET /guilds/20/roles"), 0);
        let error = resolve_role(&http, GUILD, "Helper").await.unwrap_err();
        assert!(matches!(error, ArgumentResolutionError::Http(_)));
        assert!(!error.to_string().contains("SECRET_HTTP_DETAIL"));
        assert!(std::error::Error::source(&error).is_none());
    }
}
