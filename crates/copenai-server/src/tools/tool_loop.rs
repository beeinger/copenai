use std::time::Duration;

use copenai_core::config::ResponsesSection;
use copenai_openai::{
    ensure_call_ids, filter_tools, from_responses_choice, new_item_id,
    parse_and_validate_calls, validate_detected_calls, FunctionTool, OutputItem,
    ParsedFunctionCall, ResolvedToolChoice, ResponsesToolChoice,
};
use futures::future::join_all;
use reqwest::Client;
use serde_json::{json, Value};

use super::mode::ToolExecutionMode;

pub struct ToolLoopEngine {
    config: ResponsesSection,
    client: Client,
}

impl ToolLoopEngine {
    pub fn new(config: ResponsesSection) -> Self {
        Self {
            config,
            client: Client::new(),
        }
    }

    pub fn resolve_mode_responses(
        &self,
        metadata_mode: Option<&str>,
        header_mode: Option<&str>,
    ) -> ToolExecutionMode {
        super::mode::resolve_mode(&self.config.tool_execution, metadata_mode, header_mode)
    }

    pub fn validate_server_mode(&self, mode: ToolExecutionMode) -> Result<(), String> {
        if mode == ToolExecutionMode::Server && self.config.tool_webhook.is_empty() {
            return Err(
                "server tool mode requires [responses].tool_webhook or use client mode".into(),
            );
        }
        Ok(())
    }

    pub fn effective_tools(
        tools: &[FunctionTool],
        choice: &ResolvedToolChoice,
    ) -> Vec<FunctionTool> {
        filter_tools(tools, choice)
    }

    pub fn resolve_choice_responses(
        tool_choice: Option<&ResponsesToolChoice>,
    ) -> ResolvedToolChoice {
        from_responses_choice(tool_choice)
    }

    pub fn parse_calls(
        &self,
        text: &str,
        tools: &[FunctionTool],
        choice: &ResolvedToolChoice,
    ) -> Result<Vec<ParsedFunctionCall>, String> {
        let effective = Self::effective_tools(tools, choice);
        if effective.is_empty() {
            return Ok(vec![]);
        }
        let mut calls = parse_and_validate_calls(text, &effective)?;
        validate_detected_calls(&calls, choice, &effective)?;
        ensure_call_ids(&mut calls);
        Ok(calls)
    }

    pub fn calls_to_output(calls: &[ParsedFunctionCall]) -> Vec<OutputItem> {
        calls
            .iter()
            .map(|c| {
                OutputItem::function_call(
                    &new_item_id("fc"),
                    &c.call_id,
                    &c.name,
                    &c.arguments.to_string(),
                )
            })
            .collect()
    }

    pub async fn execute_webhook(
        &self,
        conversation_id: &str,
        response_id: &str,
        call: &ParsedFunctionCall,
    ) -> Result<String, String> {
        let url = &self.config.tool_webhook;
        let payload = json!({
            "conversation_id": conversation_id,
            "response_id": response_id,
            "call_id": call.call_id,
            "name": call.name,
            "arguments": call.arguments,
        });
        let timeout = Duration::from_secs(self.config.tool_webhook_timeout_secs);
        let resp = tokio::time::timeout(timeout, self.client.post(url).json(&payload).send())
            .await
            .map_err(|_| "tool_webhook timeout".to_string())?
            .map_err(|e| format!("tool_webhook request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("tool_webhook returned {}", resp.status()));
        }
        let body: Value = resp
            .json()
            .await
            .map_err(|e| format!("tool_webhook invalid json: {e}"))?;
        let output = body
            .get("output")
            .cloned()
            .ok_or_else(|| "tool_webhook response missing output field".to_string())?;
        Ok(output.to_string())
    }

    async fn execute_calls(
        &self,
        conversation_id: &str,
        response_id: &str,
        calls: &[ParsedFunctionCall],
        parallel: bool,
    ) -> Result<Vec<String>, String> {
        if parallel && calls.len() > 1 {
            let futs: Vec<_> = calls
                .iter()
                .map(|call| self.execute_webhook(conversation_id, response_id, call))
                .collect();
            let results = join_all(futs).await;
            results.into_iter().collect()
        } else {
            let mut outputs = Vec::new();
            for call in calls {
                outputs.push(
                    self.execute_webhook(conversation_id, response_id, call)
                        .await?,
                );
            }
            Ok(outputs)
        }
    }

    pub async fn run_server_loop(
        &self,
        conversation_id: &str,
        response_id: &str,
        initial_text: &str,
        tools: &[FunctionTool],
        choice: &ResolvedToolChoice,
        parallel: bool,
        mut prompt_fn: impl FnMut(String) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<String, String>> + Send>,
        >,
    ) -> Result<(String, Vec<OutputItem>, bool), String> {
        let mut all_outputs = Vec::new();
        let mut text = initial_text.to_string();
        let mut steps = 0u32;

        loop {
            let calls = self.parse_calls(&text, tools, choice)?;
            if calls.is_empty() {
                if !text.is_empty() {
                    all_outputs
                        .push(OutputItem::message_text(&text, &new_item_id("msg")));
                }
                return Ok((text, all_outputs, false));
            }

            for call in &calls {
                all_outputs.extend(Self::calls_to_output(std::slice::from_ref(call)));
            }

            if steps >= self.config.max_tool_steps {
                return Ok((text, all_outputs, true));
            }
            steps += 1;

            let webhook_outputs = match self
                .execute_calls(conversation_id, response_id, &calls, parallel)
                .await
            {
                Ok(o) => o,
                Err(e) if self.config.tool_webhook_fallback == "agent" => {
                    vec![format!("webhook error (agent fallback): {e}")]
                }
                Err(e) => return Err(e),
            };

            let continuation = calls
                .iter()
                .zip(webhook_outputs.iter())
                .map(|(call, output)| {
                    format!(
                        "function_call_output call_id={} output={}",
                        call.call_id, output
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            text = prompt_fn(continuation).await?;
        }
    }

    pub fn build_function_call_outputs(
        outputs: &[(String, String)],
    ) -> Vec<copenai_openai::InputItem> {
        outputs
            .iter()
            .map(|(call_id, output)| copenai_openai::InputItem::FunctionCallOutput {
                call_id: call_id.clone(),
                output: output.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_tools_none() {
        let tools = vec![FunctionTool::new("a")];
        let filtered = ToolLoopEngine::effective_tools(&tools, &ResolvedToolChoice::None);
        assert!(filtered.is_empty());
    }
}
