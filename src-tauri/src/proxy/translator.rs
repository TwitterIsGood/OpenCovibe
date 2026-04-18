/// Protocol translation between Anthropic and OpenAI formats.
///
/// Inbound: always Anthropic (Claude CLI speaks Anthropic).
/// Outbound: Anthropic or OpenAI depending on provider's `protocol` setting.

/// Translate an Anthropic /v1/messages request to an OpenAI /v1/chat/completions request.
pub fn anthropic_to_openai_request(
    anthropic: &serde_json::Value,
    model: &str,
) -> serde_json::Value {
    let mut messages = Vec::new();

    // System prompt -> system message
    if let Some(system) = anthropic.get("system") {
        messages.push(serde_json::json!({
            "role": "system",
            "content": system,
        }));
    }

    // Convert messages
    if let Some(msgs) = anthropic.get("messages").and_then(|m| m.as_array()) {
        for msg in msgs {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let content = msg.get("content");

            if let Some(content) = content {
                if content.is_string() {
                    messages.push(serde_json::json!({
                        "role": role,
                        "content": content,
                    }));
                } else if let Some(blocks) = content.as_array() {
                    // Convert content blocks to OpenAI format
                    let mut parts = Vec::new();
                    for block in blocks {
                        if let Some(block_type) = block.get("type").and_then(|t| t.as_str()) {
                            match block_type {
                                "text" => {
                                    if let Some(text) = block.get("text") {
                                        parts.push(serde_json::json!({
                                            "type": "text",
                                            "text": text,
                                        }));
                                    }
                                }
                                "image" => {
                                    if let (Some(source), Some(media_type)) = (
                                        block.get("source"),
                                        block
                                            .get("source")
                                            .and_then(|s| s.get("media_type"))
                                            .and_then(|t| t.as_str()),
                                    ) {
                                        if let Some(data) = source.get("data").and_then(|d| d.as_str())
                                        {
                                            parts.push(serde_json::json!({
                                                "type": "image_url",
                                                "image_url": {
                                                    "url": format!("data:{media_type};base64,{data}")
                                                }
                                            }));
                                        }
                                    }
                                }
                                "tool_use" => {
                                    // Tool use from assistant -> function call
                                    if let (Some(name), Some(id), Some(input)) = (
                                        block.get("name").and_then(|n| n.as_str()),
                                        block.get("id").and_then(|i| i.as_str()),
                                        block.get("input"),
                                    ) {
                                        parts.push(serde_json::json!({
                                            "type": "function",
                                            "id": id,
                                            "function": {
                                                "name": name,
                                                "arguments": input.to_string(),
                                            }
                                        }));
                                    }
                                }
                                "tool_result" => {
                                    // Tool result -> tool message
                                    if let (Some(id), Some(content)) = (
                                        block.get("tool_use_id").and_then(|i| i.as_str()),
                                        block.get("content"),
                                    ) {
                                        let content_str = if content.is_string() {
                                            content.as_str().unwrap_or("").to_string()
                                        } else {
                                            content.to_string()
                                        };
                                        messages.push(serde_json::json!({
                                            "role": "tool",
                                            "tool_call_id": id,
                                            "content": content_str,
                                        }));
                                        continue; // Don't add to parts
                                    }
                                }
                                _ => {
                                    // Pass through as-is
                                    parts.push(block.clone());
                                }
                            }
                        }
                    }

                    if !parts.is_empty() {
                        messages.push(serde_json::json!({
                            "role": role,
                            "content": parts,
                        }));
                    }
                }
            }
        }
    }

    // Convert tools
    let tools = anthropic
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|tools| {
            tools
                .iter()
                .map(|tool| {
                    let name = tool.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let description = tool
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("");
                    let parameters = tool
                        .get("input_schema")
                        .cloned()
                        .unwrap_or(serde_json::json!({}));
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": name,
                            "description": description,
                            "parameters": parameters,
                        }
                    })
                })
                .collect::<Vec<_>>()
        });

    let mut result = serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": anthropic.get("stream").and_then(|s| s.as_bool()).unwrap_or(false),
    });

    if let Some(max_tokens) = anthropic.get("max_tokens") {
        result["max_tokens"] = max_tokens.clone();
    }

    if let Some(tools) = tools {
        result["tools"] = serde_json::Value::Array(tools);
    }

    if let Some(temperature) = anthropic.get("temperature") {
        result["temperature"] = temperature.clone();
    }

    if let Some(top_p) = anthropic.get("top_p") {
        result["top_p"] = top_p.clone();
    }

    result
}

/// Translate an OpenAI non-streaming response to Anthropic format.
pub fn openai_to_anthropic_response(body: &[u8]) -> Vec<u8> {
    let openai: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return body.to_vec(),
    };

    let mut content = Vec::new();
    let mut stop_reason = "end_turn";

    if let Some(choices) = openai.get("choices").and_then(|c| c.as_array()) {
        if let Some(choice) = choices.first() {
            if let Some(message) = choice.get("message") {
                // Text content
                if let Some(text) = message.get("content").and_then(|c| c.as_str()) {
                    if !text.is_empty() {
                        content.push(serde_json::json!({
                            "type": "text",
                            "text": text,
                        }));
                    }
                }

                // Tool calls
                if let Some(tool_calls) = message.get("tool_calls").and_then(|t| t.as_array()) {
                    for tc in tool_calls {
                        if let (Some(id), Some(func)) =
                            (tc.get("id").and_then(|i| i.as_str()), tc.get("function"))
                        {
                            let name = func.get("name").and_then(|n| n.as_str()).unwrap_or("");
                            let arguments =
                                func.get("arguments").and_then(|a| a.as_str()).unwrap_or("{}");
                            let input: serde_json::Value =
                                serde_json::from_str(arguments).unwrap_or(serde_json::json!({}));
                            content.push(serde_json::json!({
                                "type": "tool_use",
                                "id": id,
                                "name": name,
                                "input": input,
                            }));
                        }
                    }
                }
            }

            if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
                stop_reason = match reason {
                    "stop" => "end_turn",
                    "tool_calls" | "function_call" => "tool_use",
                    "length" => "max_tokens",
                    _ => "end_turn",
                };
            }
        }
    }

    if content.is_empty() {
        content.push(serde_json::json!({"type": "text", "text": ""}));
    }

    let anthropic = serde_json::json!({
        "id": openai.get("id").unwrap_or(&serde_json::Value::String("msg_proxy".to_string())),
        "type": "message",
        "role": "assistant",
        "content": content,
        "model": openai.get("model").unwrap_or(&serde_json::Value::String("unknown".to_string())),
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": {
            "input_tokens": openai.get("usage").and_then(|u| u.get("prompt_tokens")).unwrap_or(&serde_json::json!(0)),
            "output_tokens": openai.get("usage").and_then(|u| u.get("completion_tokens")).unwrap_or(&serde_json::json!(0)),
        }
    });

    serde_json::to_vec(&anthropic).unwrap_or_else(|_| body.to_vec())
}

/// Translate a single OpenAI SSE chunk to Anthropic SSE events.
/// Returns a list of "event: ...\ndata: ...\n\n" formatted strings.
pub fn translate_openai_sse_chunk(chunk: &serde_json::Value) -> Option<Vec<String>> {
    let mut events = Vec::new();

    let choices = chunk.get("choices")?.as_array()?;
    if choices.is_empty() {
        return None;
    }

    let choice = &choices[0];
    let index = choice.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;

    // Check if this is the first chunk (has role)
    if let Some(_role) = choice
        .get("delta")
        .and_then(|d| d.get("role"))
    {
        // message_start
        events.push(format_sse(
            "message_start",
            &serde_json::json!({
                "type": "message_start",
                "message": {
                    "id": chunk.get("id").unwrap_or(&serde_json::Value::String("msg_proxy".to_string())),
                    "type": "message",
                    "role": "assistant",
                    "content": [],
                    "model": chunk.get("model").unwrap_or(&serde_json::Value::String("unknown".to_string())),
                    "stop_reason": null,
                    "stop_sequence": null,
                    "usage": {"input_tokens": 0, "output_tokens": 0},
                }
            }),
        ));
    }

    if let Some(delta) = choice.get("delta") {
        // Text content delta
        if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
            if !content.is_empty() {
                // content_block_start if first content
                if events.is_empty() {
                    events.push(format_sse(
                        "content_block_start",
                        &serde_json::json!({
                            "type": "content_block_start",
                            "index": 0,
                            "content_block": {"type": "text", "text": ""}
                        }),
                    ));
                }
                events.push(format_sse(
                    "content_block_delta",
                    &serde_json::json!({
                        "type": "content_block_delta",
                        "index": 0,
                        "delta": {"type": "text_delta", "text": content}
                    }),
                ));
            }
        }

        // Tool calls delta
        if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
            for tc in tool_calls {
                let tc_index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                if let (Some(id), Some(func)) =
                    (tc.get("id").and_then(|i| i.as_str()), tc.get("function"))
                {
                    // Tool call start
                    let name = func.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    if !name.is_empty() {
                        events.push(format_sse(
                            "content_block_start",
                            &serde_json::json!({
                                "type": "content_block_start",
                                "index": tc_index + 1,
                                "content_block": {
                                    "type": "tool_use",
                                    "id": id,
                                    "name": name,
                                    "input": {}
                                }
                            }),
                        ));
                    }
                }
                if let Some(arguments) = delta
                    .get("tool_calls")
                    .and_then(|t| t.as_array())
                    .and_then(|t| t.first())
                    .and_then(|tc| tc.get("function"))
                    .and_then(|f| f.get("arguments"))
                    .and_then(|a| a.as_str())
                {
                    if !arguments.is_empty() {
                        events.push(format_sse(
                            "content_block_delta",
                            &serde_json::json!({
                                "type": "content_block_delta",
                                "index": index + 1,
                                "delta": {
                                    "type": "input_json_delta",
                                    "partial_json": arguments,
                                }
                            }),
                        ));
                    }
                }
            }
        }
    }

    // Finish reason
    if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
        // Close any open content blocks
        events.push(format_sse(
            "content_block_stop",
            &serde_json::json!({"type": "content_block_stop", "index": 0}),
        ));

        let stop_reason = match reason {
            "stop" => "end_turn",
            "tool_calls" | "function_call" => "tool_use",
            "length" => "max_tokens",
            _ => "end_turn",
        };
        events.push(format_sse(
            "message_delta",
            &serde_json::json!({
                "type": "message_delta",
                "delta": {"stop_reason": stop_reason, "stop_sequence": null},
                "usage": {"output_tokens": 0}
            }),
        ));
        events.push(format_sse(
            "message_stop",
            &serde_json::json!({"type": "message_stop"}),
        ));
    }

    if events.is_empty() {
        None
    } else {
        Some(events)
    }
}

/// Format an SSE event string.
fn format_sse(event: &str, data: &serde_json::Value) -> String {
    format!("event: {event}\ndata: {}\n\n", data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── anthropic_to_openai_request tests ──

    #[test]
    fn translate_simple_text_message() {
        let anthropic = json!({
            "model": "claude-3-sonnet",
            "messages": [
                {"role": "user", "content": "Hello"}
            ],
            "max_tokens": 1024,
            "stream": true,
        });
        let result = anthropic_to_openai_request(&anthropic, "gpt-4");
        assert_eq!(result["model"], "gpt-4");
        assert_eq!(result["stream"], true);
        assert_eq!(result["max_tokens"], 1024);
        let msgs = result["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "Hello");
    }

    #[test]
    fn translate_system_prompt() {
        let anthropic = json!({
            "system": "You are helpful.",
            "messages": [{"role": "user", "content": "Hi"}],
        });
        let result = anthropic_to_openai_request(&anthropic, "test");
        let msgs = result["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "You are helpful.");
        assert_eq!(msgs[1]["role"], "user");
    }

    #[test]
    fn translate_tool_use_blocks() {
        let anthropic = json!({
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Let me check."},
                    {"type": "tool_use", "id": "tool_123", "name": "get_weather", "input": {"city": "Tokyo"}},
                ]
            }],
        });
        let result = anthropic_to_openai_request(&anthropic, "test");
        let msgs = result["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        let content = msgs[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "function");
        assert_eq!(content[1]["function"]["name"], "get_weather");
    }

    #[test]
    fn translate_tool_result_blocks() {
        let anthropic = json!({
            "messages": [
                {"role": "user", "content": "What's the weather?"},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "tool_abc", "name": "weather", "input": {}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "tool_abc", "content": "Sunny, 22°C"}
                ]},
            ],
        });
        let result = anthropic_to_openai_request(&anthropic, "test");
        let msgs = result["messages"].as_array().unwrap();
        // system? no. user, assistant, tool
        let tool_msg = &msgs[2];
        assert_eq!(tool_msg["role"], "tool");
        assert_eq!(tool_msg["tool_call_id"], "tool_abc");
        assert_eq!(tool_msg["content"], "Sunny, 22°C");
    }

    #[test]
    fn translate_tools_definition() {
        let anthropic = json!({
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{
                "name": "search",
                "description": "Search the web",
                "input_schema": {"type": "object", "properties": {"q": {"type": "string"}}}
            }],
        });
        let result = anthropic_to_openai_request(&anthropic, "test");
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["name"], "search");
        assert!(tools[0]["function"].get("parameters").is_some());
    }

    #[test]
    fn translate_image_block() {
        let anthropic = json!({
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBOR"}},
                    {"type": "text", "text": "What is this?"},
                ]
            }],
        });
        let result = anthropic_to_openai_request(&anthropic, "test");
        let content = result["messages"].as_array().unwrap()[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "image_url");
        assert!(content[0]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,"));
    }

    // ── openai_to_anthropic_response tests ──

    #[test]
    fn translate_text_response() {
        let openai = json!({
            "id": "chatcmpl-123",
            "model": "gpt-4",
            "choices": [{
                "message": {"role": "assistant", "content": "Hello!"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        let result = openai_to_anthropic_response(serde_json::to_vec(&openai).unwrap().as_slice());
        let anthropic: serde_json::Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(anthropic["type"], "message");
        assert_eq!(anthropic["role"], "assistant");
        assert_eq!(anthropic["stop_reason"], "end_turn");
        let content = anthropic["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Hello!");
        assert_eq!(anthropic["usage"]["input_tokens"], 10);
        assert_eq!(anthropic["usage"]["output_tokens"], 5);
    }

    #[test]
    fn translate_tool_calls_response() {
        let openai = json!({
            "id": "chatcmpl-456",
            "model": "gpt-4",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {"name": "search", "arguments": "{\"q\": \"weather\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 20, "completion_tokens": 10}
        });
        let result = openai_to_anthropic_response(serde_json::to_vec(&openai).unwrap().as_slice());
        let anthropic: serde_json::Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(anthropic["stop_reason"], "tool_use");
        let content = anthropic["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["name"], "search");
        assert_eq!(content[0]["input"]["q"], "weather");
    }

    #[test]
    fn translate_max_tokens_stop() {
        let openai = json!({
            "id": "chatcmpl-789",
            "choices": [{
                "message": {"content": "partial"},
                "finish_reason": "length"
            }],
            "usage": {"prompt_tokens": 0, "completion_tokens": 0}
        });
        let result = openai_to_anthropic_response(serde_json::to_vec(&openai).unwrap().as_slice());
        let anthropic: serde_json::Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(anthropic["stop_reason"], "max_tokens");
    }

    #[test]
    fn translate_invalid_json_passthrough() {
        let body = b"not json at all";
        let result = openai_to_anthropic_response(body);
        assert_eq!(result, b"not json at all");
    }

    // ── translate_openai_sse_chunk tests ──

    #[test]
    fn sse_first_chunk_with_role() {
        let chunk = json!({
            "id": "chatcmpl-1",
            "model": "gpt-4",
            "choices": [{"delta": {"role": "assistant", "content": ""}, "index": 0}]
        });
        let events = translate_openai_sse_chunk(&chunk).unwrap();
        assert!(events[0].contains("event: message_start"));
        assert!(events[0].contains("\"type\":\"message_start\""));
    }

    #[test]
    fn sse_text_delta() {
        let chunk = json!({
            "id": "chatcmpl-1",
            "choices": [{"delta": {"content": "Hello world"}, "index": 0}]
        });
        let events = translate_openai_sse_chunk(&chunk).unwrap();
        assert!(events[0].contains("event: content_block_start"));
        assert!(events[1].contains("event: content_block_delta"));
        assert!(events[1].contains("\"text\":\"Hello world\""));
    }

    #[test]
    fn sse_finish_stop() {
        let chunk = json!({
            "id": "chatcmpl-1",
            "choices": [{"delta": {}, "finish_reason": "stop", "index": 0}]
        });
        let events = translate_openai_sse_chunk(&chunk).unwrap();
        // Should have content_block_stop + message_delta + message_stop
        assert!(events.iter().any(|e| e.contains("event: content_block_stop")));
        assert!(events.iter().any(|e| e.contains("event: message_delta")));
        assert!(events.iter().any(|e| e.contains("\"stop_reason\":\"end_turn\"")));
        assert!(events.iter().any(|e| e.contains("event: message_stop")));
    }

    #[test]
    fn sse_finish_tool_use() {
        let chunk = json!({
            "id": "chatcmpl-1",
            "choices": [{"delta": {}, "finish_reason": "tool_calls", "index": 0}]
        });
        let events = translate_openai_sse_chunk(&chunk).unwrap();
        assert!(events.iter().any(|e| e.contains("\"stop_reason\":\"tool_use\"")));
    }

    #[test]
    fn sse_empty_choices_returns_none() {
        let chunk = json!({"choices": []});
        assert!(translate_openai_sse_chunk(&chunk).is_none());
    }

    #[test]
    fn sse_empty_content_delta_skipped() {
        let chunk = json!({
            "id": "chatcmpl-1",
            "choices": [{"delta": {"content": ""}, "index": 0}]
        });
        // Empty string content should produce no events
        assert!(translate_openai_sse_chunk(&chunk).is_none());
    }
}
