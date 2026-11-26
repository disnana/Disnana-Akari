use std::{fs, io, path::Path};

use disnana_akari::{Bot, Result};

mod commands;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let env_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");
    let bot = match fs::read_to_string(&env_path) {
        Ok(settings) => {
            let token = settings
                .trim_start_matches('\u{feff}')
                .lines()
                .filter_map(|line| line.split_once('='))
                .find_map(|(key, value)| (key.trim() == "DISCORD_TOKEN").then_some(value));
            token
                .and_then(|value| Bot::new(value).ok())
                .ok_or_else(|| {
                    format!(
                        "{} の DISCORD_TOKEN に Bot トークンを設定してください",
                        env_path.display()
                    )
                })?
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Bot::from_env()?,
        Err(error) => {
            return Err(format!("{} を読み込めませんでした ({error})", env_path.display()).into());
        }
    };

    bot.run_commands(commands::register()?).await
}
