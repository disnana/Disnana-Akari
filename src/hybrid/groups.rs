use std::sync::Arc;

use twilight_model::application::command::CommandOption;

use super::{
    CommandContext, HybridArguments, HybridContext, InvocationHooks, SlashContext, TypedHandler,
};
use crate::{
    CommandGroup, Commands, Result, SlashCommands, TextCommandGroup, commands::PreparedTextHandler,
    slash::CommandOptionLocalizations,
};

/// テキストとスラッシュで同じグループを登録します。
///
/// クロージャ内でサブコマンドを登録します。スラッシュを使わない場合は `None` を指定します。
/// その場合もグループの名前、深さ、件数は Discord のスラッシュコマンドの制約に従います。
///
/// # エラー
///
/// 重複する名前、空のグループ、無効な定義、またはクロージャが返したエラーで失敗します。
/// エラーの場合はどちらのコマンド一覧にも変更を加えません。
pub fn register_hybrid_group(
    commands: &mut Commands,
    slash: Option<&mut SlashCommands>,
    name: &str,
    description: &str,
    configure: impl FnOnce(&mut HybridCommandGroup<'_>) -> Result<()>,
) -> Result<()> {
    register_hybrid_group_with_aliases(commands, slash, name, description, &[], configure)
}

/// テキスト側のグループに別名も付けて、両方のコマンド一覧へ登録します。
///
/// スラッシュでは正式名を使います。制約は [`register_hybrid_group`] と同じです。
///
/// # エラー
///
/// 名前や別名が重複している場合や、グループの定義が無効な場合に失敗します。
/// エラーの場合はどちらのコマンド一覧にも変更を加えません。
pub fn register_hybrid_group_with_aliases(
    commands: &mut Commands,
    slash: Option<&mut SlashCommands>,
    name: &str,
    description: &str,
    aliases: &[&str],
    configure: impl FnOnce(&mut HybridCommandGroup<'_>) -> Result<()>,
) -> Result<()> {
    commands.ensure_available_with_aliases(name, aliases)?;
    if let Some(slash) = slash.as_deref() {
        slash.ensure_hybrid_group_available(name)?;
    }

    let mut staged_text = commands.stage_group();
    let mut staged_slash = slash
        .as_deref()
        .map_or_else(SlashCommands::global, SlashCommands::stage_hybrid_group);
    configure(&mut HybridCommandGroup {
        text: staged_text.group_with_aliases(name, aliases)?,
        slash: staged_slash.group(name, description)?,
    })?;
    staged_slash.validate_staged_hybrid_group(name)?;

    // 登録先は排他的に借りています。検証後に名前が重複することはありません。
    commands.append_staged_group(staged_text);
    if let Some(slash) = slash {
        slash.append_staged_hybrid_group(staged_slash);
    }
    Ok(())
}

/// [`register_hybrid_group`] のクロージャで、両方の呼び出し方をまとめて設定します。
///
/// Discord と同じく、最上位グループにサブコマンドと中間グループを置けます。
/// 中間グループにはサブコマンドを登録できます。権限が必要なら、共通ハンドラーで
/// [`HybridContext::require_permissions`] を呼びます。
#[derive(Debug)]
pub struct HybridCommandGroup<'a> {
    text: TextCommandGroup<'a>,
    slash: CommandGroup<'a>,
}

impl HybridCommandGroup<'_> {
    /// この階層にあるスラッシュ側の子コマンド、または中間グループの翻訳を置き換えます。
    /// テキスト側の正式名や別名は変わりません。
    ///
    /// # エラー
    /// 名前が未登録、翻訳が無効、または同じ階層の名前と衝突した場合に失敗します。
    pub fn set_child_localizations(
        &mut self,
        name: &str,
        localizations: CommandOptionLocalizations,
    ) -> Result<()> {
        self.slash.set_child_localizations(name, localizations)
    }

    /// 型付きの引数を使うサブコマンドを登録します。
    ///
    /// 引数の仕様は [`register_typed_hybrid`](super::register_typed_hybrid) と同じです。
    ///
    /// # エラー
    ///
    /// 名前が使用済み、または定義が無効な場合に失敗します。
    pub fn subcommand<E>(
        &mut self,
        name: &str,
        description: &str,
        options: Vec<CommandOption>,
        handler: impl for<'a> AsyncFn(
            &mut HybridContext<'a>,
            HybridArguments<'a>,
        ) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.subcommand_with_aliases(name, description, &[], options, handler)
    }

    /// 型付きの引数を使うサブコマンドと、そのテキスト側の別名を登録します。
    ///
    /// # エラー
    ///
    /// 名前や別名が使用済み、または定義が無効な場合に失敗します。
    pub fn subcommand_with_aliases<E>(
        &mut self,
        name: &str,
        description: &str,
        aliases: &[&str],
        options: Vec<CommandOption>,
        handler: impl for<'a> AsyncFn(
            &mut HybridContext<'a>,
            HybridArguments<'a>,
        ) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.text.ensure_names_available(name, aliases)?;
        let typed = Arc::new(TypedHandler::new(options, handler)?);
        let slash_handler = Arc::clone(&typed);
        self.slash.subcommand_prepared(
            name,
            description,
            typed.schema.options().to_vec(),
            async move |ctx: &mut SlashContext<'_>, hooks: &InvocationHooks| {
                slash_handler.slash(ctx, hooks).await
            },
        )?;
        self.text.subcommand_handler(
            name,
            aliases,
            Box::new(PreparedTextHandler(async move |ctx: CommandContext<'_>| {
                typed.text(ctx).await
            })),
        )
    }

    /// サブコマンドをまとめる中間グループを追加します。
    ///
    /// # エラー
    ///
    /// 名前や説明が無効、名前が使用済み、またはすでに中間グループの中の場合に失敗します。
    pub fn group(&mut self, name: &str, description: &str) -> Result<HybridCommandGroup<'_>> {
        self.group_with_aliases(name, description, &[])
    }

    /// 中間グループと、そのテキスト側の別名を追加します。
    ///
    /// # エラー
    ///
    /// 名前や別名が使用済み、定義が無効、またはすでに中間グループの中の場合に失敗します。
    pub fn group_with_aliases(
        &mut self,
        name: &str,
        description: &str,
        aliases: &[&str],
    ) -> Result<HybridCommandGroup<'_>> {
        self.text.ensure_names_available(name, aliases)?;
        let slash = self.slash.group(name, description)?;
        let text = self.text.group_with_aliases(name, aliases)?;
        Ok(HybridCommandGroup { text, slash })
    }

    /// テキストでグループ名だけを入力したときの関数を登録します。
    ///
    /// スラッシュではグループ名だけで実行できないため、指定した名前のサブコマンドとして表示します。
    /// 引数はありません。続けて未登録の名前を入力しても、この関数は呼ばれません。
    ///
    /// # エラー
    ///
    /// すでに関数を登録済み、またはスラッシュ側の名前や説明が無効な場合に失敗します。
    pub fn fallback<E>(
        &mut self,
        slash_name: &str,
        description: &str,
        handler: impl for<'a> AsyncFn(
            &mut HybridContext<'a>,
            HybridArguments<'a>,
        ) -> std::result::Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Result<()>
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.text.ensure_fallback_available()?;
        let typed = Arc::new(TypedHandler::new(Vec::new(), handler)?);
        let slash_handler = Arc::clone(&typed);
        self.slash.subcommand_prepared(
            slash_name,
            description,
            Vec::new(),
            async move |ctx: &mut SlashContext<'_>, hooks: &InvocationHooks| {
                slash_handler.slash(ctx, hooks).await
            },
        )?;
        self.text.fallback_handler(Box::new(PreparedTextHandler(
            async move |ctx: CommandContext<'_>| typed.text(ctx).await,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Error, RegistrationError};

    async fn handler(_: &mut HybridContext<'_>, _: HybridArguments<'_>) -> Result<()> {
        Ok(())
    }

    #[test]
    fn rejects_destination_collisions_before_running_configure() -> Result<()> {
        let mut text = Commands::new("!")?;
        let mut slash = SlashCommands::global();
        text.register("taken", async |_| -> Result<()> { Ok(()) })?;
        slash.group("existing", "既存")?.subcommand(
            "leaf",
            "既存のコマンド",
            Vec::new(),
            async |_| -> Result<()> { Ok(()) },
        )?;
        let result = register_hybrid_group_with_aliases(
            &mut text,
            Some(&mut slash),
            "fresh",
            "説明",
            &["taken"],
            |_| panic!("destination alias collision must precede configure"),
        );
        assert!(result.is_err());
        slash.ensure_hybrid_group_available("fresh")?;
        let result =
            register_hybrid_group(&mut text, Some(&mut slash), "existing", "説明", |_| {
                panic!("slash collision must precede configure")
            });
        assert!(result.is_err());
        assert!(text.matches_message("!taken"));
        text.ensure_available("existing")
    }

    #[test]
    fn late_errors_and_invalid_tree_leave_no_root_or_aliases() -> Result<()> {
        let mut text = Commands::new("!")?;
        let mut slash = SlashCommands::global();
        let aborted = register_hybrid_group_with_aliases(
            &mut text,
            Some(&mut slash),
            "config",
            "設定",
            &["c"],
            |group| {
                group.subcommand("status", "確認", Vec::new(), handler)?;
                Err(Error::msg("abort"))
            },
        );
        assert!(aborted.is_err());
        text.ensure_available_with_aliases("config", &["c"])?;
        slash.ensure_hybrid_group_available("config")?;

        let empty = register_hybrid_group_with_aliases(
            &mut text,
            Some(&mut slash),
            "config",
            "設定",
            &["c"],
            |group| {
                group.subcommand("status", "確認", Vec::new(), handler)?;
                group.group("empty", "空")?;
                Ok(())
            },
        );
        assert!(
            matches!(empty, Err(Error::Registration(RegistrationError::EmptyHybridGroup { name })) if name == "config empty")
        );
        text.ensure_available_with_aliases("config", &["c"])?;
        slash.ensure_hybrid_group_available("config")?;
        assert!(matches!(
            register_hybrid_group(&mut text, None, "blank", "空", |_| Ok(())),
            Err(Error::Registration(RegistrationError::EmptyHybridGroup { name })) if name == "blank"
        ));

        let too_many =
            register_hybrid_group(&mut text, Some(&mut slash), "config", "設定", |group| {
                for i in 0..=25 {
                    group.subcommand(&format!("cmd{i}"), "処理", Vec::new(), handler)?;
                }
                Ok(())
            });
        assert!(too_many.is_err());
        text.ensure_available("config")?;
        slash.ensure_hybrid_group_available("config")
    }

    #[test]
    fn a_caught_builder_error_does_not_leave_one_path_registered() -> Result<()> {
        let mut text = Commands::new("!")?;
        let mut slash = SlashCommands::global();
        register_hybrid_group(&mut text, Some(&mut slash), "config", "設定", |group| {
            group.subcommand_with_aliases("first", "最初", &["a"], Vec::new(), handler)?;
            assert!(
                group
                    .subcommand_with_aliases("second", "次", &["a"], Vec::new(), handler)
                    .is_err()
            );
            assert!(group.subcommand("second", "", Vec::new(), handler).is_err());
            group.subcommand_with_aliases("second", "次", &["b"], Vec::new(), handler)?;

            group.fallback("status", "状態", handler)?;
            assert!(group.fallback("other", "重複", handler).is_err());
            group.subcommand("other", "追加", Vec::new(), handler)?;

            let mut nested = group.group("nested", "入れ子")?;
            assert!(nested.group("deep", "深過ぎ").is_err());
            nested.subcommand("deep", "末端", Vec::new(), handler)
        })?;
        for input in [
            "!config",
            "!config a",
            "!config b",
            "!config other",
            "!config nested deep",
        ] {
            assert!(text.matches_message(input));
        }
        assert!(!text.matches_message("!config status"));
        Ok(())
    }

    #[test]
    fn text_only_registration_accepts_a_valid_group_and_root_alias() -> Result<()> {
        let mut text = Commands::with_prefixes(&["!", "!!"])?;
        register_hybrid_group_with_aliases(&mut text, None, "config", "設定", &["c"], |group| {
            group.subcommand("status", "状態", Vec::new(), handler)
        })?;
        assert!(text.matches_message("!!c status"));
        assert!(!text.matches_message("!!c"));
        Ok(())
    }
}
