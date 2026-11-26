use std::{env, fs, io, path::Path};

use disnana_akari::model::id::{Id, marker::GuildMarker};
use disnana_akari::{Bot, Result};

// 丸括弧の (Bot, Option<...>) は 2 個の値をまとめたタプルです。
// Option は、スラッシュコマンド用のサーバー ID がなくても Bot を起動できるように使います。
pub fn load() -> Result<(Bot, Option<Id<GuildMarker>>)> {
    // env! はコンパイル時の環境変数を取り出します。この値は Cargo が与える
    // Cargo.toml のあるフォルダなので、RustRover の起動場所が違っても同じ .env を読めます。
    let env_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");

    // match は値の形によって処理を分けます。Ok は成功、Err は失敗です。
    let (bot, guild_id) = match fs::read_to_string(&env_path) {
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
            (bot, guild_id)
        }
        // Err に続く if は、この種類のエラーのときだけ、という追加条件です。
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let bot = Bot::from_env()?;
            let guild_id = env::var("DISCORD_GUILD_ID").ok();
            (bot, parse_guild_id(guild_id.as_deref())?)
        }
        Err(error) => {
            return Err(format!("{} を読み込めませんでした ({error})", env_path.display()).into());
        }
    };

    Ok((bot, guild_id))
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
