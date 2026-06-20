use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

use copenai_core::error::{CoreError, Result};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ApiKeyRecord {
    pub id: String,
    pub name: String,
    pub prefix: String,
    pub created_at: String,
}

pub struct ApiKeyStore;

const KEY_PREFIX: &str = "sk-copenai-";

impl ApiKeyStore {
    pub fn generate_secret() -> String {
        format!("{KEY_PREFIX}{}", Uuid::new_v4().simple())
    }

    pub fn prefix_of(secret: &str) -> String {
        secret.chars().take(16).collect()
    }

    pub async fn create(pool: &SqlitePool, name: &str) -> Result<(ApiKeyRecord, String)> {
        let secret = Self::generate_secret();
        let hash = bcrypt::hash(&secret, bcrypt::DEFAULT_COST)
            .map_err(|e| CoreError::Other(e.to_string()))?;
        let id = Uuid::new_v4().to_string();
        let prefix = Self::prefix_of(&secret);
        let created_at = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO api_keys (id, name, key_hash, prefix, created_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(name)
        .bind(&hash)
        .bind(&prefix)
        .bind(&created_at)
        .execute(pool)
        .await
        .map_err(|e| CoreError::Other(e.to_string()))?;
        let record = ApiKeyRecord {
            id,
            name: name.to_string(),
            prefix,
            created_at,
        };
        Ok((record, secret))
    }

    pub async fn list(pool: &SqlitePool) -> Result<Vec<ApiKeyRecord>> {
        sqlx::query_as::<_, ApiKeyRecord>(
            "SELECT id, name, prefix, created_at FROM api_keys ORDER BY created_at DESC",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| CoreError::Other(e.to_string()))
    }

    pub async fn delete(pool: &SqlitePool, id_or_prefix: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM api_keys WHERE id = ? OR prefix = ?")
            .bind(id_or_prefix)
            .bind(id_or_prefix)
            .execute(pool)
            .await
            .map_err(|e| CoreError::Other(e.to_string()))?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn validate(pool: &SqlitePool, secret: &str) -> Result<bool> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT key_hash FROM api_keys")
            .fetch_all(pool)
            .await
            .map_err(|e| CoreError::Other(e.to_string()))?;
        for (hash,) in rows {
            if bcrypt::verify(secret, &hash).unwrap_or(false) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn mem_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn create_and_validate_key() {
        let pool = mem_pool().await;
        let (_, secret) = ApiKeyStore::create(&pool, "test").await.unwrap();
        assert!(ApiKeyStore::validate(&pool, &secret).await.unwrap());
        assert!(!ApiKeyStore::validate(&pool, "bad").await.unwrap());
    }
}
