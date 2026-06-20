use chrono::Utc;
use sqlx::SqlitePool;

use copenai_core::error::{CoreError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationStatus {
    Active,
    Dormant,
}

impl ConversationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Dormant => "dormant",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "dormant" => Self::Dormant,
            _ => Self::Active,
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ConversationRecord {
    pub id: String,
    pub acp_session_id: Option<String>,
    pub cursor_chat_id: Option<String>,
    pub workspace_rel: String,
    pub status: String,
    pub last_active: String,
    pub created_at: String,
}

pub struct ConversationStore;

impl ConversationStore {
    pub async fn upsert(
        pool: &SqlitePool,
        id: &str,
        workspace_rel: &str,
        acp_session_id: Option<&str>,
        cursor_chat_id: Option<&str>,
    ) -> Result<ConversationRecord> {
        let now = Utc::now().to_rfc3339();
        let existing: Option<ConversationRecord> = sqlx::query_as(
            "SELECT id, acp_session_id, cursor_chat_id, workspace_rel, status, last_active, created_at FROM conversations WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| CoreError::Other(e.to_string()))?;

        if existing.is_some() {
            sqlx::query(
                "UPDATE conversations SET acp_session_id = COALESCE(?, acp_session_id), cursor_chat_id = COALESCE(?, cursor_chat_id), last_active = ?, status = 'active' WHERE id = ?",
            )
            .bind(acp_session_id)
            .bind(cursor_chat_id)
            .bind(&now)
            .bind(id)
            .execute(pool)
            .await
            .map_err(|e| CoreError::Other(e.to_string()))?;
            return Self::get(pool, id).await;
        }

        sqlx::query(
            "INSERT INTO conversations (id, acp_session_id, cursor_chat_id, workspace_rel, status, last_active, created_at) VALUES (?, ?, ?, ?, 'active', ?, ?)",
        )
        .bind(id)
        .bind(acp_session_id)
        .bind(cursor_chat_id)
        .bind(workspace_rel)
        .bind(&now)
        .bind(&now)
        .execute(pool)
        .await
        .map_err(|e| CoreError::Other(e.to_string()))?;
        Self::get(pool, id).await
    }

    pub async fn get(pool: &SqlitePool, id: &str) -> Result<ConversationRecord> {
        sqlx::query_as(
            "SELECT id, acp_session_id, cursor_chat_id, workspace_rel, status, last_active, created_at FROM conversations WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| CoreError::Other(e.to_string()))?
        .ok_or_else(|| CoreError::Other(format!("conversation not found: {id}")))
    }

    pub async fn touch(pool: &SqlitePool, id: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE conversations SET last_active = ?, status = 'active' WHERE id = ?")
            .bind(&now)
            .bind(id)
            .execute(pool)
            .await
            .map_err(|e| CoreError::Other(e.to_string()))?;
        Ok(())
    }

    pub async fn set_dormant(pool: &SqlitePool, id: &str) -> Result<()> {
        sqlx::query("UPDATE conversations SET status = 'dormant' WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await
            .map_err(|e| CoreError::Other(e.to_string()))?;
        Ok(())
    }

    pub async fn set_acp_session(pool: &SqlitePool, id: &str, session_id: &str) -> Result<()> {
        sqlx::query("UPDATE conversations SET acp_session_id = ? WHERE id = ?")
            .bind(session_id)
            .bind(id)
            .execute(pool)
            .await
            .map_err(|e| CoreError::Other(e.to_string()))?;
        Ok(())
    }

    pub async fn set_cursor_chat_id(pool: &SqlitePool, id: &str, chat_id: &str) -> Result<()> {
        sqlx::query("UPDATE conversations SET cursor_chat_id = ? WHERE id = ?")
            .bind(chat_id)
            .bind(id)
            .execute(pool)
            .await
            .map_err(|e| CoreError::Other(e.to_string()))?;
        Ok(())
    }

    pub async fn count_active(pool: &SqlitePool) -> Result<i64> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM conversations WHERE status = 'active'")
                .fetch_one(pool)
                .await
                .map_err(|e| CoreError::Other(e.to_string()))?;
        Ok(count)
    }
}
