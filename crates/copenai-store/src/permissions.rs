use std::time::Duration;

use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

use copenai_core::error::{CoreError, Result};

#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct PermissionRequest {
    pub id: String,
    pub conversation_id: String,
    pub tool_title: String,
    pub options_json: String,
    pub status: String,
    pub decision_option_id: Option<String>,
    pub created_at: String,
}

pub struct PermissionStore;

impl PermissionStore {
    pub async fn insert_pending(
        pool: &SqlitePool,
        conversation_id: &str,
        tool_title: &str,
        options_json: &str,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let created_at = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO permission_requests (id, conversation_id, tool_title, options_json, status, created_at) VALUES (?, ?, ?, ?, 'pending', ?)",
        )
        .bind(&id)
        .bind(conversation_id)
        .bind(tool_title)
        .bind(options_json)
        .bind(&created_at)
        .execute(pool)
        .await
        .map_err(|e| CoreError::Other(e.to_string()))?;
        Ok(id)
    }

    pub async fn list_pending(
        pool: &SqlitePool,
        conversation_id: Option<&str>,
    ) -> Result<Vec<PermissionRequest>> {
        if let Some(conv) = conversation_id {
            sqlx::query_as(
                "SELECT id, conversation_id, tool_title, options_json, status, decision_option_id, created_at FROM permission_requests WHERE status = 'pending' AND conversation_id = ? ORDER BY created_at ASC",
            )
            .bind(conv)
            .fetch_all(pool)
            .await
            .map_err(|e| CoreError::Other(e.to_string()))
        } else {
            sqlx::query_as(
                "SELECT id, conversation_id, tool_title, options_json, status, decision_option_id, created_at FROM permission_requests WHERE status = 'pending' ORDER BY created_at ASC",
            )
            .fetch_all(pool)
            .await
            .map_err(|e| CoreError::Other(e.to_string()))
        }
    }

    pub async fn respond(
        pool: &SqlitePool,
        id: &str,
        option_id: Option<&str>,
        cancel: bool,
    ) -> Result<bool> {
        let status = if cancel { "cancelled" } else { "approved" };
        let result = sqlx::query(
            "UPDATE permission_requests SET status = ?, decision_option_id = ? WHERE id = ? AND status = 'pending'",
        )
        .bind(status)
        .bind(option_id)
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| CoreError::Other(e.to_string()))?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn resolve(pool: &SqlitePool, id: &str, status: &str) -> Result<()> {
        sqlx::query("UPDATE permission_requests SET status = ? WHERE id = ?")
            .bind(status)
            .bind(id)
            .execute(pool)
            .await
            .map_err(|e| CoreError::Other(e.to_string()))?;
        Ok(())
    }

    pub async fn wait_for_decision(
        pool: &SqlitePool,
        id: &str,
        timeout: Duration,
    ) -> Result<Option<String>> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let row: Option<(String, Option<String>)> = sqlx::query_as(
                "SELECT status, decision_option_id FROM permission_requests WHERE id = ?",
            )
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(|e| CoreError::Other(e.to_string()))?;

            if let Some((status, option_id)) = row {
                match status.as_str() {
                    "approved" => return Ok(option_id),
                    "cancelled" => return Ok(None),
                    "pending" => {}
                    _ => return Ok(None),
                }
            } else {
                return Ok(None);
            }

            if tokio::time::Instant::now() >= deadline {
                return Ok(None);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}
