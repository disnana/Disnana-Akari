use std::{env, fs, io, path::Path};

use disnana_akari::model::id::{Id, marker::GuildMarker};
use disnana_akari::{Bot, Result};

// struct は、関連する値を名前付きでひとまとまりにします。
pub struct Settings {
    pub bot: Bot,
    pub guild_id: Option<Id<GuildMarker>>,
    pub replace_guild_commands: bool,
}

pub fn load() -> Result<Settings> {
    // env! はコンパイル時の環境変数を取り出します。この値は Cargo が与える
    // Cargo.toml のあるフォルダなので、RustRover の起動場所が違っても同じ .env を読めます。
    let env_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");

    // match は値の形によって処理を分けます。Ok は成功、Err は失敗です。
    let (bot, guild_id, replace_guild_commands) = match fs::read_to_string(&env_path) {
        Ok(settings) => {
            // &settings は文字列をコピーせず、一時的に借りて関数に渡す書き方です。
            let token = setting(&settings, "DISCORD_TOKEN");
            let bot = Bot::new(token.unwrap_or_default()).map_err(|_| {
                format!(
                    "{} の DISCORD_TOKEN に Bot トークンを設定してください",
                    env_path.display()
                )
            })?;

            let guild_id = parse_guild_id(setting(&settings, "DISCORD_GUILD_ID"))?;
            let replace = parse_replace(setting(&settings, "DISCORD_REPLACE_GUILD_COMMANDS"))?;
            (bot, guild_id, replace)
        }
        // Err に続く if は、この種類のエラーのときだけ、という追加条件です。
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let bot = Bot::from_env()?;
            let guild_id = env::var("DISCORD_GUILD_ID").ok();
            let replace = env::var("DISCORD_REPLACE_GUILD_COMMANDS").ok();
            (
                bot,
                parse_guild_id(guild_id.as_deref())?,
                parse_replace(replace.as_deref())?,
            )
        }
        Err(error) => {
            return Err(format!("{} を読み込めませんでした ({error})", env_path.display()).into());
        }
    };

    if replace_guild_commands && guild_id.is_none() {
        return Err("一括同期には DISCORD_GUILD_ID が必要です".into());
    }

    Ok(Settings {
        bot,
        guild_id,
        replace_guild_commands,
    })
}

// 'a は「戻す &str は settings の中のデータを借りている」と表すための名前です。
fn setting<'a>(settings: &'a str, name: &str) -> Option<&'a str> {
    // .env の各行を最初の = で分けて、探している名前に一致したら値を返します。
    // 最初に UTF-8 の BOM があれば取り除き、行の左右の空白は無視します。
    for line in settings.trim_start_matches('\u{feff}').lines() {
        // && は両方の条件を満たすか調べます。= がない行では右側を調べません。
        if let Some((key, value)) = line.split_once('=')
            && key.trim() == name
        {
            return Some(value.trim());
        }
    }

    None
}

fn parse_guild_id(value: Option<&str>) -> Result<Option<Id<GuildMarker>>> {
    match value.map(str::trim) {
        None | Some("") => Ok(None),
        // parse で文字列を型付きの Discord サーバー ID に変えます。
        Some(value) => Ok(Some(value.parse().map_err(
            |_| "DISCORD_GUILD_ID には数字の Discord サーバー ID を指定してください",
        )?)),
    }
}

fn parse_replace(value: Option<&str>) -> Result<bool> {
    // 明示的に true と書いた場合だけ、Discord 側のコマンド一覧を置き換えます。
    match value.map(str::trim) {
        None | Some("" | "false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => {
            Err("DISCORD_REPLACE_GUILD_COMMANDS には true または false を指定してください".into())
        }
    }
}
