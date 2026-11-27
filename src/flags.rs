//! Discord が数値やビットで返す権限・フラグ・設定を、名前で扱う補助。
//!
//! Twilight はこれらをすでに型（ビットフラグと列挙型）にしている。ここで足すのは、
//! 名前で調べるメソッド、プリセット、人が読める名前（日本語）だけで、型は作らない。
//! 名前の表は 1 か所に書き、メソッドと日本語名はそこからマクロで作る。

use twilight_model::{
    channel::{ChannelFlags, ChannelType, message::MessageFlags},
    guild::{
        DefaultMessageNotificationLevel, ExplicitContentFilter, MfaLevel, NSFWLevel, Permissions,
        PremiumTier, SystemChannelFlags, VerificationLevel,
    },
    user::UserFlags,
};

mod sealed {
    pub trait Sealed {}
}

/// 設定の列挙型に、人が読める名前を付けます。
///
/// 未知の値には「不明」を返します。Discord が値を追加しても、表示が壊れません。
pub trait Label: sealed::Sealed + Copy {
    /// 日本語の名前を返します。
    fn label(self) -> &'static str;
}

macro_rules! labels {
    ($ty:ty { $($pattern:pat => $label:literal,)+ }) => {
        impl sealed::Sealed for $ty {}

        impl Label for $ty {
            fn label(self) -> &'static str {
                match self {
                    $($pattern => $label,)+
                    _ => "不明",
                }
            }
        }
    };
}

labels!(VerificationLevel {
    VerificationLevel::None => "無し",
    VerificationLevel::Low => "低（メール認証済み）",
    VerificationLevel::Medium => "中（登録から 5 分以上）",
    VerificationLevel::High => "高（参加から 10 分以上）",
    VerificationLevel::VeryHigh => "最高（電話番号認証済み）",
});
labels!(DefaultMessageNotificationLevel {
    DefaultMessageNotificationLevel::All => "すべてのメッセージ",
    DefaultMessageNotificationLevel::Mentions => "メンションのみ",
});
labels!(ExplicitContentFilter {
    ExplicitContentFilter::None => "無効",
    ExplicitContentFilter::MembersWithoutRole => "ロールのないメンバーのメッセージ",
    ExplicitContentFilter::AllMembers => "すべてのメンバーのメッセージ",
});
labels!(MfaLevel {
    MfaLevel::None => "不要",
    MfaLevel::Elevated => "必要",
});
labels!(NSFWLevel {
    NSFWLevel::Default => "既定",
    NSFWLevel::Explicit => "成人向け",
    NSFWLevel::Safe => "全年齢",
    NSFWLevel::AgeRestricted => "年齢制限",
});
labels!(PremiumTier {
    PremiumTier::None => "なし",
    PremiumTier::Tier1 => "レベル 1",
    PremiumTier::Tier2 => "レベル 2",
    PremiumTier::Tier3 => "レベル 3",
});
labels!(ChannelType {
    ChannelType::GuildText => "テキスト",
    ChannelType::Private => "DM",
    ChannelType::GuildVoice => "ボイス",
    ChannelType::Group => "グループ DM",
    ChannelType::GuildCategory => "カテゴリー",
    ChannelType::GuildAnnouncement => "アナウンス",
    ChannelType::AnnouncementThread => "アナウンスのスレッド",
    ChannelType::PublicThread => "公開スレッド",
    ChannelType::PrivateThread => "プライベートスレッド",
    ChannelType::GuildStageVoice => "ステージ",
    ChannelType::GuildDirectory => "ディレクトリ",
    ChannelType::GuildForum => "フォーラム",
    ChannelType::GuildMedia => "メディア",
});

/// フラグの型ごとに、名前で調べるトレイトを作る。
///
/// 表の 1 行が、メソッドの名前（定数名の小文字）と日本語名を決める。
macro_rules! named_flags {
    (
        $ty:ty, $(#[$meta:meta])* $ext:ident, $table:ident {
            $($konst:ident => $get:ident, $label:literal;)+
        }
    ) => {
        const $table: &[($ty, &str, &str)] =
            &[$((<$ty>::$konst, stringify!($konst), $label),)+];

        $(#[$meta])*
        pub trait $ext: sealed::Sealed + Copy {
            $(
                #[doc = concat!("`", stringify!($konst), "` を持っているかを返します。")]
                fn $get(self) -> bool;
            )+

            /// 持っているものの定数名（`MANAGE_MESSAGES` など）を、定義順に返します。
            fn names(self) -> impl Iterator<Item = &'static str>;

            /// 持っているものの日本語名を、定義順に返します。
            fn labels(self) -> impl Iterator<Item = &'static str>;

            /// 名前の表にないビットを返します。Twilight や Discord が追加した、未対応の値です。
            fn unknown_bits(self) -> u64;
        }

        impl sealed::Sealed for $ty {}

        impl $ext for $ty {
            $(
                fn $get(self) -> bool {
                    self.contains(<$ty>::$konst)
                }
            )+

            fn names(self) -> impl Iterator<Item = &'static str> {
                $table
                    .iter()
                    .filter(move |(flag, ..)| self.contains(*flag))
                    .map(|(_, name, _)| *name)
            }

            fn labels(self) -> impl Iterator<Item = &'static str> {
                $table
                    .iter()
                    .filter(move |(flag, ..)| self.contains(*flag))
                    .map(|(_, _, label)| *label)
            }

            fn unknown_bits(self) -> u64 {
                let known = $table
                    .iter()
                    .fold(0, |bits, (flag, ..)| bits | u64::from(flag.bits()));
                u64::from(self.bits()) & !known
            }
        }
    };
}

named_flags!(Permissions,
    /// 権限を、名前で調べます。
    ///
    /// `perms.manage_messages()` のように、定数名の小文字で呼べます。
    /// 更新には、ビットフラグ標準の `insert`、`remove`、`set` を使います。
    PermissionsExt, PERMISSION_TABLE {
        CREATE_INVITE => create_invite, "招待を作成";
        KICK_MEMBERS => kick_members, "メンバーをキック";
        BAN_MEMBERS => ban_members, "メンバーを BAN";
        ADMINISTRATOR => administrator, "管理者";
        MANAGE_CHANNELS => manage_channels, "チャンネルの管理";
        MANAGE_GUILD => manage_guild, "サーバーの管理";
        ADD_REACTIONS => add_reactions, "リアクションの追加";
        VIEW_AUDIT_LOG => view_audit_log, "監査ログを表示";
        PRIORITY_SPEAKER => priority_speaker, "優先スピーカー";
        STREAM => stream, "配信";
        VIEW_CHANNEL => view_channel, "チャンネルを見る";
        SEND_MESSAGES => send_messages, "メッセージを送信";
        SEND_TTS_MESSAGES => send_tts_messages, "読み上げメッセージを送信";
        MANAGE_MESSAGES => manage_messages, "メッセージの管理";
        EMBED_LINKS => embed_links, "埋め込みリンク";
        ATTACH_FILES => attach_files, "ファイルを添付";
        READ_MESSAGE_HISTORY => read_message_history, "メッセージ履歴を読む";
        MENTION_EVERYONE => mention_everyone, "全員へのメンション";
        USE_EXTERNAL_EMOJIS => use_external_emojis, "外部の絵文字を使用";
        VIEW_GUILD_INSIGHTS => view_guild_insights, "サーバーインサイトを表示";
        CONNECT => connect, "接続";
        SPEAK => speak, "発言";
        MUTE_MEMBERS => mute_members, "メンバーをミュート";
        DEAFEN_MEMBERS => deafen_members, "メンバーのスピーカーをミュート";
        MOVE_MEMBERS => move_members, "メンバーを移動";
        USE_VAD => use_vad, "音声検出を使用";
        CHANGE_NICKNAME => change_nickname, "ニックネームの変更";
        MANAGE_NICKNAMES => manage_nicknames, "ニックネームの管理";
        MANAGE_ROLES => manage_roles, "ロールの管理";
        MANAGE_WEBHOOKS => manage_webhooks, "ウェブフックの管理";
        MANAGE_GUILD_EXPRESSIONS => manage_guild_expressions, "エクスプレッションの管理";
        USE_SLASH_COMMANDS => use_slash_commands, "アプリコマンドを使用";
        REQUEST_TO_SPEAK => request_to_speak, "スピーカー参加をリクエスト";
        MANAGE_EVENTS => manage_events, "イベントの管理";
        MANAGE_THREADS => manage_threads, "スレッドの管理";
        CREATE_PUBLIC_THREADS => create_public_threads, "公開スレッドの作成";
        CREATE_PRIVATE_THREADS => create_private_threads, "プライベートスレッドの作成";
        USE_EXTERNAL_STICKERS => use_external_stickers, "外部のスタンプを使用";
        SEND_MESSAGES_IN_THREADS => send_messages_in_threads, "スレッドでメッセージを送信";
        USE_EMBEDDED_ACTIVITIES => use_embedded_activities, "アクティビティを使用";
        MODERATE_MEMBERS => moderate_members, "メンバーをタイムアウト";
        VIEW_CREATOR_MONETIZATION_ANALYTICS => view_creator_monetization_analytics, "収益化アナリティクスを表示";
        USE_SOUNDBOARD => use_soundboard, "サウンドボードを使用";
        CREATE_GUILD_EXPRESSIONS => create_guild_expressions, "エクスプレッションの作成";
        CREATE_EVENTS => create_events, "イベントの作成";
        USE_EXTERNAL_SOUNDS => use_external_sounds, "外部のサウンドを使用";
        SEND_VOICE_MESSAGES => send_voice_messages, "ボイスメッセージを送信";
        SEND_POLLS => send_polls, "投票の作成";
        USE_EXTERNAL_APPS => use_external_apps, "外部のアプリを使用";
        PIN_MESSAGES => pin_messages, "メッセージのピン留め";
        BYPASS_SLOWMODE => bypass_slowmode, "スローモードの影響を受けない";
    }
);

named_flags!(MessageFlags,
    /// メッセージのフラグを、名前で調べます。
    MessageFlagsExt, MESSAGE_FLAG_TABLE {
        CROSSPOSTED => crossposted, "他のサーバーに公開済み";
        IS_CROSSPOST => is_crosspost, "他のサーバーから公開されたメッセージ";
        SUPPRESS_EMBEDS => suppress_embeds, "埋め込みを非表示";
        SOURCE_MESSAGE_DELETED => source_message_deleted, "元のメッセージが削除済み";
        URGENT => urgent, "緊急";
        HAS_THREAD => has_thread, "スレッドあり";
        EPHEMERAL => ephemeral, "本人にだけ表示";
        LOADING => loading, "応答待ち";
        FAILED_TO_MENTION_SOME_ROLES_IN_THREAD => failed_to_mention_some_roles_in_thread, "スレッドで一部のロールにメンションできなかった";
        SUPPRESS_NOTIFICATIONS => suppress_notifications, "通知なし";
        IS_VOICE_MESSAGE => is_voice_message, "ボイスメッセージ";
        HAS_SNAPSHOT => has_snapshot, "転送されたメッセージ";
        IS_COMPONENTS_V2 => is_components_v2, "Components V2";
    }
);

named_flags!(UserFlags,
    /// ユーザーの公開フラグ（バッジなど）を、名前で調べます。
    UserFlagsExt, USER_FLAG_TABLE {
        STAFF => staff, "Discord 社員";
        PARTNER => partner, "パートナー";
        HYPESQUAD => hypesquad, "ハイプスクワッドイベント";
        BUG_HUNTER_LEVEL_1 => bug_hunter_level_1, "バグハンター（レベル 1）";
        HYPESQUAD_ONLINE_HOUSE_1 => hypesquad_online_house_1, "ハイプスクワッド（ブレイブリー）";
        HYPESQUAD_ONLINE_HOUSE_2 => hypesquad_online_house_2, "ハイプスクワッド（ブリリアンス）";
        HYPESQUAD_ONLINE_HOUSE_3 => hypesquad_online_house_3, "ハイプスクワッド（バランス）";
        PREMIUM_EARLY_SUPPORTER => premium_early_supporter, "初期サポーター";
        TEAM_PSEUDO_USER => team_pseudo_user, "チームユーザー";
        BUG_HUNTER_LEVEL_2 => bug_hunter_level_2, "バグハンター（レベル 2）";
        VERIFIED_BOT => verified_bot, "認証済み Bot";
        VERIFIED_DEVELOPER => verified_developer, "認証済み Bot 開発者";
        CERTIFIED_MODERATOR => certified_moderator, "認定モデレーター";
        BOT_HTTP_INTERACTIONS => bot_http_interactions, "HTTP で Interaction を受ける Bot";
        ACTIVE_DEVELOPER => active_developer, "現役の開発者";
    }
);

named_flags!(SystemChannelFlags,
    /// サーバーのシステムチャンネルのフラグを、名前で調べます。
    SystemChannelFlagsExt, SYSTEM_CHANNEL_FLAG_TABLE {
        SUPPRESS_JOIN_NOTIFICATIONS => suppress_join_notifications, "参加の通知を表示しない";
        SUPPRESS_PREMIUM_SUBSCRIPTIONS => suppress_premium_subscriptions, "ブーストの通知を表示しない";
        SUPPRESS_GUILD_REMINDER_NOTIFICATIONS => suppress_guild_reminder_notifications, "サーバー設定のヒントを表示しない";
        SUPPRESS_JOIN_NOTIFICATION_REPLIES => suppress_join_notification_replies, "参加通知へのスタンプ返信を表示しない";
        SUPPRESS_ROLE_SUBSCRIPTION_PURCHASE_NOTIFICATIONS => suppress_role_subscription_purchase_notifications, "ロール購読の購入通知を表示しない";
        SUPPRESS_ROLE_SUBSCRIPTION_PURCHASE_NOTIFICATION_REPLIES => suppress_role_subscription_purchase_notification_replies, "購入通知へのスタンプ返信を表示しない";
    }
);

named_flags!(ChannelFlags,
    /// チャンネルのフラグを、名前で調べます。
    ChannelFlagsExt, CHANNEL_FLAG_TABLE {
        PINNED => pinned, "固定済み";
        REQUIRE_TAG => require_tag, "タグが必須";
        HIDE_MEDIA_DOWNLOAD_OPTIONS => hide_media_download_options, "メディアのダウンロードを隠す";
        CHANNEL_OBFUSCATED => channel_obfuscated, "難読化されたチャンネル";
        IS_SPOILER_CHANNEL => is_spoiler_channel, "ネタバレ扱いのチャンネル";
    }
);

/// `Permissions` のプリセットです。
///
/// 目的ごとの、よく使う権限のまとまりを返します。`PermissionsExt` と同じ
/// `use disnana_akari::prelude::*;` で使えます。
pub trait PermissionPresets: sealed::Sealed {
    /// テキストチャンネルで使う権限のまとまりです。管理系の権限（メッセージの管理など）を含みます。
    fn text() -> Self;
    /// ボイスチャンネルで使う権限のまとまりです。メンバーの管理（ミュート、移動）を含みます。
    fn voice() -> Self;
    /// モデレーションに使う権限のまとまりです。
    fn moderator() -> Self;
}

impl PermissionPresets for Permissions {
    fn text() -> Self {
        Self::VIEW_CHANNEL
            | Self::SEND_MESSAGES
            | Self::SEND_TTS_MESSAGES
            | Self::MANAGE_MESSAGES
            | Self::EMBED_LINKS
            | Self::ATTACH_FILES
            | Self::READ_MESSAGE_HISTORY
            | Self::MENTION_EVERYONE
            | Self::USE_EXTERNAL_EMOJIS
            | Self::ADD_REACTIONS
            | Self::USE_SLASH_COMMANDS
            | Self::MANAGE_THREADS
            | Self::CREATE_PUBLIC_THREADS
            | Self::CREATE_PRIVATE_THREADS
            | Self::SEND_MESSAGES_IN_THREADS
            | Self::USE_EXTERNAL_STICKERS
            | Self::SEND_VOICE_MESSAGES
            | Self::SEND_POLLS
            | Self::USE_EXTERNAL_APPS
            | Self::PIN_MESSAGES
            | Self::BYPASS_SLOWMODE
    }

    fn voice() -> Self {
        Self::VIEW_CHANNEL
            | Self::CONNECT
            | Self::SPEAK
            | Self::STREAM
            | Self::USE_VAD
            | Self::PRIORITY_SPEAKER
            | Self::MUTE_MEMBERS
            | Self::DEAFEN_MEMBERS
            | Self::MOVE_MEMBERS
            | Self::USE_EMBEDDED_ACTIVITIES
            | Self::USE_SOUNDBOARD
            | Self::USE_EXTERNAL_SOUNDS
            | Self::REQUEST_TO_SPEAK
    }

    fn moderator() -> Self {
        Self::KICK_MEMBERS
            | Self::BAN_MEMBERS
            | Self::MODERATE_MEMBERS
            | Self::MANAGE_MESSAGES
            | Self::MANAGE_NICKNAMES
            | Self::MANAGE_THREADS
            | Self::VIEW_AUDIT_LOG
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 表が、Twilight の持つ全ビットをちょうど 1 回ずつ覆っていることを確かめる。
    /// Twilight が新しいフラグを足したときは、ここで失敗して気づける。
    macro_rules! assert_table_is_complete {
        ($table:expr, $all:expr) => {{
            let mut seen = 0_u64;
            for (flag, name, label) in $table {
                let bits = u64::from(flag.bits());
                assert_eq!(bits.count_ones(), 1, "{name} が 1 ビットではありません");
                assert_eq!(seen & bits, 0, "{name} が重複しています");
                assert!(!label.is_empty(), "{name} の日本語名が空です");
                seen |= bits;
            }
            assert_eq!(seen, u64::from($all.bits()), "表にないフラグがあります");
        }};
    }

    #[test]
    fn tables_cover_every_twilight_flag() {
        assert_table_is_complete!(PERMISSION_TABLE, Permissions::all());
        assert_table_is_complete!(MESSAGE_FLAG_TABLE, MessageFlags::all());
        assert_table_is_complete!(USER_FLAG_TABLE, UserFlags::all());
        assert_table_is_complete!(SYSTEM_CHANNEL_FLAG_TABLE, SystemChannelFlags::all());
        assert_table_is_complete!(CHANNEL_FLAG_TABLE, ChannelFlags::all());
    }

    #[test]
    fn labels_and_names_follow_the_table_order() {
        let perms = Permissions::MANAGE_MESSAGES | Permissions::KICK_MEMBERS;
        assert_eq!(
            perms.names().collect::<Vec<_>>(),
            ["KICK_MEMBERS", "MANAGE_MESSAGES"]
        );
        assert_eq!(
            perms.labels().collect::<Vec<_>>(),
            ["メンバーをキック", "メッセージの管理"]
        );
        assert!(perms.manage_messages());
        assert!(!perms.ban_members());
    }

    #[test]
    fn unknown_bits_are_reported_not_dropped() {
        let perms = Permissions::from_bits_retain(Permissions::SEND_MESSAGES.bits() | (1 << 62));
        assert_eq!(perms.unknown_bits(), 1 << 62);
        assert_eq!(perms.labels().collect::<Vec<_>>(), ["メッセージを送信"]);
        assert_eq!(Permissions::all().unknown_bits(), 0);
    }

    #[test]
    fn presets_contain_only_known_permissions_and_missing_is_listed() {
        for preset in [
            Permissions::text(),
            Permissions::voice(),
            Permissions::moderator(),
        ] {
            assert_eq!(preset.unknown_bits(), 0);
            assert!(!preset.is_empty());
        }
        let missing = Permissions::moderator() - Permissions::KICK_MEMBERS;
        assert!(!missing.kick_members());
        assert!(missing.ban_members());
    }

    #[test]
    fn enums_have_labels_and_unknown_values_do_not_break_display() {
        assert_eq!(VerificationLevel::High.label(), "高（参加から 10 分以上）");
        assert_eq!(ChannelType::GuildForum.label(), "フォーラム");
        assert_eq!(ChannelType::Unknown(250).label(), "不明");
        assert_eq!(PremiumTier::Tier2.label(), "レベル 2");
    }
}
