use std::sync::Arc;

use twilight_http::{Client, api_error::ApiError, error::ErrorType};
use twilight_model::{
    guild::Member,
    id::{
        Id,
        marker::{GuildMarker, UserMarker},
    },
};

use crate::{DiscordArgument, MemberCacheHandle};

use super::ArgumentResolutionError;

const SEARCH_LIMIT: u16 = 1_000;

/// メンション、ユーザー ID、完全一致のユーザー名またはニックネームをサーバーのメンバーに変換します。
///
/// ID が分かる場合は指定したキャッシュを優先します。名前は同じサーバーの REST 検索結果で判定します。
/// 名前は大文字と小文字を区別し、Discord のグローバル表示名では検索しません。
///
/// # エラー
///
/// 入力形式が不正な場合や、メンバーが見つからない場合に失敗します。
/// 複数のメンバーが一致する場合や、検索結果が上限の1,000件に達した場合は
/// [`ArgumentResolutionError::Ambiguous`] を返します。Discord API の失敗も返します。
pub async fn resolve(
    http: &Client,
    guild: Id<GuildMarker>,
    cache: Option<&MemberCacheHandle>,
    argument: &str,
) -> Result<Arc<Member>, ArgumentResolutionError> {
    if let Some(user) = Id::<UserMarker>::parse_discord_argument(argument) {
        if let Some(member) = cache.and_then(|cache| cache.get(guild, user)) {
            return Ok(member);
        }
        return fetch_by_id(http, guild, user).await;
    }

    if argument.trim().is_empty()
        || argument.starts_with("<@")
        || argument.starts_with("<#")
        || argument.bytes().all(|byte| byte.is_ascii_digit())
        || argument.chars().take(33).count() > 32
    {
        return Err(ArgumentResolutionError::InvalidInput);
    }

    // メンバーキャッシュは部分的なので、名前が一意かどうかを判断する根拠にしない。
    let members = http
        .search_guild_members(guild, argument)
        .limit(SEARCH_LIMIT)
        .await?
        .models()
        .await?;
    select_exact(argument, members).map(Arc::new)
}

async fn fetch_by_id(
    http: &Client,
    guild: Id<GuildMarker>,
    user: Id<UserMarker>,
) -> Result<Arc<Member>, ArgumentResolutionError> {
    let response = http.guild_member(guild, user).await;
    let member = match response {
        Ok(response) => response.model().await?,
        Err(error)
            if matches!(
                error.kind(),
                ErrorType::Response { status, error: ApiError::General(body), .. }
                    if status.get() == 404 && body.code == 10_007
            ) =>
        {
            return Err(ArgumentResolutionError::NotFound);
        }
        Err(error) => return Err(error.into()),
    };
    if member.user.id != user {
        return Err(ArgumentResolutionError::NotFound);
    }
    Ok(Arc::new(member))
}

fn select_exact(argument: &str, members: Vec<Member>) -> Result<Member, ArgumentResolutionError> {
    // 検索結果には総件数がない。上限に達したら、別の完全一致が隠れている可能性がある。
    if members.len() >= usize::from(SEARCH_LIMIT) {
        return Err(ArgumentResolutionError::Ambiguous);
    }
    let mut matches = members
        .into_iter()
        .filter(|member| member.user.name == argument || member.nick.as_deref() == Some(argument));
    let first = matches.next().ok_or(ArgumentResolutionError::NotFound)?;
    if matches.any(|candidate| candidate.user.id != first.user.id) {
        return Err(ArgumentResolutionError::Ambiguous);
    }
    Ok(first)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::{Value, json};
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::TcpListener,
    };
    use twilight_gateway::{Event, Intents, ShardId};
    use twilight_model::gateway::payload::incoming::MemberAdd;

    use super::*;
    use crate::{Events, member_cache::MemberCacheRuntime};

    fn member(id: u64, username: &str, nick: Option<&str>) -> Value {
        json!({
            "user": {
                "id": id.to_string(), "username": username, "discriminator": "0",
                "global_name": "global display"
            },
            "nick": nick, "deaf": false, "mute": false, "flags": 0, "roles": []
        })
    }

    fn model(id: u64, username: &str, nick: Option<&str>) -> Member {
        serde_json::from_value(member(id, username, nick)).unwrap()
    }

    #[test]
    fn accepts_exact_nickname_or_username_but_not_prefix_or_global_name() {
        let target = model(8, "username", Some("nickname"));
        for argument in ["username", "nickname"] {
            assert_eq!(
                select_exact(argument, vec![target.clone()])
                    .unwrap()
                    .user
                    .id,
                Id::new(8)
            );
        }
        for argument in ["user", "Username", "global display"] {
            assert!(matches!(
                select_exact(argument, vec![target.clone()]),
                Err(ArgumentResolutionError::NotFound)
            ));
        }
    }

    #[test]
    fn rejects_mixed_name_collisions_and_capped_search_results() {
        let username = model(8, "target", None);
        let nick = model(9, "another", Some("target"));
        assert!(matches!(
            select_exact("target", vec![username.clone(), nick]),
            Err(ArgumentResolutionError::Ambiguous)
        ));
        assert!(matches!(
            select_exact("target", vec![username.clone(); usize::from(SEARCH_LIMIT)]),
            Err(ArgumentResolutionError::Ambiguous)
        ));
        assert_eq!(
            select_exact("target", vec![username.clone(), username])
                .unwrap()
                .user
                .id,
            Id::new(8)
        );
    }

    async fn reply(listener: &TcpListener, path: &str, status: &str, body: Value) {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = BufReader::new(socket);
        let mut line = String::new();
        socket.read_line(&mut line).await.unwrap();
        assert_eq!(line.trim_end(), format!("GET {path} HTTP/1.1"));
        while !line.trim_end().is_empty() {
            line.clear();
            assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
        }
        let body = body.to_string();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket
            .get_mut()
            .write_all(response.as_bytes())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn exact_member_id_and_name_use_guild_scoped_rest() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http = Client::builder()
            .proxy(listener.local_addr().unwrap().to_string(), true)
            .build();
        let guild = Id::new(2);
        let cache = MemberCacheHandle::new();
        let runtime = MemberCacheRuntime::prepare(
            4,
            &Events::new(),
            Intents::GUILDS | Intents::GUILD_MEMBERS,
            Some(cache.clone()),
            None,
        )
        .unwrap()
        .unwrap();
        runtime
            .shard(ShardId::ONE)
            .observe(&Event::MemberAdd(Box::new(MemberAdd {
                guild_id: guild,
                member: model(11, "cached", Some("exact")),
            })));
        let serve = async {
            reply(
                &listener,
                "/api/v10/guilds/2/members/8",
                "200 OK",
                member(8, "username", None),
            )
            .await;
            reply(
                &listener,
                "/api/v10/guilds/2/members/search?query=exact&limit=1000",
                "200 OK",
                json!([
                    member(9, "other", Some("exact")),
                    member(10, "exactly", None)
                ]),
            )
            .await;
            reply(
                &listener,
                "/api/v10/guilds/2/members/99",
                "404 Not Found",
                json!({"code": 10007, "message": "Unknown Member"}),
            )
            .await;
        };
        let check = async {
            let cached = resolve(&http, guild, Some(&cache), "11").await.unwrap();
            assert_eq!(cached.user.id, Id::new(11));
            assert!(matches!(
                resolve(&http, guild, None, "<@&8>").await,
                Err(ArgumentResolutionError::InvalidInput)
            ));
            let mentioned = resolve(&http, guild, None, "<@!8>").await.unwrap();
            assert_eq!(mentioned.user.id, Id::new(8));
            let named = resolve(&http, guild, Some(&cache), "exact").await.unwrap();
            assert_eq!(named.user.id, Id::new(9));
            assert!(matches!(
                resolve(&http, guild, None, "99").await,
                Err(ArgumentResolutionError::NotFound)
            ));
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(serve, check);
        })
        .await
        .unwrap();
    }
}
