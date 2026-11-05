use super::{MAX_EVENT_BYTES, ProviderProgress, ProviderStep, ToolCall, append, frame_end};
use anyhow::{Context, Result, bail, ensure};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tokio::sync::mpsc;

#[derive(Default)]
struct Block {
    kind: String,
    id: String,
    name: String,
    text: String,
    input: Option<Value>,
    input_parts: String,
    thinking: String,
    signature: String,
    data: String,
}

pub(super) async fn read_stream(
    response: reqwest::Response,
    progress: Option<&mpsc::Sender<ProviderProgress>>,
) -> Result<ProviderStep> {
    let mut stream = response.bytes_stream();
    let mut pending = Vec::new();
    let mut blocks = BTreeMap::<usize, Block>::new();
    let mut text = String::new();
    let mut completed = false;
    let mut stop_reason = None;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Anthropic stream interrupted")?;
        ensure!(
            pending.len().saturating_add(chunk.len()) <= MAX_EVENT_BYTES,
            "provider event is too large"
        );
        pending.extend_from_slice(&chunk);
        while let Some(end) = frame_end(&pending) {
            let frame: Vec<u8> = pending.drain(..end).collect();
            let frame = std::str::from_utf8(&frame).context("invalid Anthropic event encoding")?;
            let Some(data) = frame
                .lines()
                .find_map(|line| line.trim_end_matches('\r').strip_prefix("data: "))
            else {
                continue;
            };
            let event: Value =
                serde_json::from_str(data).context("invalid Anthropic event JSON")?;
            match event.get("type").and_then(Value::as_str).unwrap_or("") {
                "content_block_start" => {
                    let index = event["index"]
                        .as_u64()
                        .context("missing content block index")?
                        as usize;
                    ensure!(
                        index < 32 && !blocks.contains_key(&index),
                        "invalid Anthropic content block"
                    );
                    let content = &event["content_block"];
                    let kind = content["type"].as_str().unwrap_or("");
                    let block = Block {
                        kind: kind.to_owned(),
                        id: content["id"].as_str().unwrap_or("").to_owned(),
                        name: content["name"].as_str().unwrap_or("").to_owned(),
                        text: content["text"].as_str().unwrap_or("").to_owned(),
                        input: content.get("input").cloned(),
                        input_parts: String::new(),
                        thinking: content["thinking"].as_str().unwrap_or("").to_owned(),
                        signature: content["signature"].as_str().unwrap_or("").to_owned(),
                        data: content["data"].as_str().unwrap_or("").to_owned(),
                    };
                    if block.kind == "text" && !block.text.is_empty() {
                        append(&mut text, &block.text)?;
                        if let Some(tx) = progress {
                            let _ = tx
                                .send(ProviderProgress::MessageDelta(block.text.clone()))
                                .await;
                        }
                    }
                    blocks.insert(index, block);
                }
                "content_block_delta" => {
                    let index = event["index"]
                        .as_u64()
                        .context("missing content block index")?
                        as usize;
                    let block = blocks
                        .get_mut(&index)
                        .context("unknown Anthropic content block")?;
                    let delta = &event["delta"];
                    match delta["type"].as_str().unwrap_or("") {
                        "text_delta" if block.kind == "text" => {
                            let value = delta["text"].as_str().context("missing text delta")?;
                            append(&mut text, value)?;
                            append(&mut block.text, value)?;
                            if !value.is_empty()
                                && let Some(tx) = progress
                            {
                                let _ = tx
                                    .send(ProviderProgress::MessageDelta(value.to_owned()))
                                    .await;
                            }
                        }
                        "input_json_delta" if block.kind == "tool_use" => {
                            let value = delta["partial_json"]
                                .as_str()
                                .context("missing tool input delta")?;
                            ensure!(
                                block.input_parts.len().saturating_add(value.len()) <= 65_536,
                                "tool arguments are too large"
                            );
                            block.input_parts.push_str(value);
                        }
                        "thinking_delta" if block.kind == "thinking" => {
                            append(
                                &mut block.thinking,
                                delta["thinking"]
                                    .as_str()
                                    .context("missing thinking delta")?,
                            )?;
                        }
                        "signature_delta" if block.kind == "thinking" => {
                            append(
                                &mut block.signature,
                                delta["signature"]
                                    .as_str()
                                    .context("missing signature delta")?,
                            )?;
                        }
                        _ => {}
                    }
                }
                "message_delta" => {
                    stop_reason = event["delta"]["stop_reason"].as_str().map(str::to_owned);
                }
                "message_stop" => {
                    completed = true;
                    break;
                }
                "error" => bail!("Anthropic rejected the streamed message"),
                _ => {}
            }
        }
        if completed {
            break;
        }
    }
    ensure!(completed, "Anthropic stream ended without completion");
    ensure!(
        stop_reason.as_deref() != Some("max_tokens"),
        "Anthropic output reached its token limit"
    );
    let mut calls = Vec::new();
    let mut continuation = Vec::new();
    for block in blocks.into_values() {
        match block.kind.as_str() {
            "text" => {
                if !block.text.is_empty() {
                    continuation.push(json!({"type":"text","text":block.text}));
                }
            }
            "tool_use" => {
                ensure!(
                    !block.id.is_empty() && !block.name.is_empty(),
                    "invalid Anthropic tool call"
                );
                let input = if block.input_parts.is_empty() {
                    block.input.unwrap_or_else(|| json!({}))
                } else {
                    serde_json::from_str(&block.input_parts)
                        .context("invalid Anthropic tool input")?
                };
                ensure!(input.is_object(), "Anthropic tool input must be an object");
                calls.push(ToolCall {
                    id: block.id.clone(),
                    name: block.name.clone(),
                    arguments: serde_json::to_string(&input)?,
                });
                continuation
                    .push(json!({"type":"tool_use","id":block.id,"name":block.name,"input":input}));
            }
            "thinking" => {
                ensure!(
                    !block.signature.is_empty(),
                    "unsigned Anthropic thinking block"
                );
                continuation.push(json!({"type":"thinking","thinking":block.thinking,"signature":block.signature}));
            }
            "redacted_thinking" => {
                ensure!(
                    !block.data.is_empty(),
                    "invalid Anthropic redacted thinking block"
                );
                continuation.push(json!({"type":"redacted_thinking","data":block.data}));
            }
            _ => {}
        }
    }
    ensure!(calls.len() <= 16, "too many provider tool calls");
    ensure!(
        stop_reason.as_deref() != Some("tool_use") || !calls.is_empty(),
        "Anthropic returned no tool calls"
    );
    Ok(ProviderStep {
        response_id: None,
        text,
        calls,
        reasoning_summaries: Vec::new(),
        continuation: if continuation.is_empty() {
            Vec::new()
        } else {
            vec![json!({"role":"assistant","content":continuation})]
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Bytes, http::Response};
    use futures_util::stream;
    use std::convert::Infallible;

    #[tokio::test]
    async fn streams_text_and_preserves_tool_use_for_next_step() {
        let chunks = stream::unfold(0, |index| async move {
            let body = match index {
                0 => {
                    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"Example \"}}\n\n"
                }
                1 => {
                    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"reply\"}}\n\n"
                }
                2 => {
                    "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool-example\",\"name\":\"read\",\"input\":{}}}\n\n"
                }
                3 => {
                    "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"example.txt\\\"}\"}}\n\n"
                }
                4 => {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    "data: {\"type\":\"message_stop\"}\n\n"
                }
                _ => return None,
            };
            Some((
                Ok::<_, Infallible>(Bytes::from_static(body.as_bytes())),
                index + 1,
            ))
        });
        let response: reqwest::Response = Response::new(reqwest::Body::wrap_stream(chunks)).into();
        let (tx, mut rx) = mpsc::channel(8);
        let reader = tokio::spawn(async move { read_stream(response, Some(&tx)).await });
        assert!(
            matches!(rx.recv().await, Some(ProviderProgress::MessageDelta(value)) if value == "Example ")
        );
        assert!(
            matches!(rx.recv().await, Some(ProviderProgress::MessageDelta(value)) if value == "reply")
        );
        assert!(!reader.is_finished());
        let step = reader.await.unwrap().unwrap();
        assert_eq!(step.text, "Example reply");
        assert_eq!(step.calls[0].arguments, "{\"path\":\"example.txt\"}");
        assert_eq!(step.continuation[0]["content"][1]["type"], "tool_use");
    }

    #[tokio::test]
    async fn signed_thinking_and_parallel_tool_calls_survive_continuation() {
        let body = [
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"example-signature"}}"#,
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"redacted_thinking","data":"example-opaque-data"}}"#,
            r#"data: {"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"tool-one","name":"read","input":{"path":"one.txt"}}}"#,
            r#"data: {"type":"content_block_start","index":3,"content_block":{"type":"tool_use","id":"tool-two","name":"read","input":{"path":"two.txt"}}}"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"}}"#,
            r#"data: {"type":"message_stop"}"#,
        ].join("\n\n") + "\n\n";
        let response: reqwest::Response = Response::new(reqwest::Body::from(body)).into();
        let step = read_stream(response, None).await.unwrap();
        assert_eq!(step.calls.len(), 2);
        assert_eq!(
            step.continuation[0]["content"][0],
            json!({"type":"thinking","thinking":"","signature":"example-signature"})
        );
        assert_eq!(
            step.continuation[0]["content"][1],
            json!({"type":"redacted_thinking","data":"example-opaque-data"})
        );
        assert_eq!(step.continuation[0]["content"][2]["id"], "tool-one");
    }
}
