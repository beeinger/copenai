use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use copenai_acp::MockResponse;
use copenai_server::test_support::TestHarness;
use http_body_util::BodyExt;
use tower::ServiceExt;

#[tokio::test]
async fn health_no_auth() {
    let harness = TestHarness::with_mock(MockResponse::Text("ok".into())).await;
    let app = harness.app();
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn chat_sync_mock() {
    let harness = TestHarness::with_mock(MockResponse::Text("mock assistant reply".into())).await;
    let app = harness.app();
    let body = serde_json::json!({
        "model": "composer-2.5",
        "messages": [{"role": "user", "content": "hello"}]
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap()
        .contains("mock assistant"));
}

#[tokio::test]
async fn tools_returns_501() {
    let harness = TestHarness::with_mock(MockResponse::Text("x".into())).await;
    let app = harness.app();
    let body = serde_json::json!({
        "model": "composer-2.5",
        "messages": [{"role": "user", "content": "hi"}],
        "tools": [{"type": "function", "function": {"name": "foo"}}]
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn missing_bearer_401() {
    let harness = TestHarness::with_mock(MockResponse::Text("x".into())).await;
    let app = harness.app();
    let body = serde_json::json!({
        "model": "composer-2.5",
        "messages": [{"role": "user", "content": "hi"}]
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn stream_mock_emits_chunks() {
    let harness = TestHarness::with_mock(MockResponse::Stream(vec![
        "hello".into(),
        " ".into(),
        "world".into(),
    ]))
    .await;
    let app = harness.app();
    let body = serde_json::json!({
        "model": "composer-2.5",
        "messages": [{"role": "user", "content": "stream"}],
        "stream": true
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("data:"));
    assert!(text.contains("[DONE]"));
    assert!(text.matches("data:").count() >= 3);
}

#[tokio::test]
async fn files_list_empty() {
    let harness = TestHarness::with_mock(MockResponse::Text("x".into())).await;
    let app = harness.app();
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/files")
                .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["object"], "list");
    assert!(json["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn files_list_and_delete() {
    let harness = TestHarness::with_mock(MockResponse::Text("x".into())).await;
    let staging = harness.paths.files_staging_dir();
    let file_id = format!("file-{}", uuid::Uuid::new_v4());
    tokio::fs::create_dir_all(&staging).await.unwrap();
    tokio::fs::write(staging.join(&file_id), b"data")
        .await
        .unwrap();
    let meta = copenai_openai::StagedFileMeta {
        filename: "tiny.bin".into(),
        content_type: "application/octet-stream".into(),
        bytes: 4,
        uploaded_at: chrono::Utc::now().to_rfc3339(),
    };
    copenai_openai::files::write_staged_meta(&staging, &file_id, &meta)
        .await
        .unwrap();

    let app = harness.app();
    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/files")
                .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let list_json: serde_json::Value =
        serde_json::from_slice(&list.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(list_json["data"].as_array().unwrap().len(), 1);

    let del = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/v1/files/{file_id}"))
                .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(del.status(), StatusCode::OK);
    let del_json: serde_json::Value =
        serde_json::from_slice(&del.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(del_json["deleted"], true);

    let get = app
        .oneshot(
            Request::builder()
                .uri(format!("/v1/files/{file_id}"))
                .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get.status(), StatusCode::NOT_FOUND);
}
