use dashmap::DashMap;
use eyre::Result;
use lazy_static::lazy_static;
use sqlx::{sqlite::SqlitePool, Row};
use std::{fs, path};
use toml::Table;

const CHAT_MAPPING_FILE: &str = "chat_mappings.toml";

#[derive(Clone)]
pub struct Persist {
    db: SqlitePool,
}

pub struct PersistSettings<'a> {
    db_name: &'a str,
}

async fn init_db(db_name: &str) -> Result<SqlitePool> {
    let db_path = path::Path::new(db_name);
    // Create the database file if it doesn't exist
    if !db_path.exists() {
        std::fs::File::create(db_path)?;
    }
    let pool = SqlitePool::connect(&format!("sqlite:{db_name}")).await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            has_files BOOLEAN NOT NULL DEFAULT 0,
        )",
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS message_mapping (
            internal_id INTEGER NOT NULL,
            external_id TEXT NOT NULL,
            portal INTEGER NOT NULL,
            PRIMARY KEY (internal_id, portal),
            FOREIGN KEY (internal_id) REFERENCES messages(id)
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

    // load_chat_mappings()?;

    Ok(pool)
}

use crate::model::{ExternAuthorId, ExternMessageId, Message, MessageId, PortalId};

impl Persist {
    pub async fn init(PersistSettings { db_name }: PersistSettings<'_>) -> Result<Self> {
        Ok(Self {
            db: init_db(db_name).await?,
        })
    }

    pub async fn insert_message(&self, msg: &Message) -> Result<MessageId> {
        let id: i64 =
            sqlx::query_scalar("INSERT INTO messages (has_files) VALUES (?) RETURNING id")
                .bind(!msg.1.attachments.is_empty())
                .fetch_one(&self.db)
                .await?;
        Ok(MessageId(id as u64))
    }

    pub async fn add_message_mapping(
        &self,
        id: MessageId,
        portal: PortalId,
        ext_id: ExternMessageId,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO message_mapping (internal_id, external_id, portal) VALUES (?, ?, ?)",
        )
        .bind(id.0 as i64)
        .bind(ext_id.as_ref())
        .bind(portal.raw() as i64)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    pub async fn get_message_id(
        &self,
        ext_id: ExternMessageId,
        portal: PortalId,
    ) -> Result<MessageId> {
        let id: i64 = sqlx::query_scalar(
            "SELECT internal_id FROM message_mapping WHERE external_id = ? AND portal = ?",
        )
        .bind(ext_id.as_ref())
        .bind(portal.raw() as i64)
        .fetch_one(&self.db)
        .await?;
        Ok(MessageId(id as u64))
    }

    pub async fn get_messages_for_portal(
        &self,
        id: MessageId,
        portal: PortalId,
    ) -> Result<Vec<ExternMessageId>> {
        let rows = sqlx::query(
            "SELECT external_id FROM message_mapping WHERE internal_id = ? AND portal = ?",
        )
        .bind(id.0 as i64)
        .bind(portal.raw() as i64)
        .fetch_all(&self.db)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                ExternMessageId::from({
                    let s: String = r.get(0);
                    s
                })
            })
            .collect())
    }
}
