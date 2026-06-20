use base64::Engine;
use copenai_e2e::E2eEnv;

const MODEL: &str = "composer-2.5";

/// Always runs (not `#[ignore]`). Prints live E2E availability to the terminal.
#[tokio::test]
async fn live_e2e_status() {
    E2eEnv::notify_status().await;
}

macro_rules! require_env {
    () => {{
        match E2eEnv::try_start().await {
            Some(e) => e,
            None => {
                E2eEnv::log_test_skip(concat!(module_path!(), "::", line!()));
                return;
            }
        }
    }};
}

fn chat(client: &reqwest::Client, env: &E2eEnv) -> reqwest::RequestBuilder {
    client
        .post(format!("{}/v1/chat/completions", env.base_url))
        .bearer_auth(&env.api_key)
        .header("content-type", "application/json")
}

#[tokio::test]
#[ignore]
async fn e2e_01_health_and_models() {
    let env = require_env!();
    let client = env.client();
    let health = client
        .get(format!("{}/health", env.base_url))
        .send()
        .await
        .unwrap();
    assert!(health.status().is_success());
    let models = client
        .get(format!("{}/v1/models", env.base_url))
        .bearer_auth(&env.api_key)
        .send()
        .await
        .unwrap();
    assert!(models.status().is_success());
    let body: serde_json::Value = models.json().await.unwrap();
    let ids: Vec<String> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect();
    assert!(ids.iter().any(|id| id.contains("composer")));
}

#[tokio::test]
#[ignore]
async fn e2e_02_sync_chat() {
    let env = require_env!();
    let client = env.client();
    let resp = chat(&client, &env)
        .json(&serde_json::json!({
            "model": MODEL,
            "messages": [{"role": "user", "content": "Reply with exactly: pong"}]
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    let content = body["choices"][0]["message"]["content"].as_str().unwrap();
    assert!(!content.is_empty());
    assert!(body["usage"]["total_tokens"].as_u64().unwrap_or(0) > 0);
}

#[tokio::test]
#[ignore]
async fn e2e_03_stream_chat() {
    let env = require_env!();
    let client = env.client();
    let resp = chat(&client, &env)
        .json(&serde_json::json!({
            "model": MODEL,
            "stream": true,
            "messages": [{"role": "user", "content": "Count from 1 to 5."}]
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let text = resp.text().await.unwrap();
    assert!(text.matches("data:").count() >= 3);
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
#[ignore]
async fn e2e_04_multi_turn_messages() {
    let env = require_env!();
    let client = env.client();
    let resp = chat(&client, &env)
        .json(&serde_json::json!({
            "model": MODEL,
            "messages": [
                {"role": "user", "content": "Remember codeword: ALPHA123."},
                {"role": "assistant", "content": "I'll remember ALPHA123."},
                {"role": "user", "content": "What was the codeword?"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    let content = body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap()
        .to_lowercase();
    assert!(content.contains("alpha123"));
}

#[tokio::test]
#[ignore]
async fn e2e_06_system_message() {
    let env = require_env!();
    let client = env.client();
    let resp = chat(&client, &env)
        .json(&serde_json::json!({
            "model": MODEL,
            "messages": [
                {"role": "system", "content": "Always start replies with BANANA:"},
                {"role": "user", "content": "Say hi"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    let content = body["choices"][0]["message"]["content"].as_str().unwrap();
    assert!(content.starts_with("BANANA:"));
}

#[tokio::test]
#[ignore]
async fn e2e_07_developer_message() {
    let env = require_env!();
    let client = env.client();
    let resp = chat(&client, &env)
        .json(&serde_json::json!({
            "model": MODEL,
            "messages": [
                {"role": "developer", "content": "Use prefix CHERRY:"},
                {"role": "user", "content": "go"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    let content = body["choices"][0]["message"]["content"].as_str().unwrap();
    assert!(content.contains("CHERRY:"));
}

#[tokio::test]
#[ignore]
async fn e2e_08_temperature() {
    let env = require_env!();
    let client = env.client();
    let resp = chat(&client, &env)
        .json(&serde_json::json!({
            "model": MODEL,
            "temperature": 0.7,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success() || resp.status() == 400);
}

#[tokio::test]
#[ignore]
async fn e2e_09_max_tokens() {
    let env = require_env!();
    let client = env.client();
    let resp = chat(&client, &env)
        .json(&serde_json::json!({
            "model": MODEL,
            "max_tokens": 50,
            "messages": [{"role": "user", "content": "Write a long essay about Rust."}]
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success() || resp.status() == 400);
}

#[tokio::test]
#[ignore]
async fn e2e_10_upload_png_chat() {
    let env = require_env!();
    let client = env.client();
    let png = include_bytes!("../fixtures/tiny.png");
    let part = reqwest::multipart::Part::bytes(png.to_vec())
        .file_name("tiny.png")
        .mime_str("image/png")
        .unwrap();
    let form = reqwest::multipart::Form::new().part("file", part);
    let upload = client
        .post(format!("{}/v1/files", env.base_url))
        .bearer_auth(&env.api_key)
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert!(upload.status().is_success());
    let file_id = upload.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let resp = chat(&client, &env)
        .json(&serde_json::json!({
            "model": MODEL,
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "Describe this image briefly."},
                    {"type": "input_file", "file_id": file_id}
                ]
            }]
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success() || resp.status() == 400);
}

#[tokio::test]
#[ignore]
async fn e2e_11_upload_wav_chat() {
    let env = require_env!();
    let client = env.client();
    let wav = include_bytes!("../fixtures/tiny.wav");
    let part = reqwest::multipart::Part::bytes(wav.to_vec())
        .file_name("tiny.wav")
        .mime_str("audio/wav")
        .unwrap();
    let form = reqwest::multipart::Form::new().part("file", part);
    let upload = client
        .post(format!("{}/v1/files", env.base_url))
        .bearer_auth(&env.api_key)
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert!(upload.status().is_success());
    let file_id = upload.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let resp = chat(&client, &env)
        .json(&serde_json::json!({
            "model": MODEL,
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "Acknowledge audio input."},
                    {"type": "input_file", "file_id": file_id}
                ]
            }]
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success() || resp.status() == 400);
}

#[tokio::test]
#[ignore]
async fn e2e_12_inline_image() {
    let env = require_env!();
    let client = env.client();
    let png = include_bytes!("../fixtures/tiny.png");
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    let url = format!("data:image/png;base64,{b64}");
    let resp = chat(&client, &env)
        .json(&serde_json::json!({
            "model": MODEL,
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "What is in this image?"},
                    {"type": "image_url", "image_url": {"url": url}}
                ]
            }]
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success() || resp.status() == 400);
}

#[tokio::test]
#[ignore]
async fn e2e_14_files_crud() {
    let env = require_env!();
    let client = env.client();
    let png = include_bytes!("../fixtures/tiny.png");
    let part = reqwest::multipart::Part::bytes(png.to_vec())
        .file_name("tiny.png")
        .mime_str("image/png")
        .unwrap();
    let form = reqwest::multipart::Form::new().part("file", part);
    let upload = client
        .post(format!("{}/v1/files", env.base_url))
        .bearer_auth(&env.api_key)
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert!(upload.status().is_success());
    let meta: serde_json::Value = upload.json().await.unwrap();
    let file_id = meta["id"].as_str().unwrap();
    let get = client
        .get(format!("{}/v1/files/{file_id}", env.base_url))
        .bearer_auth(&env.api_key)
        .send()
        .await
        .unwrap();
    assert!(get.status().is_success());
    let content = client
        .get(format!("{}/v1/files/{file_id}/content", env.base_url))
        .bearer_auth(&env.api_key)
        .send()
        .await
        .unwrap();
    assert!(content.status().is_success());
    assert_eq!(content.bytes().await.unwrap().len(), png.len());

    let list = client
        .get(format!("{}/v1/files", env.base_url))
        .bearer_auth(&env.api_key)
        .send()
        .await
        .unwrap();
    assert!(list.status().is_success());
    let listed: serde_json::Value = list.json().await.unwrap();
    assert!(listed["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["id"].as_str() == Some(file_id)));

    let del = client
        .delete(format!("{}/v1/files/{file_id}", env.base_url))
        .bearer_auth(&env.api_key)
        .send()
        .await
        .unwrap();
    assert!(del.status().is_success());
    assert_eq!(
        del.json::<serde_json::Value>().await.unwrap()["deleted"],
        true
    );

    let gone = client
        .get(format!("{}/v1/files/{file_id}", env.base_url))
        .bearer_auth(&env.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(gone.status(), 404);
}

#[tokio::test]
#[ignore]
async fn e2e_15_unknown_model() {
    let env = require_env!();
    let client = env.client();
    let resp = chat(&client, &env)
        .json(&serde_json::json!({
            "model": "not-a-real-model-xyz",
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
#[ignore]
async fn e2e_16_missing_bearer() {
    let env = require_env!();
    let client = env.client();
    let resp = client
        .post(format!("{}/v1/chat/completions", env.base_url))
        .json(&serde_json::json!({
            "model": MODEL,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
#[ignore]
async fn e2e_17_tools_501() {
    let env = require_env!();
    let client = env.client();
    let resp = chat(&client, &env)
        .json(&serde_json::json!({
            "model": MODEL,
            "messages": [{"role": "user", "content": "hi"}],
            "tools": []
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 501);
}

#[tokio::test]
#[ignore]
async fn e2e_18_model_switch() {
    let env = require_env!();
    let client = env.client();
    let conv = "model-switch-conv";
    let r1 = chat(&client, &env)
        .header("x-conversation-id", conv)
        .json(&serde_json::json!({
            "model": MODEL,
            "messages": [{"role": "user", "content": "hello"}]
        }))
        .send()
        .await
        .unwrap();
    assert!(r1.status().is_success());
    let r2 = chat(&client, &env)
        .header("x-conversation-id", conv)
        .json(&serde_json::json!({
            "model": "auto",
            "messages": [{"role": "user", "content": "hello again"}]
        }))
        .send()
        .await
        .unwrap();
    assert!(r2.status().is_success() || r2.status() == 400);
}

#[tokio::test]
#[ignore]
async fn e2e_19_user_field_not_conv_id() {
    let env = require_env!();
    let client = env.client();
    let _ = chat(&client, &env)
        .header("x-conversation-id", "sticky-conv-1")
        .json(&serde_json::json!({
            "model": MODEL,
            "user": "should-not-override",
            "messages": [{"role": "user", "content": "hello sticky"}]
        }))
        .send()
        .await
        .unwrap();
    let resp2 = chat(&client, &env)
        .header("x-conversation-id", "sticky-conv-1")
        .json(&serde_json::json!({
            "model": MODEL,
            "user": "other-user",
            "messages": [
                {"role": "user", "content": "My name is Bob."},
                {"role": "assistant", "content": "Hi Bob."},
                {"role": "user", "content": "What is my name?"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert!(resp2.status().is_success());
    let body: serde_json::Value = resp2.json().await.unwrap();
    let content = body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap()
        .to_lowercase();
    assert!(content.contains("bob"));
}
