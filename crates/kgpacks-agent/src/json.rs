//! `kgpacks-agent` — robust JSON extraction.
//!
//! Rust port of `@kgpacks/agent`'s `json.ts`. LLMs routinely wrap JSON in
//! Markdown code fences. [`strip_markdown_fences`] mirrors the reference's
//! `_strip_markdown_fences`, and [`safe_parse_json`] parses with `serde_json`
//! ONLY (never an evaluator), guards against forbidden (`__proto__` /
//! `constructor` / `prototype`) keys, and fails CLOSED with
//! [`AgentError::ResponseFormat`] carrying the (size-capped) raw input.

use serde_json::Value;

use crate::errors::AgentError;

/// Object keys that indicate a prototype-pollution-style payload (rejected to
/// preserve parity with the reference's guard).
const FORBIDDEN_KEYS: [&str; 3] = ["__proto__", "constructor", "prototype"];

/// Strip a surrounding Markdown code fence (```` ``` ````/```` ```json ````/…)
/// from model output and trim surrounding whitespace. Bare (unfenced) text is
/// returned trimmed and unchanged. Tolerates a missing closing fence.
pub fn strip_markdown_fences(text: &str) -> String {
    let s = text.trim();
    if s.is_empty() {
        return String::new();
    }
    if !s.starts_with("```") {
        return s.to_string();
    }

    // Drop the opening fence line: ``` + optional language tag, through the
    // first newline (mirrors `^```[^\n]*\n?`).
    let after_lang = &s[3..];
    let mut body: &str = match after_lang.find('\n') {
        Some(nl) => &after_lang[nl + 1..],
        None => "",
    };

    // Drop a trailing closing fence: optional trailing spaces/tabs, ```` ``` ````,
    // and one optional preceding newline (mirrors `\n?```[ \t]*$`).
    let trimmed = body.trim_end_matches([' ', '\t']);
    body = match trimmed.strip_suffix("```") {
        Some(without_fence) => without_fence.strip_suffix('\n').unwrap_or(without_fence),
        None => trimmed,
    };

    body.trim().to_string()
}

/// Recursively detect an injected `__proto__` / `constructor` / `prototype`
/// object key.
fn has_forbidden_keys(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(has_forbidden_keys),
        Value::Object(map) => {
            map.keys().any(|k| FORBIDDEN_KEYS.contains(&k.as_str()))
                || map.values().any(has_forbidden_keys)
        }
        _ => false,
    }
}

/// Parse JSON, failing closed. Returns [`AgentError::ResponseFormat`] (with the
/// offending content attached) on unparseable input or a forbidden-key payload.
/// Callers shape-check the returned [`Value`].
pub fn safe_parse_json(text: &str) -> Result<Value, AgentError> {
    let parsed: Value = serde_json::from_str(text).map_err(|err| {
        AgentError::response_format(format!("Model output is not valid JSON: {err}"), text)
    })?;
    if has_forbidden_keys(&parsed) {
        return Err(AgentError::response_format(
            "Model output contains forbidden keys (possible prototype pollution).",
            text,
        ));
    }
    Ok(parsed)
}
