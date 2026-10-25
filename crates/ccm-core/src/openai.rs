//! OpenAI Chat Completions ⇄ Anthropic Messages translation for
//! `POST /v1/chat/completions`.

use anyhow::{bail, Result};
use serde_json::{json, Map, Value};

/// Default `max_tokens` when the OpenAI request sets none (Anthropic
/// requires one).
const DEFAULT_MAX_TOKENS: i64 = 8192;

/// Options the response side needs to know about.
#[derive(Debug, Clone, Default)]
pub struct RequestInfo {
    /// The model name the client asked for, echoed back in responses.
    pub model: String,
    pub stream: bool,
    /// `stream_options.include_usage`: send a final usage chunk.
    pub include_usage: bool,
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// OpenAI content (string or parts) → Anthropic content blocks.
fn content_blocks(content: &Value) -> Result<Vec<Value>> {
    match content {
        Value::Null => Ok(vec![]),
        Value::String(s) if s.is_empty() => Ok(vec![]),
        Value::String(s) => Ok(vec![json!({"type": "text", "text": s})]),
        Value::Array(parts) => parts.iter().map(part_block).collect(),
        _ => bail!("message content must be a string or an array of parts"),
    }
}

fn part_block(p: &Value) -> Result<Value> {
    match p["type"].as_str() {
        Some("text") => Ok(json!({"type": "text", "text": p["text"].as_str().unwrap_or_default()})),
        Some("image_url") => {
            let url = p["image_url"]["url"]
                .as_str()
                .or_else(|| p["image_url"].as_str())
                .unwrap_or_default();
            image_block(url)
        }
        other => bail!("unsupported content part type {other:?}"),
    }
}

fn image_block(url: &str) -> Result<Value> {
    if let Some(rest) = url.strip_prefix("data:") {
        let Some((meta, data)) = rest.split_once(',') else {
            bail!("malformed data URL");
        };
        let media_type = meta.strip_suffix(";base64").unwrap_or(meta);
        if !meta.ends_with(";base64") {
            bail!("image data URLs must be base64-encoded");
        }
        return Ok(
            json!({"type": "image", "source": {"type": "base64", "media_type": media_type, "data": data}}),
        );
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return Ok(json!({"type": "image", "source": {"type": "url", "url": url}}));
    }
    bail!("image_url must be a data: or http(s) URL")
}

/// Appends a turn, merging into the previous one when the role repeats.
fn push_turn(messages: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if blocks.is_empty() {
        return;
    }
    if let Some(last) = messages.last_mut() {
        if last["role"] == role {
            if let Some(arr) = last["content"].as_array_mut() {
                arr.extend(blocks);
                return;
            }
        }
    }
    messages.push(json!({"role": role, "content": blocks}));
}

/// Translates an OpenAI chat completion request into an Anthropic Messages
/// body. `reasoning_effort` becomes this proxy's `effort` field.
pub fn to_anthropic(req: &Value) -> Result<(Value, RequestInfo)> {
    let Some(input) = req["messages"].as_array() else {
        bail!("messages is required");
    };
    let model = req["model"].as_str().unwrap_or_default().to_string();
    if model.is_empty() {
        bail!("model is required");
    }
    let mut system: Vec<Value> = vec![];
    let mut messages: Vec<Value> = vec![];
    for m in input {
        match m["role"].as_str() {
            Some("system") | Some("developer") => {
                let t = text_of(&m["content"]);
                if !t.is_empty() {
                    system.push(json!({"type": "text", "text": t}));
                }
            }
            Some("user") => push_turn(&mut messages, "user", content_blocks(&m["content"])?),
            Some("assistant") => {
                let mut blocks = content_blocks(&m["content"])?;
                for call in m["tool_calls"].as_array().into_iter().flatten() {
                    let args = call["function"]["arguments"].as_str().unwrap_or("{}");
                    let input: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": call["id"],
                        "name": call["function"]["name"],
                        "input": input,
                    }));
                }
                push_turn(&mut messages, "assistant", blocks);
            }
            Some("tool") => push_turn(
                &mut messages,
                "user",
                vec![json!({
                    "type": "tool_result",
                    "tool_use_id": m["tool_call_id"],
                    "content": text_of(&m["content"]),
                })],
            ),
            other => bail!("unsupported message role {other:?}"),
        }
    }

    let mut out = Map::new();
    out.insert("model".into(), json!(model));
    if !system.is_empty() {
        out.insert("system".into(), Value::Array(system));
    }
    out.insert("messages".into(), Value::Array(messages));
    let max_tokens = req["max_completion_tokens"]
        .as_i64()
        .or_else(|| req["max_tokens"].as_i64());
    match (max_tokens, req["reasoning_effort"].as_str()) {
        // Let the effort tier pick max_tokens / thinking.
        (None, Some(effort)) => {
            out.insert("effort".into(), json!(effort));
        }
        (m, effort) => {
            out.insert("max_tokens".into(), json!(m.unwrap_or(DEFAULT_MAX_TOKENS)));
            if let Some(e) = effort {
                out.insert("effort".into(), json!(e));
            }
        }
    }
    for k in ["temperature", "top_p"] {
        if let Some(v) = req.get(k).filter(|v| !v.is_null()) {
            out.insert(k.into(), v.clone());
        }
    }
    match &req["stop"] {
        Value::String(s) => {
            out.insert("stop_sequences".into(), json!([s]));
        }
        Value::Array(a) if !a.is_empty() => {
            out.insert("stop_sequences".into(), Value::Array(a.clone()));
        }
        _ => {}
    }
    if let Some(tools) = req["tools"].as_array().filter(|t| !t.is_empty()) {
        let tools: Vec<Value> = tools
            .iter()
            .filter(|t| t["type"] == "function")
            .map(|t| {
                let f = &t["function"];
                let mut tool = json!({
                    "name": f["name"],
                    "input_schema": if f["parameters"].is_object() { f["parameters"].clone() } else { json!({"type": "object"}) },
                });
                if let Some(d) = f["description"].as_str() {
                    tool["description"] = json!(d);
                }
                tool
            })
            .collect();
        out.insert("tools".into(), Value::Array(tools));
    }
    let choice = match &req["tool_choice"] {
        Value::String(s) if s == "auto" => Some(json!({"type": "auto"})),
        Value::String(s) if s == "none" => Some(json!({"type": "none"})),
        Value::String(s) if s == "required" => Some(json!({"type": "any"})),
        Value::Object(o) if o.get("type").and_then(Value::as_str) == Some("function") => {
            Some(json!({"type": "tool", "name": o["function"]["name"]}))
        }
        _ => None,
    };
    if let Some(c) = choice {
        out.insert("tool_choice".into(), c);
    }
    if let Some(u) = req["user"].as_str() {
        out.insert("metadata".into(), json!({"user_id": u}));
    }
    let stream = req["stream"].as_bool().unwrap_or(false);
    if stream {
        out.insert("stream".into(), json!(true));
    }
    Ok((
        Value::Object(out),
        RequestInfo {
            model,
            stream,
            include_usage: req["stream_options"]["include_usage"].as_bool() == Some(true),
        },
    ))
}

pub fn finish_reason(stop_reason: &str) -> &'static str {
    match stop_reason {
        "max_tokens" => "length",
        "tool_use" => "tool_calls",
        "refusal" => "content_filter",
        _ => "stop",
    }
}

fn usage(u: &Value) -> Value {
    let prompt = u["input_tokens"].as_i64().unwrap_or(0)
        + u["cache_read_input_tokens"].as_i64().unwrap_or(0)
        + u["cache_creation_input_tokens"].as_i64().unwrap_or(0);
    let completion = u["output_tokens"].as_i64().unwrap_or(0);
    json!({
        "prompt_tokens": prompt,
        "completion_tokens": completion,
        "total_tokens": prompt + completion,
        "prompt_tokens_details": {"cached_tokens": u["cache_read_input_tokens"].as_i64().unwrap_or(0)},
    })
}

/// Translates a complete Anthropic Messages response.
pub fn from_anthropic(resp: &Value, info: &RequestInfo) -> Value {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls = vec![];
    for block in resp["content"].as_array().into_iter().flatten() {
        match block["type"].as_str() {
            Some("text") => text.push_str(block["text"].as_str().unwrap_or_default()),
            Some("thinking") => reasoning.push_str(block["thinking"].as_str().unwrap_or_default()),
            Some("tool_use") => tool_calls.push(json!({
                "id": block["id"],
                "type": "function",
                "function": {"name": block["name"], "arguments": block["input"].to_string()},
            })),
            _ => {}
        }
    }
    let mut message = json!({
        "role": "assistant",
        "content": if text.is_empty() && !tool_calls.is_empty() { Value::Null } else { json!(text) },
    });
    if !tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(tool_calls);
    }
    if !reasoning.is_empty() {
        message["reasoning_content"] = json!(reasoning);
    }
    json!({
        "id": format!("chatcmpl-{}", resp["id"].as_str().unwrap_or_default()),
        "object": "chat.completion",
        "created": crate::now_unix(),
        "model": info.model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason(resp["stop_reason"].as_str().unwrap_or("end_turn")),
        }],
        "usage": usage(&resp["usage"]),
    })
}

/// Rewrites an Anthropic SSE stream into OpenAI `chat.completion.chunk`
/// events, incrementally (chunk boundaries may split lines).
pub struct StreamTranslator {
    info: RequestInfo,
    pending: Vec<u8>,
    event: String,
    id: String,
    created: i64,
    /// Anthropic content-block index → OpenAI tool_calls index.
    tool_index: Vec<(i64, usize)>,
    usage: Value,
    finished: bool,
}

impl StreamTranslator {
    pub fn new(info: RequestInfo) -> Self {
        Self {
            info,
            pending: vec![],
            event: String::new(),
            id: String::new(),
            created: crate::now_unix(),
            tool_index: vec![],
            usage: json!({}),
            finished: false,
        }
    }

    /// Consumes upstream bytes, returns translated SSE bytes (possibly empty).
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                self.event.clear();
            } else if let Some(ev) = line.strip_prefix("event:") {
                self.event = ev.trim().to_string();
            } else if let Some(data) = line.strip_prefix("data:") {
                if let Ok(v) = serde_json::from_str::<Value>(data.trim()) {
                    let kind = if self.event.is_empty() {
                        v["type"].as_str().unwrap_or_default().to_string()
                    } else {
                        self.event.clone()
                    };
                    self.handle(&kind, &v, &mut out);
                }
            }
        }
        out
    }

    fn chunk(&self, delta: Value, finish: Option<&str>) -> Value {
        json!({
            "id": format!("chatcmpl-{}", self.id),
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.info.model,
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
        })
    }

    fn emit(out: &mut Vec<u8>, v: &Value) {
        out.extend_from_slice(b"data: ");
        out.extend_from_slice(v.to_string().as_bytes());
        out.extend_from_slice(b"\n\n");
    }

    fn handle(&mut self, kind: &str, v: &Value, out: &mut Vec<u8>) {
        match kind {
            "message_start" => {
                self.id = v["message"]["id"].as_str().unwrap_or_default().to_string();
                self.usage = v["message"]["usage"].clone();
                Self::emit(
                    out,
                    &self.chunk(json!({"role": "assistant", "content": ""}), None),
                );
            }
            "content_block_start" if v["content_block"]["type"] == "tool_use" => {
                let idx = self.tool_index.len();
                self.tool_index
                    .push((v["index"].as_i64().unwrap_or(-1), idx));
                let b = &v["content_block"];
                let delta = json!({"tool_calls": [{
                    "index": idx, "id": b["id"], "type": "function",
                    "function": {"name": b["name"], "arguments": ""},
                }]});
                Self::emit(out, &self.chunk(delta, None));
            }
            "content_block_delta" => {
                let d = &v["delta"];
                let delta = match d["type"].as_str() {
                    Some("text_delta") => json!({"content": d["text"]}),
                    Some("thinking_delta") => json!({"reasoning_content": d["thinking"]}),
                    Some("input_json_delta") => {
                        let block = v["index"].as_i64().unwrap_or(-1);
                        let Some(&(_, idx)) = self.tool_index.iter().find(|(b, _)| *b == block)
                        else {
                            return;
                        };
                        json!({"tool_calls": [{"index": idx, "function": {"arguments": d["partial_json"]}}]})
                    }
                    _ => return,
                };
                Self::emit(out, &self.chunk(delta, None));
            }
            "message_delta" => {
                if let Some(o) = v["usage"]["output_tokens"].as_i64() {
                    self.usage["output_tokens"] = json!(o);
                }
                if let Some(i) = v["usage"]["input_tokens"].as_i64().filter(|i| *i > 0) {
                    self.usage["input_tokens"] = json!(i);
                }
                if let Some(reason) = v["delta"]["stop_reason"].as_str() {
                    Self::emit(out, &self.chunk(json!({}), Some(finish_reason(reason))));
                }
            }
            "message_stop" => self.finish(out),
            "error" => {
                Self::emit(out, &json!({"error": v["error"]}));
                self.finish(out);
            }
            _ => {}
        }
    }

    fn finish(&mut self, out: &mut Vec<u8>) {
        if self.finished {
            return;
        }
        self.finished = true;
        if self.info.include_usage {
            Self::emit(
                out,
                &json!({
                    "id": format!("chatcmpl-{}", self.id),
                    "object": "chat.completion.chunk",
                    "created": self.created,
                    "model": self.info.model,
                    "choices": [],
                    "usage": usage(&self.usage),
                }),
            );
        }
        out.extend_from_slice(b"data: [DONE]\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_translation() {
        let req = json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "system", "content": "Be terse."},
                {"role": "user", "content": [
                    {"type": "text", "text": "What's this?"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}
                ]},
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "call_1", "type": "function", "function": {"name": "lookup", "arguments": "{\"q\":\"x\"}"}}
                ]},
                {"role": "tool", "tool_call_id": "call_1", "content": "result"},
                {"role": "user", "content": "thanks"}
            ],
            "tools": [{"type": "function", "function": {"name": "lookup", "description": "d", "parameters": {"type": "object", "properties": {"q": {"type": "string"}}}}}],
            "tool_choice": "required",
            "max_tokens": 100,
            "stop": "END",
            "temperature": 0.2,
            "stream": true,
            "stream_options": {"include_usage": true},
            "user": "u-1"
        });
        let (out, info) = to_anthropic(&req).unwrap();
        assert_eq!(info.model, "gpt-4o");
        assert!(info.stream && info.include_usage);
        assert_eq!(out["system"][0]["text"], "Be terse.");
        let msgs = out["messages"].as_array().unwrap();
        assert_eq!(
            msgs.len(),
            3,
            "tool result and the next user text merge: {msgs:?}"
        );
        assert_eq!(
            msgs[0]["content"][1]["source"],
            json!({"type": "base64", "media_type": "image/png", "data": "AAAA"})
        );
        assert_eq!(
            msgs[1]["content"][0],
            json!({"type": "tool_use", "id": "call_1", "name": "lookup", "input": {"q": "x"}})
        );
        assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
        assert_eq!(msgs[2]["content"][1]["text"], "thanks");
        assert_eq!(
            out["tools"][0]["input_schema"]["properties"]["q"]["type"],
            "string"
        );
        assert_eq!(out["tool_choice"], json!({"type": "any"}));
        assert_eq!(out["max_tokens"], 100);
        assert_eq!(out["stop_sequences"], json!(["END"]));
        assert_eq!(out["metadata"]["user_id"], "u-1");
        assert_eq!(out["stream"], true);
    }

    #[test]
    fn reasoning_effort_maps_to_effort_tier() {
        let (out, _) =
            to_anthropic(&json!({"model": "m", "messages": [], "reasoning_effort": "high"}))
                .unwrap();
        assert_eq!(out["effort"], "high");
        assert!(out.get("max_tokens").is_none());
        let (out, _) = to_anthropic(&json!({"model": "m", "messages": []})).unwrap();
        assert_eq!(out["max_tokens"], DEFAULT_MAX_TOKENS);
        assert!(to_anthropic(&json!({"model": "m"})).is_err());
        assert!(to_anthropic(&json!({"model": "m", "messages": [{"role": "user", "content": [{"type": "image_url", "image_url": {"url": "ftp://x"}}]}]})).is_err());
    }

    #[test]
    fn response_translation() {
        let resp = json!({
            "id": "msg_1", "stop_reason": "tool_use",
            "content": [
                {"type": "thinking", "thinking": "hmm"},
                {"type": "text", "text": "Calling."},
                {"type": "tool_use", "id": "toolu_1", "name": "lookup", "input": {"q": "x"}}
            ],
            "usage": {"input_tokens": 10, "cache_read_input_tokens": 5, "output_tokens": 7}
        });
        let out = from_anthropic(
            &resp,
            &RequestInfo {
                model: "gpt-4o".into(),
                ..Default::default()
            },
        );
        assert_eq!(out["model"], "gpt-4o");
        let c = &out["choices"][0];
        assert_eq!(c["finish_reason"], "tool_calls");
        assert_eq!(c["message"]["content"], "Calling.");
        assert_eq!(c["message"]["reasoning_content"], "hmm");
        assert_eq!(
            c["message"]["tool_calls"][0]["function"]["arguments"],
            "{\"q\":\"x\"}"
        );
        assert_eq!(out["usage"]["prompt_tokens"], 15);
        assert_eq!(out["usage"]["total_tokens"], 22);
    }

    #[test]
    fn stream_translation() {
        let sse = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_9\",\"usage\":{\"input_tokens\":12}}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: ping\ndata: {\"type\":\"ping\"}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"f\",\"input\":{}}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"a\\\":1}\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":4}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        );
        let mut t = StreamTranslator::new(RequestInfo {
            model: "gpt-4o".into(),
            stream: true,
            include_usage: true,
        });
        let mut out = Vec::new();
        for c in sse.as_bytes().chunks(11) {
            out.extend(t.feed(c));
        }
        let text = String::from_utf8(out).unwrap();
        let events: Vec<&str> = text
            .split("\n\n")
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_start_matches("data: "))
            .collect();
        assert_eq!(*events.last().unwrap(), "[DONE]");
        let chunks: Vec<Value> = events[..events.len() - 1]
            .iter()
            .map(|e| serde_json::from_str(e).unwrap())
            .collect();
        assert_eq!(chunks[0]["choices"][0]["delta"]["role"], "assistant");
        assert_eq!(chunks[0]["id"], "chatcmpl-msg_9");
        let content: String = chunks
            .iter()
            .filter_map(|c| c["choices"][0]["delta"]["content"].as_str())
            .collect();
        assert_eq!(content, "Hello");
        let tool_start = chunks
            .iter()
            .find(|c| c["choices"][0]["delta"]["tool_calls"][0]["id"] == "toolu_1")
            .unwrap();
        assert_eq!(
            tool_start["choices"][0]["delta"]["tool_calls"][0]["index"],
            0
        );
        assert!(chunks
            .iter()
            .any(
                |c| c["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"]
                    == "{\"a\":1}"
            ));
        assert!(chunks
            .iter()
            .any(|c| c["choices"][0]["finish_reason"] == "tool_calls"));
        let usage = chunks.last().unwrap();
        assert_eq!(usage["choices"], json!([]));
        assert_eq!(usage["usage"]["completion_tokens"], 4);
        assert_eq!(usage["usage"]["prompt_tokens"], 12);
    }
}
