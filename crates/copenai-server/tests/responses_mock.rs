use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use copenai_acp::MockResponse;
use copenai_server::test_support::TestHarness;
use http_body_util::BodyExt;
use tower::ServiceExt;

#[tokio::test]
async fn responses_sync_mock() {
    let harness = TestHarness::with_mock(MockResponse::Text("responses hello".into())).await;
    let app = harness.app();
    let body = serde_json::json!({
        "model": "composer-2.5",
        "input": "hello"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
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
    assert_eq!(json["object"], "response");
    assert_eq!(json["status"], "completed");
    let text = json["output"][0]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(text.contains("responses hello"));
}

#[tokio::test]
async fn responses_stream_mock() {
    let harness =
        TestHarness::with_mock(MockResponse::Stream(vec!["stream ".into(), "reply".into()])).await;
    let app = harness.app();
    let body = serde_json::json!({
        "model": "composer-2.5",
        "input": "hello",
        "stream": true
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/event-stream"
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("response.created"));
    assert!(text.contains("response.completed"));
    assert!(text.contains("[DONE]"));
}

#[tokio::test]
async fn responses_store_and_get() {
    let harness = TestHarness::with_mock(MockResponse::Text("stored".into())).await;
    let app = harness.app();
    let body = serde_json::json!({
        "model": "composer-2.5",
        "input": "persist",
        "store": true
    });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(status, StatusCode::OK);
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let id = created["id"].as_str().unwrap().to_string();

    let get_resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/v1/responses/{id}"))
                .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_resp.status(), StatusCode::OK);
    let bytes = get_resp.into_body().collect().await.unwrap().to_bytes();
    let got: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(got["id"], id);
    assert_eq!(got["status"], "completed");
}

#[tokio::test]
async fn responses_client_tool_mode() {
    let tool_json = r#"{"name": "get_weather", "arguments": {"location": "Boston"}}"#;
    let harness = TestHarness::with_mock(MockResponse::Text(tool_json.into())).await;
    let app = harness.app();
    let body = serde_json::json!({
        "model": "composer-2.5",
        "input": "weather?",
        "tools": [{
            "type": "function",
            "name": "get_weather",
            "description": "Get weather",
            "parameters": {
                "type": "object",
                "properties": { "location": { "type": "string" } },
                "required": ["location"]
            }
        }],
        "metadata": { "tool_execution": "client" }
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
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
    assert_eq!(json["status"], "incomplete");
    let outputs = json["output"].as_array().unwrap();
    assert!(outputs.iter().any(|o| o["type"] == "function_call"));
    assert!(outputs.iter().any(|o| o["name"] == "get_weather"));
}

#[tokio::test]
async fn responses_server_mode_requires_webhook() {
    let harness = TestHarness::with_mock(MockResponse::Text("x".into())).await;
    let app = harness.app();
    let body = serde_json::json!({
        "model": "composer-2.5",
        "input": "hi",
        "tools": [{ "type": "function", "name": "foo" }],
        "metadata": { "tool_execution": "server" }
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn responses_list() {
    let harness = TestHarness::with_mock(MockResponse::Text("listed".into())).await;
    let app = harness.app();
    let body = serde_json::json!({
        "model": "composer-2.5",
        "input": "list me",
        "store": true
    });
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/responses?limit=10")
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
    assert!(!json["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn responses_agent_tool_observability() {
    let harness = TestHarness::with_mock(MockResponse::WithEvents {
        text: "done".into(),
        reasoning: vec!["thinking...".into()],
        agent_tools: vec![("tc_1".into(), "Read File".into())],
    })
    .await;
    let app = harness.app();
    let body = serde_json::json!({
        "model": "composer-2.5",
        "input": "go",
        "include": ["reasoning"]
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
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
    let outputs = json["output"].as_array().unwrap();
    assert!(outputs.iter().any(|o| o["type"] == "reasoning"));
    assert!(outputs.iter().any(|o| {
        o["type"] == "function_call" && o["name"].as_str().unwrap_or("").starts_with("agent_")
    }));
}

#[tokio::test]
async fn responses_client_tool_roundtrip() {
    let tool_json = r#"{"name": "get_weather", "arguments": {"location": "Boston"}}"#;
    let harness = TestHarness::with_mock(MockResponse::Text(tool_json.into())).await;
    let app = harness.app();
    let body = serde_json::json!({
        "model": "composer-2.5",
        "input": "weather?",
        "store": true,
        "tools": [{ "type": "function", "name": "get_weather" }],
        "metadata": { "tool_execution": "client" }
    });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let first: serde_json::Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let first_id = first["id"].as_str().unwrap().to_string();

    let body2 = serde_json::json!({
        "model": "composer-2.5",
        "input": [{ "type": "function_call_output", "call_id": "c1", "output": "sunny" }],
        "previous_response_id": first_id,
        "store": true
    });
    let resp2 = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body2.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
}

#[tokio::test]
async fn responses_server_tool_webhook() {
    let tool_json = r#"{"name": "echo", "arguments": {}}"#;
    let (harness, _) = TestHarness::with_webhook_server(
        MockResponse::Sequence(vec![
            MockResponse::Text(tool_json.into()),
            MockResponse::Text("final".into()),
        ]),
        |_| "hooked".into(),
    )
    .await;
    let app = harness.app();
    let body = serde_json::json!({
        "model": "composer-2.5",
        "input": "go",
        "tools": [{ "type": "function", "name": "echo" }],
        "metadata": { "tool_execution": "server" }
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(json["status"], "completed");
}

#[tokio::test]
async fn responses_delete() {
    let harness = TestHarness::with_mock(MockResponse::Text("x".into())).await;
    let app = harness.app();
    let body = serde_json::json!({
        "model": "composer-2.5",
        "input": "del",
        "store": true
    });
    let created: serde_json::Value = serde_json::from_slice(
        &app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/responses")
                    .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    let id = created["id"].as_str().unwrap();
    let del = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/v1/responses/{id}"))
                .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(del.status(), StatusCode::OK);
}

#[tokio::test]
async fn responses_x_tool_execution_header() {
    let harness = TestHarness::with_config(
        MockResponse::Text(r#"{"name":"foo","arguments":{}}"#.into()),
        copenai_core::config::ResponsesSection {
            tool_execution: "client".into(),
            ..Default::default()
        },
    )
    .await;
    let app = harness.app();
    let body = serde_json::json!({
        "model": "composer-2.5",
        "input": "x",
        "tools": [{ "type": "function", "name": "foo" }],
        "metadata": { "tool_execution": "server" }
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                .header("x-tool-execution", "client")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn responses_stream_with_tools() {
    let tool_json = r#"{"name": "get_weather", "arguments": {"location": "NYC"}}"#;
    let harness = TestHarness::with_mock(MockResponse::Text(tool_json.into())).await;
    let app = harness.app();
    let body = serde_json::json!({
        "model": "composer-2.5",
        "input": "weather",
        "stream": true,
        "tools": [{ "type": "function", "name": "get_weather" }],
        "metadata": { "tool_execution": "client" }
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("function_call_arguments"));
}

#[tokio::test]
async fn responses_tool_choice_none() {
    let harness = TestHarness::with_mock(MockResponse::Text("plain text".into())).await;
    let app = harness.app();
    let body = serde_json::json!({
        "model": "composer-2.5",
        "input": "hi",
        "tools": [{ "type": "function", "name": "foo" }],
        "tool_choice": "none"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(json["status"], "completed");
}

#[tokio::test]
async fn responses_max_tool_steps() {
    let tool_json = r#"{"name": "loop", "arguments": {}}"#;
    let mut responses = copenai_core::config::ResponsesSection {
        max_tool_steps: 1,
        tool_execution: "server".into(),
        ..Default::default()
    };
    let (_, url) = TestHarness::with_webhook_server(
        MockResponse::Sequence(vec![
            MockResponse::Text(tool_json.into()),
            MockResponse::Text(tool_json.into()),
        ]),
        |_| "out".into(),
    )
    .await;
    responses.tool_webhook = url;
    let harness = TestHarness::with_config(
        MockResponse::Sequence(vec![
            MockResponse::Text(tool_json.into()),
            MockResponse::Text(tool_json.into()),
        ]),
        responses,
    )
    .await;
    let app = harness.app();
    let body = serde_json::json!({
        "model": "composer-2.5",
        "input": "loop",
        "tools": [{ "type": "function", "name": "loop" }],
        "metadata": { "tool_execution": "server" }
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header(header::AUTHORIZATION, format!("Bearer {}", harness.api_key))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(json["status"], "incomplete");
    assert_eq!(json["incomplete_details"]["reason"], "max_tool_steps");
}
