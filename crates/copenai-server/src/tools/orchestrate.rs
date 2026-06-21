use std::collections::HashMap;

use copenai_acp::{
    AgentPrompt, AgentToolEvent, AgentToolEventKind, FinishReason, PromptStreamEvent,
};
use copenai_openai::{
    build_json_schema_prompt, build_prompt_plan, build_schema_retry_prompt, build_tool_system_prompt,
    from_responses_choice, map_message_content, messages::{truncate_history, usage_char_count}, new_item_id,
    validate_json_output, MessageContent,
    OutputItem, ParsedChat, ParsedResponse, ResponseCreateRequest,
    ResponsesStreamEvent, ToolCall, Usage,
};
use futures::StreamExt;

use super::tool_loop::ToolLoopEngine;
use crate::tools::ToolExecutionMode;
use crate::state::SharedState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncompleteReason {
    MaxOutputTokens,
    MaxToolSteps,
    ToolCalls,
}

impl IncompleteReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MaxOutputTokens => "max_output_tokens",
            Self::MaxToolSteps => "max_tool_steps",
            Self::ToolCalls => "tool_calls",
        }
    }
}

pub struct ResponsesTurnOutcome {
    pub text: String,
    pub outputs: Vec<OutputItem>,
    pub usage: Usage,
    pub incomplete: bool,
    pub incomplete_reason: Option<IncompleteReason>,
}

pub struct ChatTurnOutcome {
    pub text: String,
    pub tool_calls: Vec<copenai_openai::ParsedFunctionCall>,
    pub usage: Usage,
    pub finish_reason: String,
    pub incomplete_reason: Option<IncompleteReason>,
}

pub struct ToolOrchestrator;

impl ToolOrchestrator {
    pub async fn build_agent_prompt(
        state: &SharedState,
        conversation_id: &str,
        model: &str,
        parsed: &ParsedChat,
        extra_system: &[String],
        tool_results: &[(String, String)],
    ) -> Result<AgentPrompt, String> {
        build_agent_prompt_inner(
            state,
            conversation_id,
            model,
            parsed,
            extra_system,
            tool_results,
        )
        .await
    }

    pub async fn build_responses_prompt(
        state: &SharedState,
        request: &ResponseCreateRequest,
        parsed: &ParsedResponse,
        conversation_id: &str,
        model: &str,
    ) -> Result<AgentPrompt, String> {
        let choice = from_responses_choice(request.tool_choice.as_ref());
        let tools = ToolLoopEngine::effective_tools(&parsed.tools, &choice);

        let mut system_parts = Vec::new();
        if !parsed.system.is_empty() {
            system_parts.push(parsed.system.clone());
        }
        if !tools.is_empty() {
            system_parts.push(build_tool_system_prompt(&tools));
        }
        if request.wants_json_schema() {
            if let Some(schema) = request.json_schema() {
                system_parts.push(build_json_schema_prompt(schema));
            }
        }
        for (call_id, output) in &parsed.function_call_outputs {
            system_parts.push(format!("Tool result for call_id={call_id}: {output}"));
        }

        let mut chat_like = ParsedChat {
            system: system_parts.join("\n\n"),
            history: parsed.history.clone(),
            final_user_content: parsed.final_user_content.clone(),
            openai_user: None,
            temperature: parsed.temperature,
            max_tokens: parsed.max_tokens,
            tools: tools.clone(),
            tool_choice: choice,
            tool_results: parsed.function_call_outputs.clone(),
            parallel_tool_calls: request.parallel_tool_calls.unwrap_or(true),
        };

        if request.truncation.as_deref() == Some("auto") {
            let budget = state.config.server.max_concurrent_agents.saturating_mul(8000);
            truncate_history(&mut chat_like.history, budget);
        }

        build_agent_prompt_inner(state, conversation_id, model, &chat_like, &[], &[]).await
    }

    pub async fn execute_responses_turn(
        state: &SharedState,
        request: &ResponseCreateRequest,
        parsed: &ParsedResponse,
        conversation_id: &str,
        response_id: &str,
        model: &str,
        mode: ToolExecutionMode,
        streaming: bool,
        event_tx: Option<tokio::sync::mpsc::Sender<ResponsesStreamEvent>>,
        mut response_obj: Option<copenai_openai::ResponseObject>,
    ) -> Result<ResponsesTurnOutcome, String> {
        let choice = from_responses_choice(request.tool_choice.as_ref());
        let tools = ToolLoopEngine::effective_tools(&parsed.tools, &choice);
        let parallel = request.parallel_tool_calls.unwrap_or(true);

        let prompt = Self::build_responses_prompt(state, request, parsed, conversation_id, model)
            .await?;
        let events = state
            .supervisor
            .prompt_stream(conversation_id, prompt)
            .await?;

        let stream_agent_tools = state.config.responses.stream_agent_tools;
        let include_reasoning = request.include_reasoning();
        let event_tx_for_tools = event_tx.clone();

        let (mut text, mut usage_snap, finish, mut agent_outputs, _agent_index) = if streaming {
            let resp = response_obj
                .clone()
                .unwrap_or_else(|| copenai_openai::response_created(response_id, model));
            let taken = response_obj.take().unwrap_or_else(|| resp.clone());
            let result = drain_stream(
                events,
                event_tx.clone(),
                taken,
                stream_agent_tools,
                include_reasoning,
            )
            .await?;
            response_obj = Some(resp);
            result
        } else {
            let (t, u, f, o, idx) =
                drain_sync(events, stream_agent_tools, include_reasoning).await?;
            (t, u, f, o, idx)
        };

        let mut incomplete_reason = if finish == FinishReason::Length {
            Some(IncompleteReason::MaxOutputTokens)
        } else {
            None
        };

        let tool_engine = ToolLoopEngine::new(state.config.responses.clone());

        if !tools.is_empty() {
            match mode {
                ToolExecutionMode::Client => {
                    let calls = tool_engine.parse_calls(&text, &tools, &choice)?;
                    if !calls.is_empty() {
                        if streaming {
                            if let (Some(tx), Some(mut resp)) = (&event_tx_for_tools, response_obj.clone()) {
                                emit_tool_sse_events(tx, &mut resp, &calls).await;
                                response_obj = Some(resp);
                            }
                        }
                        agent_outputs.extend(ToolLoopEngine::calls_to_output(&calls));
                        text = String::new();
                        incomplete_reason = Some(IncompleteReason::ToolCalls);
                    } else if !text.is_empty() {
                        agent_outputs.push(OutputItem::message_text(
                            &text,
                            &new_item_id("msg"),
                        ));
                    }
                }
                ToolExecutionMode::Server => {
                    let state_clone = state.clone();
                    let conv = conversation_id.to_string();
                    let rid = response_id.to_string();
                    let model_owned = model.to_string();
                    let req = request.clone();
                    let parsed_clone = parsed.clone();
                    let choice_clone = choice.clone();

                    let conv_for_loop = conv.clone();
                    let (final_text, loop_outputs, hit_limit) = tool_engine
                        .run_server_loop(
                            &conv_for_loop,
                            &rid,
                            &text,
                            &tools,
                            &choice_clone,
                            parallel,
                            move |continuation| {
                                let state = state_clone.clone();
                                let req = req.clone();
                                let mut p = parsed_clone.clone();
                                let conv = conv.clone();
                                let model_owned = model_owned.clone();
                                p.final_user_content = MessageContent::Text(continuation);
                                Box::pin(async move {
                                    let prompt = ToolOrchestrator::build_responses_prompt(
                                        &state,
                                        &req,
                                        &p,
                                        &conv,
                                        &model_owned,
                                    )
                                    .await?;
                                    let events = state
                                        .supervisor
                                        .prompt_stream(&conv, prompt)
                                        .await?;
                                    let (t, _, _, _, _) =
                                        drain_sync(events, false, false).await?;
                                    Ok(t)
                                })
                            },
                        )
                        .await?;
                    text = final_text;
                    agent_outputs.extend(loop_outputs);
                    if hit_limit {
                        incomplete_reason = Some(IncompleteReason::MaxToolSteps);
                    }
                }
            }
        } else if !text.is_empty() {
            agent_outputs.push(OutputItem::message_text(
                &text,
                &new_item_id("msg"),
            ));
        }

        if request.wants_json_schema() {
            if let Some(schema) = request.json_schema() {
                match validate_json_output(&text, schema) {
                    Ok(_) => {}
                    Err(first_err) => {
                        let retry_prompt = build_schema_retry_prompt(schema, &first_err);
                        let mut retry_parsed = parsed.clone();
                        retry_parsed.system = format!("{}\n\n{}", retry_parsed.system, retry_prompt);
                        let retry_agent = Self::build_responses_prompt(
                            state,
                            request,
                            &retry_parsed,
                            conversation_id,
                            model,
                        )
                        .await?;
                        let retry_events = state
                            .supervisor
                            .prompt_stream(conversation_id, retry_agent)
                            .await?;
                        let (retry_text, retry_usage, retry_finish, _, _) =
                            drain_sync(retry_events, false, false).await?;
                        text = retry_text;
                        usage_snap.prompt_tokens = retry_usage.prompt_tokens;
                        usage_snap.completion_tokens = retry_usage.completion_tokens;
                        usage_snap.total_tokens = retry_usage.total_tokens;
                        if retry_finish == FinishReason::Length {
                            incomplete_reason = Some(IncompleteReason::MaxOutputTokens);
                        }
                        validate_json_output(&text, schema)?;
                        if !text.is_empty() && agent_outputs.is_empty() {
                            agent_outputs.push(OutputItem::message_text(
                                &text,
                                &new_item_id("msg"),
                            ));
                        }
                    }
                }
            }
        }

        let usage = Usage {
            prompt_tokens: usage_snap.prompt_tokens,
            completion_tokens: usage_snap.completion_tokens,
            total_tokens: usage_snap.total_tokens,
        };

        Ok(ResponsesTurnOutcome {
            text,
            outputs: agent_outputs,
            usage,
            incomplete: incomplete_reason.is_some(),
            incomplete_reason,
        })
    }

    pub async fn execute_chat_turn(
        state: &SharedState,
        parsed: &ParsedChat,
        conversation_id: &str,
        model: &str,
        mode: ToolExecutionMode,
        response_id: &str,
    ) -> Result<ChatTurnOutcome, String> {
        let choice = parsed.tool_choice.clone();
        let tools = parsed.tools.clone();
        let parallel = parsed.parallel_tool_calls;

        let mut extra = Vec::new();
        if !tools.is_empty() {
            extra.push(build_tool_system_prompt(&tools));
        }
        for (call_id, output) in &parsed.tool_results {
            extra.push(format!("Tool result for call_id={call_id}: {output}"));
        }

        let prompt = build_agent_prompt_inner(
            state,
            conversation_id,
            model,
            parsed,
            &extra,
            &[],
        )
        .await?;

        let events = state
            .supervisor
            .prompt_stream(conversation_id, prompt)
            .await?;
        let (mut text, usage_snap, finish, _, _) =
            drain_sync(events, false, false).await?;

        let tool_engine = ToolLoopEngine::new(state.config.responses.clone());
        let mut tool_calls = Vec::new();
        let mut finish_reason = finish.as_openai_str().to_string();
        let mut incomplete_reason = None;

        if !tools.is_empty() {
            match mode {
                ToolExecutionMode::Client => {
                    let calls = tool_engine.parse_calls(&text, &tools, &choice)?;
                    if !calls.is_empty() {
                        tool_calls = calls;
                        text = String::new();
                        finish_reason = "tool_calls".into();
                    }
                }
                ToolExecutionMode::Server => {
                    let state_clone = state.clone();
                    let conv = conversation_id.to_string();
                    let rid = response_id.to_string();
                    let model_owned = model.to_string();
                    let parsed_clone = parsed.clone();
                    let choice_clone = choice.clone();

                    let conv_for_loop = conv.clone();
                    let (final_text, _, hit_limit) = tool_engine
                        .run_server_loop(
                            &conv_for_loop,
                            &rid,
                            &text,
                            &tools,
                            &choice_clone,
                            parallel,
                            move |continuation| {
                                let state = state_clone.clone();
                                let mut p = parsed_clone.clone();
                                let conv = conv.clone();
                                let model_owned = model_owned.clone();
                                p.final_user_content = MessageContent::Text(continuation);
                                Box::pin(async move {
                                    let prompt = build_agent_prompt_inner(
                                        &state,
                                        &conv,
                                        &model_owned,
                                        &p,
                                        &[],
                                        &[],
                                    )
                                    .await?;
                                    let events = state
                                        .supervisor
                                        .prompt_stream(&conv, prompt)
                                        .await?;
                                    let (t, _, _, _, _) =
                                        drain_sync(events, false, false).await?;
                                    Ok(t)
                                })
                            },
                        )
                        .await?;
                    text = final_text;
                    if hit_limit {
                        incomplete_reason = Some(IncompleteReason::MaxToolSteps);
                    }
                }
            }
        }

        if finish == FinishReason::Length && incomplete_reason.is_none() {
            incomplete_reason = Some(IncompleteReason::MaxOutputTokens);
        }

        Ok(ChatTurnOutcome {
            text,
            tool_calls,
            usage: Usage {
                prompt_tokens: usage_snap.prompt_tokens,
                completion_tokens: usage_snap.completion_tokens,
                total_tokens: usage_snap.total_tokens,
            },
            finish_reason,
            incomplete_reason,
        })
    }
}

pub fn calls_to_chat_tool_calls(calls: &[copenai_openai::ParsedFunctionCall]) -> Vec<ToolCall> {
    calls
        .iter()
        .enumerate()
        .map(|(i, c)| ToolCall {
            id: if c.call_id.is_empty() {
                format!("call_{i}")
            } else {
                c.call_id.clone()
            },
            call_type: "function".into(),
            function: copenai_openai::FunctionCallPayload {
                name: c.name.clone(),
                arguments: c.arguments.to_string(),
            },
        })
        .collect()
}

async fn build_agent_prompt_inner(
    state: &SharedState,
    conversation_id: &str,
    model: &str,
    parsed: &ParsedChat,
    extra_system: &[String],
    tool_results: &[(String, String)],
) -> Result<AgentPrompt, String> {
    let mut system_parts = Vec::new();
    if !parsed.system.is_empty() {
        system_parts.push(parsed.system.clone());
    }
    system_parts.extend(extra_system.iter().cloned());
    for (call_id, output) in tool_results {
        system_parts.push(format!("Tool result for call_id={call_id}: {output}"));
    }

    let session_hot = state.supervisor.is_session_active(conversation_id).await;
    let chat_like = ParsedChat {
        system: system_parts.join("\n\n"),
        history: parsed.history.clone(),
        final_user_content: parsed.final_user_content.clone(),
        openai_user: parsed.openai_user.clone(),
        temperature: parsed.temperature,
        max_tokens: parsed.max_tokens,
        tools: parsed.tools.clone(),
        tool_choice: parsed.tool_choice.clone(),
        tool_results: parsed.tool_results.clone(),
        parallel_tool_calls: parsed.parallel_tool_calls,
    };
    let plan = build_prompt_plan(&chat_like, session_hot);

    let mapped = map_message_content(
        &state.paths,
        conversation_id,
        &parsed.final_user_content,
    )
    .await?;

    Ok(AgentPrompt {
        model: model.to_string(),
        mapped: mapped.clone(),
        system_prefix: plan.system_prefix,
        replay_transcript: plan.replay_transcript,
        temperature: parsed.temperature,
        max_tokens: parsed.max_tokens,
        usage_prompt_chars: usage_char_count(&chat_like, &mapped),
    })
}

type DrainResult = (
    String,
    copenai_acp::UsageSnapshot,
    FinishReason,
    Vec<OutputItem>,
    HashMap<String, usize>,
);

async fn drain_sync(
    mut events: copenai_acp::PromptEventStream,
    stream_agent_tools: bool,
    include_reasoning: bool,
) -> Result<DrainResult, String> {
    let mut text = String::new();
    let mut usage = copenai_acp::UsageSnapshot::default();
    let mut finish = FinishReason::Stop;
    let mut agent_outputs = Vec::new();
    let mut agent_index: HashMap<String, usize> = HashMap::new();

    while let Some(event) = events.next().await {
        match event {
            PromptStreamEvent::Delta(d) => text.push_str(&d),
            PromptStreamEvent::Usage(u) => usage = u,
            PromptStreamEvent::Done {
                finish_reason,
                full_text,
                usage: u,
            } => {
                text = full_text;
                usage = u;
                finish = finish_reason;
                break;
            }
            PromptStreamEvent::Error(e) => return Err(e),
            PromptStreamEvent::ReasoningDelta(d) if include_reasoning => {
                agent_outputs.push(OutputItem::reasoning(&new_item_id("rs"), &d));
            }
            PromptStreamEvent::ReasoningDelta(_) => {}
            PromptStreamEvent::AgentToolCall(ev) if stream_agent_tools => {
                apply_agent_tool_event(&mut agent_outputs, &mut agent_index, &ev);
            }
            PromptStreamEvent::AgentToolCall(_) => {}
        }
    }
    Ok((text, usage, finish, agent_outputs, agent_index))
}

async fn drain_stream(
    mut events: copenai_acp::PromptEventStream,
    event_tx: Option<tokio::sync::mpsc::Sender<ResponsesStreamEvent>>,
    mut response: copenai_openai::ResponseObject,
    stream_agent_tools: bool,
    include_reasoning: bool,
) -> Result<DrainResult, String> {
    let mut text = String::new();
    let mut usage = copenai_acp::UsageSnapshot::default();
    let mut finish = FinishReason::Stop;
    let mut agent_outputs = Vec::new();
    let mut agent_index: HashMap<String, usize> = HashMap::new();
    let msg_item_id = new_item_id("msg");
    let msg_index = 0usize;
    let mut msg_started = false;
    let mut content_part_added = false;

    while let Some(event) = events.next().await {
        match event {
            PromptStreamEvent::Delta(d) => {
                text.push_str(&d);
                if !msg_started {
                    msg_started = true;
                    let item = OutputItem::message_text("", &msg_item_id);
                    response.output.push(item.clone());
                    if let Some(tx) = &event_tx {
                        let _ = tx
                            .send(ResponsesStreamEvent::OutputItemAdded {
                                response: response.clone(),
                                output_index: msg_index,
                                item,
                            })
                            .await;
                    }
                }
                if !content_part_added {
                    content_part_added = true;
                    if let Some(tx) = &event_tx {
                        if let Some(OutputItem::Message { content, .. }) =
                            response.output.get(msg_index)
                        {
                            if let Some(part) = content.first() {
                                let _ = tx
                                    .send(ResponsesStreamEvent::ContentPartAdded {
                                        response: response.clone(),
                                        output_index: msg_index,
                                        content_index: 0,
                                        part: part.clone(),
                                    })
                                    .await;
                            }
                        }
                    }
                }
                if let Some(tx) = &event_tx {
                    let _ = tx
                        .send(ResponsesStreamEvent::OutputTextDelta {
                            response: response.clone(),
                            output_index: msg_index,
                            content_index: 0,
                            delta: d,
                        })
                        .await;
                }
            }
            PromptStreamEvent::ReasoningDelta(d) if include_reasoning => {
                let item = OutputItem::reasoning(&new_item_id("rs"), &d);
                let idx = response.output.len();
                response.output.push(item.clone());
                if let Some(tx) = &event_tx {
                    let _ = tx
                        .send(ResponsesStreamEvent::OutputItemAdded {
                            response: response.clone(),
                            output_index: idx,
                            item,
                        })
                        .await;
                    let _ = tx
                        .send(ResponsesStreamEvent::ReasoningDelta {
                            response: response.clone(),
                            output_index: idx,
                            delta: d,
                        })
                        .await;
                }
            }
            PromptStreamEvent::AgentToolCall(ev) if stream_agent_tools => {
                let idx = apply_agent_tool_event(&mut agent_outputs, &mut agent_index, &ev);
                if let Some(item) = agent_outputs.get(idx).cloned() {
                    match ev.kind {
                        AgentToolEventKind::Started => {
                            response.output.push(item.clone());
                        }
                        AgentToolEventKind::Updated => {
                            if let Some(existing) = response.output.get_mut(idx) {
                                *existing = item.clone();
                            }
                        }
                    }
                    if let Some(tx) = &event_tx {
                        if matches!(ev.kind, AgentToolEventKind::Started) {
                            let _ = tx
                                .send(ResponsesStreamEvent::OutputItemAdded {
                                    response: response.clone(),
                                    output_index: idx,
                                    item: item.clone(),
                                })
                                .await;
                        }
                        let _ = tx
                            .send(ResponsesStreamEvent::OutputItemDone {
                                response: response.clone(),
                                output_index: idx,
                                item,
                            })
                            .await;
                    }
                }
            }
            PromptStreamEvent::Usage(u) => usage = u,
            PromptStreamEvent::Done {
                finish_reason,
                full_text,
                usage: u,
            } => {
                text = full_text;
                usage = u;
                finish = finish_reason;
                break;
            }
            PromptStreamEvent::Error(e) => return Err(e),
            _ => {}
        }
    }

    if msg_started {
        if let Some(item) = response.output.get_mut(msg_index) {
            if let OutputItem::Message { content, .. } = item {
                if let Some(copenai_openai::OutputContentPart::OutputText { text: t, .. }) =
                    content.first_mut()
                {
                    *t = text.clone();
                }
            }
        }
        if let Some(tx) = &event_tx {
            if let Some(item) = response.output.get(msg_index).cloned() {
                let _ = tx
                    .send(ResponsesStreamEvent::OutputItemDone {
                        response: response.clone(),
                        output_index: msg_index,
                        item,
                    })
                    .await;
            }
        }
    }

    Ok((text, usage, finish, agent_outputs, agent_index))
}

fn apply_agent_tool_event(
    outputs: &mut Vec<OutputItem>,
    index_map: &mut HashMap<String, usize>,
    ev: &AgentToolEvent,
) -> usize {
    match ev.kind {
        AgentToolEventKind::Started => {
            let item = agent_tool_to_output(ev);
            let idx = outputs.len();
            outputs.push(item);
            index_map.insert(ev.tool_call_id.clone(), idx);
            idx
        }
        AgentToolEventKind::Updated => {
            if let Some(&idx) = index_map.get(&ev.tool_call_id) {
                let item = agent_tool_to_output(ev);
                outputs[idx] = item;
                idx
            } else {
                let item = agent_tool_to_output(ev);
                let idx = outputs.len();
                outputs.push(item);
                index_map.insert(ev.tool_call_id.clone(), idx);
                idx
            }
        }
    }
}

fn agent_tool_to_output(ev: &AgentToolEvent) -> OutputItem {
    OutputItem::agent_function_call(
        &new_item_id("agent_fc"),
        &ev.tool_call_id,
        &ev.title,
        ev.raw_input.as_ref(),
    )
}

pub async fn build_agent_prompt(
    state: &SharedState,
    conversation_id: &str,
    model: &str,
    parsed: &ParsedChat,
    extra_system: &[String],
) -> Result<AgentPrompt, String> {
    build_agent_prompt_inner(state, conversation_id, model, parsed, extra_system, &[]).await
}

async fn emit_tool_sse_events(
    tx: &tokio::sync::mpsc::Sender<ResponsesStreamEvent>,
    response: &mut copenai_openai::ResponseObject,
    calls: &[copenai_openai::ParsedFunctionCall],
) {
    for call in calls {
        let item = ToolLoopEngine::calls_to_output(std::slice::from_ref(call));
        let item = item.into_iter().next().unwrap();
        let idx = response.output.len();
        response.output.push(item.clone());
        let args = call.arguments.to_string();
        let _ = tx
            .send(ResponsesStreamEvent::OutputItemAdded {
                response: response.clone(),
                output_index: idx,
                item: item.clone(),
            })
            .await;
        if !args.is_empty() {
            let _ = tx
                .send(ResponsesStreamEvent::FunctionCallArgumentsDelta {
                    response: response.clone(),
                    output_index: idx,
                    delta: args.clone(),
                })
                .await;
        }
        let _ = tx
            .send(ResponsesStreamEvent::FunctionCallArgumentsDone {
                response: response.clone(),
                output_index: idx,
                arguments: args,
            })
            .await;
        let _ = tx
            .send(ResponsesStreamEvent::OutputItemDone {
                response: response.clone(),
                output_index: idx,
                item,
            })
            .await;
    }
}
