use dashmap::DashMap;
use eyre::Result;
use lazy_static::lazy_static;
use sqlx::{sqlite::SqlitePool, Row};
use std::fs;
use toml::Table;

use crate::discord as d;
use crate::telegram as t;

#[derive(Copy, Clone, Debug)]
pub enum Hub {
    Server(d::GuildId),
    Category(d::GuildId, d::ChannelId),
}

lazy_static! {
    static ref DISCORD_TO_TELEGRAM_CACHE: DashMap<d::ChannelId, t::ChatId> = DashMap::new();
    static ref TELEGRAM_TO_DISCORD_CACHE: DashMap<t::ChatId, (d::ChannelId, Option<String>)> =
        DashMap::new();
    static ref ADMINS: tokio::sync::RwLock<Vec<d::UserId>> = vec![].into();
    static ref DISCORD_IMAGE_CHANNEL: tokio::sync::RwLock<Option<d::ChannelId>> = None.into();
    static ref HUBS: DashMap<String, Hub> = DashMap::new();
}

const CONFIG_FILE: &str = "config.toml";
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
            has_caption BOOLEAN NOT NULL DEFAULT 0,
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

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS telegram_chats (
            chat_id BIGINT PRIMARY KEY,
            title TEXT NOT NULL,
            is_member BOOLEAN NOT NULL DEFAULT 1,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )",
    )
    .execute(&pool)
    .await?;

    load_config().await?;

    Ok(pool)
}

pub async fn insert_mapping(
    pool: &SqlitePool,
    discord_message_id: d::MessageId,
    telegram_message_id: t::MessageId,
    telegram_chat_id: t::ChatId,
    has_caption: bool,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO message_mapping (discord_message_id, telegram_message_id, telegram_chat_id, has_caption) VALUES (?, ?, ?, ?)",
    )
    .bind(i64::from(discord_message_id))
    .bind(telegram_message_id.0 as i64)
    .bind(telegram_chat_id.0)
    .bind(has_caption)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_telegram_message_id(
    pool: &SqlitePool,
    discord_message_id: d::MessageId,
) -> Result<Vec<(t::MessageId, bool)>> {
    let result = sqlx::query(
        "SELECT telegram_message_id, has_caption FROM message_mapping WHERE discord_message_id = ?",
    )
    .bind(i64::from(discord_message_id))
    .fetch_all(pool)
    .await?;

    Ok(result
        .into_iter()
        .map(|row| {
            (
                t::MessageId(row.get::<i64, _>(0) as i32),
                row.get::<bool, _>(1),
            )
        })
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

async fn load_config() -> Result<()> {
    if !std::path::Path::new(CONFIG_FILE).exists() {
        fs::write(CONFIG_FILE, "")?;
    }

    let content = fs::read_to_string(CONFIG_FILE)?;
    let config: Table = toml::from_str(&content)?;

    // Load chat mappings
    if let Some(chat_mappings) = config.get("chat_mappings").and_then(|v| v.as_table()) {
        for (discord_channel_id, telegram_chat_id) in chat_mappings {
            let discord_channel_id = d::ChannelId::from(discord_channel_id.parse::<u64>()?);
            let vals = telegram_chat_id.as_array().unwrap();
            let telegram_chat_id = t::ChatId(vals[0].as_integer().unwrap() as i64);
            let webhook_url = vals.get(1).and_then(|v| v.as_str()).map(String::from);

            DISCORD_TO_TELEGRAM_CACHE.insert(discord_channel_id, telegram_chat_id);
            TELEGRAM_TO_DISCORD_CACHE.insert(telegram_chat_id, (discord_channel_id, webhook_url));
        }
    }

    if let Some(hubs) = config.get("hubs").and_then(|v| v.as_table()) {
        for (name, hub) in hubs {
            if let Some(guild_id) = hub.as_integer() {
                let guild_id = d::GuildId::from(guild_id as u64);
                if let Some(prev) = HUBS.insert(name.clone(), Hub::Server(guild_id)) {
                    log::warn!(
                        "Multiple hubs named \"{name}\": {prev:?} and Hub::Server({guild_id})"
                    );
                };
                continue;
            }
            if let Some(ids) = hub
                .as_array()
                .and_then(|v| v.iter().map(|i| i.as_integer()).collect::<Option<Vec<_>>>())
                .and_then(|v| <[i64; 2]>::try_from(v).ok())
            {
                let [guild_id, channel_id] = ids.map(|i| i as u64);
                let guild_id = d::GuildId::from(guild_id);
                let channel_id = d::ChannelId::from(channel_id);
                if let Some(prev) = HUBS.insert(name.clone(), Hub::Category(guild_id, channel_id)) {
                    log::warn!("Multiple hubs named \"{name}\": {prev:?} and Hub::Category({guild_id}, {channel_id})");
                };
                continue;
            }
            log::warn!("Invalid format for hub named \"{name}\": {hub:?}. Should be either a single guild id or a 2-item array of guild id and category id")
        }
    }

    // Load admins
    *ADMINS.write().await = config
        .get("options")
        .and_then(|t| t.get("admins"))
        .and_then(|v| v.as_array())
        .unwrap_or(&vec![])
        .into_iter()
        .filter_map(|u| u.as_integer())
        .map(|i| i as u64)
        .map(Into::into)
        .collect();

    *DISCORD_IMAGE_CHANNEL.write().await = config
        .get("options")
        .and_then(|t| t.get("image_channel"))
        .and_then(|v| v.as_integer())
        .map(|i| i as u64)
        .map(Into::into);

    Ok(())
}

async fn save_config() -> Result<()> {
    let mut mappings = Table::new();

    use toml::Value;

    fn int(x: impl Into<u64>) -> Value {
        Value::Integer(x.into() as i64)
    }

    for entry in TELEGRAM_TO_DISCORD_CACHE.iter() {
        let mut out = vec![Value::Integer(entry.key().0)];
        if let Some(webhook_url) = entry.value().1.clone() {
            out.push(Value::String(webhook_url));
        }
        mappings.insert(entry.value().0.to_string(), Value::Array(out));
    }

    let mut hubs = Table::new();

    for entry in &*HUBS {
        hubs.insert(
            entry.key().to_string(),
            match *entry.value() {
                Hub::Server(g) => int(g),
                Hub::Category(g, c) => Value::Array(vec![int(g), int(c)]),
            },
        );
    }

    let mut options = Table::new();
    options.insert(
        "admins".to_string(),
        toml::Value::Array(ADMINS.read().await.iter().copied().map(int).collect()),
    );
    if let Some(image_channel) = &*DISCORD_IMAGE_CHANNEL.read().await {
        options.insert("image_channel".to_string(), int(*image_channel));
    }
    let mut config = Table::new();
    config.insert("chat_mappings".to_string(), Value::Table(mappings));
    config.insert("options".to_string(), Value::Table(options));

    let toml_string = toml::to_string(&config)?;
    fs::write(CONFIG_FILE, toml_string)?;

    Ok(())
}

pub async fn get_hub_server(name: &str) -> Option<Hub> {
    HUBS.get(name).as_deref().copied()
}

pub async fn set_hub_server(name: String, hub: Hub) -> Result<()> {
    HUBS.insert(name, hub);

    save_config().await
}

pub async fn set_chat_mapping(
    discord_channel_id: d::ChannelId,
    telegram_chat_id: t::ChatId,
    webhook_url: Option<String>,
) -> Result<()> {
    // Insert new mapping
    DISCORD_TO_TELEGRAM_CACHE.insert(discord_channel_id, telegram_chat_id);
    TELEGRAM_TO_DISCORD_CACHE.insert(telegram_chat_id, (discord_channel_id, webhook_url));

    // Save the updated mappings to the TOML file
    save_config().await
}

pub enum RemovalChatId {
    Discord(d::ChannelId),
    Telegram(t::ChatId),
}
pub async fn remove_chat_mapping(id: RemovalChatId) -> Result<()> {
    match id {
        RemovalChatId::Discord(id) => {
            DISCORD_TO_TELEGRAM_CACHE.remove(&id);
            TELEGRAM_TO_DISCORD_CACHE.retain(|_, (stored_id, _)| *stored_id != id);
        }
        RemovalChatId::Telegram(id) => {
            TELEGRAM_TO_DISCORD_CACHE.remove(&id);
            DISCORD_TO_TELEGRAM_CACHE.retain(|_, stored_id| *stored_id != id);
        }
    }

    save_config().await
}

pub async fn admins() -> Vec<d::UserId> {
    ADMINS.read().await.clone()
}

pub async fn discord_image_channel() -> Option<d::ChannelId> {
    (&*DISCORD_IMAGE_CHANNEL.read().await).as_ref().copied()
}

pub fn get_telegram_chat_id(discord_channel_id: d::ChannelId) -> Option<t::ChatId> {
    DISCORD_TO_TELEGRAM_CACHE
        .get(&discord_channel_id)
        .map(|v| *v)
}

pub fn get_discord_channel_id(
    telegram_chat_id: t::ChatId,
) -> Option<(d::ChannelId, Option<String>)> {
    TELEGRAM_TO_DISCORD_CACHE
        .get(&telegram_chat_id)
        .map(|v| v.clone())
}

pub async fn get_telegram_chats(
    pool: &SqlitePool,
) -> Result<Vec<(t::ChatId, String)>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT chat_id, title
        FROM telegram_chats
        WHERE is_member = 1
        "#,
    )
    .fetch_all(pool)
    .await
    .map(|r| {
        r.into_iter()
            .map(|(id, title)| (t::ChatId(id), title))
            .collect()
    })
}

pub async fn update_chat_membership(
    pool: &SqlitePool,
    chat_id: t::ChatId,
    title: &str,
    is_member: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO telegram_chats (chat_id, title, is_member)
        VALUES (?, ?, ?)
        ON CONFLICT(chat_id) DO UPDATE SET
            title = excluded.title,
            is_member = excluded.is_member,
            updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(chat_id.0)
    .bind(title)
    .bind(is_member)
    .execute(pool)
    .await?;

    Ok(())
}
