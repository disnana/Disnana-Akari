use std::collections::{BTreeMap, btree_map::Entry};

use serde::Serialize;
use twilight_http::{Client, request::Request, response::marker::ListBody, routing::Route};
use twilight_model::{
    application::{
        command::{Command, CommandType},
        interaction::{InteractionMember, application_command::CommandData},
    },
    channel::Message,
    id::{Id, marker::ApplicationMarker},
    user::User,
};
use twilight_validate::command;

use super::{
    InvocationHooks, Scope, SlashCommands, SlashContext, SlashHandler,
    invoke::PreparedSlashHandler, settings::CommandPayload,
};
use crate::{Error, HandlerKind, RegistrationError, Result, error::handler_error};

/// ユーザーコマンドのメニューで選ばれたユーザーです。
///
/// ハンドラーを呼び出したユーザーは、別途 [`SlashContext::author_id`] で取得できます。
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct UserMenuTarget<'a> {
    /// 選択されたユーザーです。
    pub user: &'a User,
    /// Discord が送信した、選択ユーザーのサーバー内の情報です。DM などでは `None` です。
    pub member: Option<&'a InteractionMember>,
}

impl<'a> UserMenuTarget<'a> {
    fn resolve(data: &'a CommandData) -> Option<Self> {
        let id = data.target_id?.cast();
        let resolved = data.resolved.as_ref()?;
        let user = resolved.users.get(&id).filter(|user| user.id == id)?;
        Some(Self {
            user,
            member: resolved.members.get(&id),
        })
    }
}

fn resolve_message(data: &CommandData) -> Option<&Message> {
    let id = data.target_id?.cast();
    data.resolved
        .as_ref()?
        .messages
        .get(&id)
        .filter(|message| message.id == id)
}

impl SlashCommands {
    /// ユーザーのコンテキストメニューに表示するコマンドを登録します。
    ///
    /// Discord から受け取った選択対象を、追加の通信をせずハンドラーに渡します。
    /// 名前は 1〜32 文字で、空白や大文字を使えます。
    ///
    /// # エラー
    ///
    /// 無効な名前、または同じ名前のユーザーコマンドを登録済みの場合に失敗します。
    pub fn register_user_menu<E>(
        &mut self,
        name: &str,
        handler: impl for<'a> AsyncFn(
            &mut SlashContext<'a>,
            UserMenuTarget<'a>,
        ) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let invoke = async move |ctx: &mut SlashContext<'_>, hooks: &InvocationHooks| {
            let target =
                UserMenuTarget::resolve(ctx.data).ok_or(Error::CommandTargetUnavailable)?;
            let handler = &handler;
            hooks
                .run(ctx, async move |ctx: &mut SlashContext<'_>| {
                    handler(ctx, target).await.map_err(handler_error)
                })
                .await
        };
        self.menus
            .insert(MenuKind::User, name, Box::new(PreparedSlashHandler(invoke)))
    }

    /// メッセージのコンテキストメニューに表示するコマンドを登録します。
    ///
    /// Discord から受け取った選択対象を、追加の通信をせずハンドラーに渡します。
    /// 名前は 1〜32 文字で、空白や大文字を使えます。
    ///
    /// # エラー
    ///
    /// 無効な名前、または同じ名前のメッセージコマンドを登録済みの場合に失敗します。
    pub fn register_message_menu<E>(
        &mut self,
        name: &str,
        handler: impl for<'a> AsyncFn(&mut SlashContext<'a>, &'a Message) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let invoke = async move |ctx: &mut SlashContext<'_>, hooks: &InvocationHooks| {
            let target = resolve_message(ctx.data).ok_or(Error::CommandTargetUnavailable)?;
            let handler = &handler;
            hooks
                .run(ctx, async move |ctx: &mut SlashContext<'_>| {
                    handler(ctx, target).await.map_err(handler_error)
                })
                .await
        };
        self.menus.insert(
            MenuKind::Message,
            name,
            Box::new(PreparedSlashHandler(invoke)),
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum MenuKind {
    User,
    Message,
}

impl MenuKind {
    pub(super) fn from_command_type(kind: CommandType) -> Option<Self> {
        match kind {
            CommandType::User => Some(Self::User),
            CommandType::Message => Some(Self::Message),
            _ => None,
        }
    }

    fn handler_kind(self) -> HandlerKind {
        match self {
            Self::User => HandlerKind::UserCommand,
            Self::Message => HandlerKind::MessageCommand,
        }
    }
}

impl From<MenuKind> for CommandType {
    fn from(kind: MenuKind) -> Self {
        match kind {
            MenuKind::User => Self::User,
            MenuKind::Message => Self::Message,
        }
    }
}

#[derive(Default)]
pub(super) struct MenuCommands {
    users: BTreeMap<String, Box<dyn SlashHandler>>,
    messages: BTreeMap<String, Box<dyn SlashHandler>>,
}

impl MenuCommands {
    pub(super) fn len(&self) -> usize {
        self.users.len() + self.messages.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.users.is_empty() && self.messages.is_empty()
    }

    pub(super) fn names(&self) -> impl Iterator<Item = (MenuKind, &str)> {
        self.users
            .keys()
            .map(|name| (MenuKind::User, name.as_str()))
            .chain(
                self.messages
                    .keys()
                    .map(|name| (MenuKind::Message, name.as_str())),
            )
    }

    fn entries_mut(&mut self, kind: MenuKind) -> &mut BTreeMap<String, Box<dyn SlashHandler>> {
        match kind {
            MenuKind::User => &mut self.users,
            MenuKind::Message => &mut self.messages,
        }
    }

    pub(super) fn contains(&self, kind: MenuKind, name: &str) -> bool {
        match kind {
            MenuKind::User => self.users.contains_key(name),
            MenuKind::Message => self.messages.contains_key(name),
        }
    }

    fn insert(&mut self, kind: MenuKind, name: &str, handler: Box<dyn SlashHandler>) -> Result<()> {
        command::name(name)?;
        match self.entries_mut(kind).entry(name.into()) {
            Entry::Vacant(entry) => {
                entry.insert(handler);
                Ok(())
            }
            Entry::Occupied(_) => Err(RegistrationError::Duplicate {
                kind: kind.handler_kind(),
                name: name.into(),
            }
            .into()),
        }
    }

    pub(super) fn remove(&mut self, kind: MenuKind, name: &str) -> Option<Box<dyn SlashHandler>> {
        self.entries_mut(kind).remove(name)
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum ReplacementCommand<'a> {
    ChatInput(&'a Command),
    Menu(CommandPayload<'a>),
}

fn replacement_payload(commands: &[Command]) -> Vec<ReplacementCommand<'_>> {
    commands
        .iter()
        .map(|command| {
            if command.kind == CommandType::ChatInput {
                ReplacementCommand::ChatInput(command)
            } else {
                ReplacementCommand::Menu(CommandPayload::from(command))
            }
        })
        .collect()
}

fn replacement_request(
    application_id: Id<ApplicationMarker>,
    scope: Scope,
    commands: &[Command],
) -> Result<Request> {
    let application_id = application_id.get();
    let route = match scope {
        Scope::Global => Route::SetGlobalCommands { application_id },
        Scope::Guild(guild) => Route::SetGuildCommands {
            application_id,
            guild_id: guild.get(),
        },
    };
    Ok(Request::builder(&route)
        .json(&replacement_payload(commands))
        .build()?)
}

pub(super) async fn replace_application_commands(
    http: &Client,
    application_id: Id<ApplicationMarker>,
    scope: Scope,
    commands: &[Command],
) -> Result<Vec<Command>> {
    let request = replacement_request(application_id, scope, commands)?;
    Ok(http
        .request::<ListBody<Command>>(request)
        .await?
        .models()
        .await?)
}

#[cfg(test)]
mod tests {
    use super::{
        CommandType, Id, MenuKind, Message, Result, Scope, SlashCommands, SlashContext,
        UserMenuTarget, replacement_request,
    };
    use crate::slash::settings::settings_request;
    use twilight_http::request::Method;

    #[test]
    fn create_menu_uses_the_scoped_twilight_route_and_typed_payload() -> Result<()> {
        for (scope, path) in [
            (Scope::Global, "applications/1/commands"),
            (Scope::Guild(Id::new(2)), "applications/1/guilds/2/commands"),
        ] {
            for (kind, wire_type) in [(MenuKind::User, 2), (MenuKind::Message, 3)] {
                let menu = SlashCommands::new(scope).menu_model(kind, "Menu")?;
                let request = settings_request(Id::new(1), scope, &menu)?;
                assert_eq!(request.method(), Method::Post);
                assert_eq!(request.path(), path);
                let body: serde_json::Value =
                    serde_json::from_slice(request.body().unwrap()).unwrap();
                assert_eq!(body, serde_json::json!({"name": "Menu", "type": wire_type}));
            }
        }
        Ok(())
    }

    #[test]
    fn bulk_overwrite_omits_descriptions_and_options_for_menus() -> Result<()> {
        let mut commands = SlashCommands::global().replace_commands();
        commands.register(
            "ping",
            "pong",
            vec![],
            async |_: &mut SlashContext<'_>| -> Result<()> { Ok(()) },
        )?;
        commands.register_user_menu(
            "Menu",
            async |_: &mut SlashContext<'_>, _: UserMenuTarget<'_>| -> Result<()> { Ok(()) },
        )?;
        commands.register_message_menu(
            "Menu",
            async |_: &mut SlashContext<'_>, _: &Message| -> Result<()> { Ok(()) },
        )?;
        let models = commands.replacement_commands()?;
        let slash = serde_json::to_value(&models[0]).unwrap();
        for (scope, path) in [
            (Scope::Global, "applications/1/commands"),
            (Scope::Guild(Id::new(2)), "applications/1/guilds/2/commands"),
        ] {
            let request = replacement_request(Id::new(1), scope, &models)?;
            assert_eq!(request.method(), Method::Put);
            assert_eq!(request.path(), path);
            let json: serde_json::Value = serde_json::from_slice(request.body().unwrap()).unwrap();
            let entries = json.as_array().unwrap();
            assert_eq!(entries[0], slash);
            assert_eq!(
                entries[1],
                serde_json::json!({"name": "Menu", "type": u8::from(CommandType::User)})
            );
            assert_eq!(
                entries[2],
                serde_json::json!({"name": "Menu", "type": u8::from(CommandType::Message)})
            );
        }
        Ok(())
    }

    #[test]
    fn bulk_overwrite_serializes_localized_menu_without_slash_fields() -> Result<()> {
        use crate::CommandSettings;
        use serde_json::{Value, json};
        use twilight_model::{
            application::interaction::InteractionContextType, oauth::ApplicationIntegrationType,
        };

        let mut slash = SlashCommands::global().replace_commands();
        slash.register_user_menu(
            "Lookup",
            async |_: &mut SlashContext<'_>, _: UserMenuTarget<'_>| -> Result<()> { Ok(()) },
        )?;
        slash.set_command_settings(
            CommandType::User,
            "Lookup",
            CommandSettings::new()
                .name_localization("ja", "ユーザー検索")
                .contexts([InteractionContextType::PrivateChannel])
                .install_types([ApplicationIntegrationType::UserInstall])
                .nsfw(true),
        )?;
        let request =
            replacement_request(Id::new(1), Scope::Global, &slash.replacement_commands()?)?;
        let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
        assert_eq!(
            body,
            json!([{
                "type": 2, "name": "Lookup", "name_localizations": {"ja": "ユーザー検索"},
                "contexts": [2], "integration_types": [1], "nsfw": true
            }])
        );
        Ok(())
    }
}
