use dashmap::DashMap;
use eyre::Result;
use lazy_static::lazy_static;
use sqlx::{sqlite::SqlitePool, Row};
use std::fs;
use toml::Table;

lazy_static! {
    static ref DISCORD_TO_TELEGRAM_CACHE: DashMap<d::ChannelId, t::ChatId> = DashMap::new();
    static ref TELEGRAM_TO_DISCORD_CACHE: DashMap<t::ChatId, d::ChannelId> = DashMap::new();
}

const CHAT_MAPPING_FILE: &str = "chat_mappings.toml";
const MESSAGE_MAPPING_DB: &str = "messages.db";

pub async fn init_db() -> Result<SqlitePool> {
    use std::path::Path;

    // Create the database file if it doesn't exist
    if !Path::new(MESSAGE_MAPPING_DB).exists() {
        std::fs::File::create(MESSAGE_MAPPING_DB)?;
    }
    let pool = SqlitePool::connect("sqlite:messages.db").await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS message_mapping (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            discord_message_id BIGINT NOT NULL,
            telegram_message_id BIGINT NOT NULL,
            telegram_chat_id BIGINT NOT NULL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )",
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS reaction_mapping (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            discord_message_id BIGINT NOT NULL,
            telegram_message_id BIGINT NOT NULL,
            telegram_chat_id BIGINT NOT NULL,
            reactions TEXT NOT NULL DEFAULT '{}',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )",
    )
    .execute(&pool)
    .await?;

    load_chat_mappings()?;

    Ok(pool)
}

use crate::discord as d;
use crate::telegram as t;

pub async fn insert_mapping(
    pool: &SqlitePool,
    discord_message_id: d::MessageId,
    telegram_message_id: t::MessageId,
    telegram_chat_id: t::ChatId,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO message_mapping (discord_message_id, telegram_message_id, telegram_chat_id) VALUES (?, ?, ?)",
    )
    .bind(i64::from(discord_message_id))
    .bind(telegram_message_id.0 as i64)
    .bind(telegram_chat_id.0)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_telegram_message_id(
    pool: &SqlitePool,
    discord_message_id: d::MessageId,
) -> Result<Vec<t::MessageId>> {
    let result =
        sqlx::query("SELECT telegram_message_id FROM message_mapping WHERE discord_message_id = ?")
            .bind(i64::from(discord_message_id))
            .fetch_all(pool)
            .await?;

    Ok(result
        .into_iter()
        .map(|row| t::MessageId(row.get::<i64, _>(0) as i32))
        .collect())
}

pub async fn get_discord_message_id(
    pool: &SqlitePool,
    telegram_message_id: t::MessageId,
    telegram_chat_id: t::ChatId,
) -> Result<Vec<d::MessageId>> {
    let result = sqlx::query(
        "SELECT discord_message_id FROM message_mapping WHERE telegram_message_id = ? AND telegram_chat_id = ?",
    )
    .bind(telegram_message_id.0 as i64)
    .bind(telegram_chat_id.0)
    .fetch_all(pool)
    .await?;

    Ok(result
        .into_iter()
        .map(|row| d::MessageId::from(row.get::<i64, _>(0) as u64))
        .collect())
}

pub async fn delete_by_discord(
    pool: &SqlitePool,
    discord_message_id: d::MessageId,
) -> Result<Vec<t::MessageId>> {
    let result = sqlx::query(
        "DELETE FROM message_mapping WHERE discord_message_id = ? RETURNING telegram_message_id",
    )
    .bind(i64::from(discord_message_id))
    .fetch_all(pool)
    .await?;

    Ok(result
        .into_iter()
        .map(|row| t::MessageId(row.get::<i64, _>(0) as i32))
        .collect())
}

pub async fn delete_by_telegram(
    pool: &SqlitePool,
    telegram_message_id: t::MessageId,
    telegram_chat_id: t::ChatId,
) -> Result<Vec<d::MessageId>> {
    let result = sqlx::query(
        "DELETE FROM message_mapping WHERE telegram_message_id = ? AND telegram_chat_id = ? RETURNING discord_message_id",
    )
    .bind(telegram_message_id.0 as i64)
    .bind(telegram_chat_id.0)
    .fetch_all(pool)
    .await?;

    Ok(result
        .into_iter()
        .map(|row| d::MessageId::from(row.get::<i64, _>(0) as u64))
        .collect())
}

pub async fn insert_reaction_mapping(
    pool: &SqlitePool,
    discord_message_id: d::MessageId,
    telegram_message_id: t::MessageId,
    telegram_chat_id: t::ChatId,
    reactions: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT OR REPLACE INTO reaction_mapping (discord_message_id, telegram_message_id, telegram_chat_id, reactions) VALUES (?, ?, ?, ?)",
    )
    .bind(i64::from(discord_message_id))
    .bind(telegram_message_id.0 as i64)
    .bind(telegram_chat_id.0)
    .bind(reactions)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_telegram_reaction_message_id(
    pool: &SqlitePool,
    discord_message_id: d::MessageId,
) -> Result<Option<(t::MessageId, String)>> {
    let result = sqlx::query_as::<_, (i64, String)>(
        "SELECT telegram_message_id, reactions FROM reaction_mapping WHERE discord_message_id = ?",
    )
    .bind(i64::from(discord_message_id))
    .fetch_optional(pool)
    .await?;

    Ok(result.map(|(id, reactions)| (t::MessageId(id as i32), reactions)))
}

pub async fn get_discord_reaction_message_id(
    pool: &SqlitePool,
    telegram_message_id: t::MessageId,
    telegram_chat_id: t::ChatId,
) -> Result<Option<(d::MessageId, String)>> {
    let result = sqlx::query_as::<_, (i64, String)>(
        "SELECT discord_message_id, reactions FROM reaction_mapping WHERE telegram_message_id = ? AND telegram_chat_id = ?",
    )
    .bind(telegram_message_id.0 as i64)
    .bind(telegram_chat_id.0)
    .fetch_optional(pool)
    .await?;

    Ok(result.map(|(id, reactions)| (d::MessageId::from(id as u64), reactions)))
}

pub async fn update_telegram_reaction_mapping(
    pool: &SqlitePool,
    discord_message_id: d::MessageId,
    reactions: &str,
) -> Result<()> {
    sqlx::query("UPDATE reaction_mapping SET reactions = ? WHERE discord_message_id = ?")
        .bind(reactions)
        .bind(i64::from(discord_message_id))
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn update_discord_reaction_mapping(
    pool: &SqlitePool,
    telegram_message_id: t::MessageId,
    telegram_chat_id: t::ChatId,
    reactions: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE reaction_mapping SET reactions = ? WHERE telegram_message_id = ? AND telegram_chat_id = ?",
    )
    .bind(reactions)
    .bind(telegram_message_id.0 as i64)
    .bind(telegram_chat_id.0)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn remove_reaction_mapping_by_discord(
    pool: &SqlitePool,
    discord_message_id: d::MessageId,
) -> Result<()> {
    sqlx::query("DELETE FROM reaction_mapping WHERE discord_message_id = ?")
        .bind(i64::from(discord_message_id))
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn remove_reaction_mapping_by_telegram(
    pool: &SqlitePool,
    telegram_message_id: t::MessageId,
    telegram_chat_id: t::ChatId,
) -> Result<()> {
    sqlx::query(
        "DELETE FROM reaction_mapping WHERE telegram_message_id = ? AND telegram_chat_id = ?",
    )
    .bind(telegram_message_id.0 as i64)
    .bind(telegram_chat_id.0)
    .execute(pool)
    .await?;

    Ok(())
}

fn load_chat_mappings() -> Result<()> {
    if !std::path::Path::new(CHAT_MAPPING_FILE).exists() {
        fs::write(CHAT_MAPPING_FILE, "")?;
    }

    let content = fs::read_to_string(CHAT_MAPPING_FILE)?;
    let mappings: Table = toml::from_str(&content)?;

    for (discord_channel_id, telegram_chat_id) in mappings.iter() {
        let discord_channel_id = d::ChannelId::from(discord_channel_id.parse::<u64>()?);
        let telegram_chat_id = t::ChatId(telegram_chat_id.as_integer().unwrap() as i64);

        DISCORD_TO_TELEGRAM_CACHE.insert(discord_channel_id, telegram_chat_id);
        TELEGRAM_TO_DISCORD_CACHE.insert(telegram_chat_id, discord_channel_id);
    }

    Ok(())
}

fn save_chat_mappings() -> Result<()> {
    let mut mappings = Table::new();

    for entry in DISCORD_TO_TELEGRAM_CACHE.iter() {
        mappings.insert(
            entry.key().to_string(),
            toml::Value::Integer(entry.value().0),
        );
    }

    let toml_string = toml::to_string(&mappings)?;
    fs::write(CHAT_MAPPING_FILE, toml_string)?;

    Ok(())
}

pub async fn insert_chat_mapping(
    discord_channel_id: d::ChannelId,
    telegram_chat_id: t::ChatId,
) -> Result<()> {
    // Check if the mapping already exists
    if DISCORD_TO_TELEGRAM_CACHE.contains_key(&discord_channel_id) {
        return Ok(());
    }

    // Insert new mapping
    DISCORD_TO_TELEGRAM_CACHE.insert(discord_channel_id, telegram_chat_id);
    TELEGRAM_TO_DISCORD_CACHE.insert(telegram_chat_id, discord_channel_id);

    // Save the updated mappings to the TOML file
    save_chat_mappings()?;

    Ok(())
}

pub fn get_telegram_chat_id(discord_channel_id: d::ChannelId) -> Option<t::ChatId> {
    DISCORD_TO_TELEGRAM_CACHE
        .get(&discord_channel_id)
        .map(|v| *v)
}

pub fn get_discord_channel_id(telegram_chat_id: t::ChatId) -> Option<d::ChannelId> {
    TELEGRAM_TO_DISCORD_CACHE.get(&telegram_chat_id).map(|v| *v)
}
