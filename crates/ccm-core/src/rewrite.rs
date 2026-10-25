//! Request-body rewrites applied before forwarding `/v1/messages`.
//! Key order is preserved (serde_json `preserve_order`), so untouched
//! fields go upstream as the client sent them.

use std::collections::BTreeMap;

use anyhow::{bail, Result};
use serde_json::{json, Map, Value};

use crate::config::EffortLevel;

/// The literal first system block Anthropic's OAuth Pro/Max endpoint
/// expects. Without it the upstream answers `rate_limit_error: "Error"`
/// regardless of actual quota.
pub const CLAUDE_CODE_IDENTIFIER: &str =
    "You are Claude Code, Anthropic's official CLI for Claude.";

fn parse_object(body: &[u8]) -> Option<Map<String, Value>> {
    match serde_json::from_slice(body) {
        Ok(Value::Object(m)) => Some(m),
        _ => None,
    }
}

/// Expands a top-level `effort` tier into `max_tokens` / `thinking`.
///
/// - No (or null) `effort` → `Ok(None)`, body unchanged.
/// - Unknown tier or non-string → `Err` listing the valid tiers.
/// - Existing `max_tokens` / `thinking` are never overwritten.
/// - `effort` itself is always stripped (Anthropic would 400 on it).
pub fn apply_effort(
    body: &[u8],
    levels: &BTreeMap<String, EffortLevel>,
) -> Result<Option<Vec<u8>>> {
    // Unparseable bodies are left for the caller's own parser to reject.
    let Some(mut obj) = parse_object(body) else {
        return Ok(None);
    };
    let tier = match obj.get("effort") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) => s.trim().to_ascii_lowercase(),
        Some(_) => bail!("effort must be a string (one of {})", known(levels)),
    };
    let Some(level) = levels.get(&tier) else {
        bail!(
            "unknown effort {:?}; valid values: {}",
            obj["effort"].as_str().unwrap_or_default(),
            known(levels)
        );
    };
    if !obj.contains_key("max_tokens") && level.max_tokens > 0 {
        obj.insert("max_tokens".into(), json!(level.max_tokens));
    }
    if !obj.contains_key("thinking") && level.thinking_budget > 0 {
        obj.insert(
            "thinking".into(),
            json!({"type": "enabled", "budget_tokens": level.thinking_budget}),
        );
    }
    obj.shift_remove("effort");
    Ok(Some(serde_json::to_vec(&obj)?))
}

fn known(levels: &BTreeMap<String, EffortLevel>) -> String {
    levels.keys().cloned().collect::<Vec<_>>().join(", ")
}

/// Makes `system` start with the Claude Code identifier block. Returns
/// `Ok(None)` when it already does. Errors mean the body is left as is.
pub fn inject_claude_code_system(body: &[u8]) -> Result<Option<Vec<u8>>> {
    let Some(mut obj) = parse_object(body) else {
        bail!("body is not a JSON object");
    };
    let ident = json!({"type": "text", "text": CLAUDE_CODE_IDENTIFIER});
    let system = match obj.get("system") {
        None | Some(Value::Null) => vec![ident],
        Some(Value::String(s)) => {
            if s.starts_with(CLAUDE_CODE_IDENTIFIER) {
                return Ok(None);
            }
            vec![ident, json!({"type": "text", "text": s})]
        }
        Some(Value::Array(blocks)) => {
            let first_is_ident = blocks.first().is_some_and(|b| {
                b["type"] == "text"
                    && b["text"]
                        .as_str()
                        .is_some_and(|t| t.starts_with(CLAUDE_CODE_IDENTIFIER))
            });
            if first_is_ident {
                return Ok(None);
            }
            std::iter::once(ident)
                .chain(blocks.iter().cloned())
                .collect()
        }
        Some(_) => bail!("system field is neither array nor string nor null"),
    };
    obj.insert("system".into(), Value::Array(system));
    Ok(Some(serde_json::to_vec(&obj)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_effort_levels;

    fn obj(b: &[u8]) -> Value {
        serde_json::from_slice(b).unwrap()
    }

    #[test]
    fn effort_noop_when_absent_or_null() {
        let levels = default_effort_levels();
        assert!(apply_effort(br#"{"model":"m","messages":[]}"#, &levels)
            .unwrap()
            .is_none());
        assert!(apply_effort(br#"{"model":"m","effort":null}"#, &levels)
            .unwrap()
            .is_none());
    }

    #[test]
    fn effort_expands_each_tier() {
        let levels = default_effort_levels();
        for (tier, lvl) in &levels {
            let body = format!(
                r#"{{"model":"m","effort":"{}","messages":[]}}"#,
                tier.to_uppercase()
            );
            let out = obj(&apply_effort(body.as_bytes(), &levels).unwrap().unwrap());
            assert!(out.get("effort").is_none());
            assert_eq!(out["max_tokens"], lvl.max_tokens);
            if lvl.thinking_budget > 0 {
                assert_eq!(
                    out["thinking"],
                    json!({"type":"enabled","budget_tokens":lvl.thinking_budget})
                );
            } else {
                assert!(out.get("thinking").is_none(), "{tier}");
            }
            // Untouched keys keep their position.
            assert_eq!(out.as_object().unwrap().keys().next().unwrap(), "model");
        }
    }

    #[test]
    fn effort_keeps_explicit_values() {
        let out = obj(&apply_effort(
            br#"{"model":"m","effort":"max","max_tokens":10,"thinking":{"type":"disabled"}}"#,
            &default_effort_levels(),
        )
        .unwrap()
        .unwrap());
        assert_eq!(out["max_tokens"], 10);
        assert_eq!(out["thinking"]["type"], "disabled");
    }

    #[test]
    fn effort_rejects_unknown_and_non_string() {
        let levels = default_effort_levels();
        let e = apply_effort(br#"{"effort":"turbo"}"#, &levels)
            .unwrap_err()
            .to_string();
        assert!(e.contains("turbo") && e.contains("medium"), "{e}");
        assert!(apply_effort(br#"{"effort":3}"#, &levels).is_err());
    }

    #[test]
    fn inject_cases() {
        let first_texts = |b: &[u8]| -> Vec<String> {
            obj(b)["system"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x["text"].as_str().unwrap().to_string())
                .collect()
        };
        let cases: &[(&str, Option<&[&str]>)] = &[
            (r#"{"model":"m","messages":[]}"#, Some(&[])),
            (r#"{"model":"m","system":null}"#, Some(&[])),
            (
                r#"{"model":"m","system":"You are a helpful pirate."}"#,
                Some(&["You are a helpful pirate."]),
            ),
            (
                r#"{"model":"m","system":[{"type":"text","text":"Be terse."}]}"#,
                Some(&["Be terse."]),
            ),
            (
                r#"{"model":"m","system":"You are Claude Code, Anthropic's official CLI for Claude.\n\nExtra."}"#,
                None,
            ),
            (
                r#"{"model":"m","system":[{"type":"text","text":"You are Claude Code, Anthropic's official CLI for Claude."},{"type":"text","text":"x"}]}"#,
                None,
            ),
        ];
        for (input, want) in cases {
            let out = inject_claude_code_system(input.as_bytes()).unwrap();
            match (want, out) {
                (None, None) => {}
                (Some(extra), Some(out)) => {
                    let texts = first_texts(&out);
                    assert_eq!(texts[0], CLAUDE_CODE_IDENTIFIER);
                    assert_eq!(&texts[1..], *extra, "{input}");
                    assert_eq!(obj(&out)["model"], "m");
                }
                (w, o) => panic!("{input}: want {w:?}, got {o:?}"),
            }
        }
        assert!(inject_claude_code_system(br#"{"system":5}"#).is_err());
    }
}
