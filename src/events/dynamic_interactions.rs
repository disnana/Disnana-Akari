use crate::{
    ButtonContext, ChannelSelectContext, MentionableSelectContext, ModalContext, Result,
    RoleSelectContext, SelectContext, UserSelectContext, error::adapt_handler,
};

macro_rules! on_dynamic_component {
    ($method:ident, $register:ident, $context:ident, $doc:literal) => {
        #[doc = $doc]
        ///
        /// 既存の固定 ID は優先されます。ID は [`crate::ui::DynamicId`] で組み立て、
        /// ハンドラーでは渡された値に対するアクセス権限を確認してください。
        ///
        /// # エラー
        ///
        /// 名前空間が無効、または同じ種類で登録済みの場合に失敗します。
        pub fn $method<E>(
            &mut self,
            namespace: &str,
            handler: impl AsyncFn(&mut $context<'_>) -> std::result::Result<(), E>
            + Send
            + Sync
            + 'static,
        ) -> Result<()>
        where
            E: Into<Box<dyn std::error::Error + Send + Sync>>,
        {
            self.interaction_handlers.$register(
                namespace,
                adapt_handler!(handler, ctx: &mut $context<'_>),
            )
        }
    };
}

impl super::Events {
    on_dynamic_component!(
        on_dynamic_button,
        register_dynamic_button,
        ButtonContext,
        "名前空間を持つボタンを処理する関数を登録します。"
    );
    on_dynamic_component!(
        on_dynamic_select,
        register_dynamic_select,
        SelectContext,
        "名前空間を持つ文字列セレクトを処理する関数を登録します。"
    );
    on_dynamic_component!(
        on_dynamic_user_select,
        register_dynamic_user_select,
        UserSelectContext,
        "名前空間を持つユーザーセレクトを処理する関数を登録します。"
    );
    on_dynamic_component!(
        on_dynamic_role_select,
        register_dynamic_role_select,
        RoleSelectContext,
        "名前空間を持つロールセレクトを処理する関数を登録します。"
    );
    on_dynamic_component!(
        on_dynamic_channel_select,
        register_dynamic_channel_select,
        ChannelSelectContext,
        "名前空間を持つチャンネルセレクトを処理する関数を登録します。"
    );
    on_dynamic_component!(
        on_dynamic_mentionable_select,
        register_dynamic_mentionable_select,
        MentionableSelectContext,
        "名前空間を持つメンショナブルセレクトを処理する関数を登録します。"
    );
    on_dynamic_component!(
        on_dynamic_modal,
        register_dynamic_modal,
        ModalContext,
        "名前空間を持つモーダル送信を処理する関数を登録します。"
    );
}
