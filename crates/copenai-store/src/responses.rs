use chrono::Utc;
use serde_json::Value;
use sqlx::SqlitePool;

use copenai_core::error::{CoreError, Result};

#[derive(Debug, Clone)]
pub struct StoredResponse {
    pub id: String,
    pub conversation_id: Option<String>,
    pub status: String,
    pub model: String,
    pub tool_execution: String,
    pub request_json: String,
    pub response_json: Option<String>,
    pub output_json: Option<String>,
    pub usage_json: Option<String>,
    pub previous_response_id: Option<String>,
    pub input_chain_json: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

pub struct ResponseStore;

type ResponseRow = (
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
);

const RESPONSE_SELECT: &str = "SELECT id, conversation_id, status, model, tool_execution, request_json, response_json, output_json, usage_json, previous_response_id, input_chain_json, created_at, completed_at FROM responses";

fn row_to_stored(r: ResponseRow) -> StoredResponse {
    StoredResponse {
        id: r.0,
        conversation_id: r.1,
        status: r.2,
        model: r.3,
        tool_execution: r.4,
        request_json: r.5,
        response_json: r.6,
        output_json: r.7,
        usage_json: r.8,
        previous_response_id: r.9,
        input_chain_json: r.10,
        created_at: r.11,
        completed_at: r.12,
    }
}

impl ResponseStore {
    pub async fn create(
        pool: &SqlitePool,
        id: &str,
        conversation_id: Option<&str>,
        model: &str,
        tool_execution: &str,
        request_json: &str,
        previous_response_id: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO responses (id, conversation_id, status, model, tool_execution, request_json, previous_response_id, created_at)
             VALUES (?, ?, 'in_progress', ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(conversation_id)
        .bind(model)
        .bind(tool_execution)
        .bind(request_json)
        .bind(previous_response_id)
        .bind(&now)
        .execute(pool)
        .await
        .map_err(|e| CoreError::Other(e.to_string()))?;
        Ok(())
    }

    pub async fn get(pool: &SqlitePool, id: &str) -> Result<Option<StoredResponse>> {
        let row = sqlx::query_as::<_, ResponseRow>(&format!("{RESPONSE_SELECT} WHERE id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| CoreError::Other(e.to_string()))?;

        Ok(row.map(row_to_stored))
    }

    pub async fn update_completed(
        pool: &SqlitePool,
        id: &str,
        status: &str,
        response_json: &str,
        output_json: &str,
        usage_json: Option<&str>,
        input_chain_json: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE responses SET status = ?, response_json = ?, output_json = ?, usage_json = ?, input_chain_json = COALESCE(?, input_chain_json), completed_at = ? WHERE id = ?",
        )
        .bind(status)
        .bind(response_json)
        .bind(output_json)
        .bind(usage_json)
        .bind(input_chain_json)
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| CoreError::Other(e.to_string()))?;
        Ok(())
    }

    pub async fn cancel(pool: &SqlitePool, id: &str) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE responses SET status = 'cancelled', completed_at = COALESCE(completed_at, ?) WHERE id = ?",
        )
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| CoreError::Other(e.to_string()))?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn list(
        pool: &SqlitePool,
        limit: u32,
        after: Option<&str>,
        desc: bool,
    ) -> Result<Vec<StoredResponse>> {
        let rows = if let Some(after_id) = after {
            if desc {
                sqlx::query_as::<_, ResponseRow>(&format!(
                    "{RESPONSE_SELECT} WHERE created_at < (SELECT created_at FROM responses WHERE id = ?) ORDER BY created_at DESC LIMIT ?"
                ))
                .bind(after_id)
                .bind(limit)
                .fetch_all(pool)
                .await
            } else {
                sqlx::query_as::<_, ResponseRow>(&format!(
                    "{RESPONSE_SELECT} WHERE created_at > (SELECT created_at FROM responses WHERE id = ?) ORDER BY created_at ASC LIMIT ?"
                ))
                .bind(after_id)
                .bind(limit)
                .fetch_all(pool)
                .await
            }
        } else if desc {
            sqlx::query_as::<_, ResponseRow>(&format!(
                "{RESPONSE_SELECT} ORDER BY created_at DESC LIMIT ?"
            ))
            .bind(limit)
            .fetch_all(pool)
            .await
        } else {
            sqlx::query_as::<_, ResponseRow>(&format!(
                "{RESPONSE_SELECT} ORDER BY created_at ASC LIMIT ?"
            ))
            .bind(limit)
            .fetch_all(pool)
            .await
        }
        .map_err(|e| CoreError::Other(e.to_string()))?;

        Ok(rows.into_iter().map(row_to_stored).collect())
    }

    pub async fn chain_input_items(pool: &SqlitePool, previous_id: &str) -> Result<Vec<Value>> {
        let mut items = Vec::new();
        let mut current = Some(previous_id.to_string());
        let mut seen = std::collections::HashSet::new();

        while let Some(id) = current {
            if !seen.insert(id.clone()) {
                break;
            }
            let stored = Self::get(pool, &id)
                .await?
                .ok_or_else(|| CoreError::Other(format!("previous_response_id not found: {id}")))?;
            if let Some(chain_json) = &stored.input_chain_json {
                if let Ok(chain) = serde_json::from_str::<Vec<Value>>(chain_json) {
                    for item in chain {
                        items.push(item);
                    }
                }
            } else if let Some(output_json) = &stored.output_json {
                if let Ok(outputs) = serde_json::from_str::<Vec<Value>>(output_json) {
                    for item in outputs {
                        if let Some(mapped) = fallback_output_to_chain(&item) {
                            items.push(mapped);
                        }
                    }
                }
            }
            current = stored.previous_response_id;
        }
        items.reverse();
        Ok(items)
    }
}

fn fallback_output_to_chain(item: &Value) -> Option<Value> {
    let obj = item.as_object()?;
    let item_type = obj.get("type")?.as_str()?;
    match item_type {
        "function_call" => Some(item.clone()),
        "message" => Some(item.clone()),
        "function_call_output" => Some(item.clone()),
        _ => {
            if obj.contains_key("name") && obj.contains_key("arguments") {
                Some(serde_json::json!({
                    "type": "function_call",
                    "call_id": obj.get("call_id").cloned().unwrap_or(Value::String(String::new())),
                    "name": obj.get("name")?,
                    "arguments": obj.get("arguments")?,
                }))
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_maps_function_call() {
        let v = serde_json::json!({
            "type": "function_call",
            "call_id": "c1",
            "name": "foo",
            "arguments": "{}"
        });
        assert!(fallback_output_to_chain(&v).is_some());
    }
}
