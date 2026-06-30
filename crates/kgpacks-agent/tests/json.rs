//! Offline parity tests for the JSON-extraction helpers (`src/json.rs`),
//! mirroring the reference `packages/agent/test/json.test.ts`:
//! `strip_markdown_fences` (mirrors the Python `_strip_markdown_fences`) and
//! `safe_parse_json` (`serde_json`-only, forbidden-key guarded, fails CLOSED
//! with `AgentError::ResponseFormat`).

use kgpacks_agent::{safe_parse_json, strip_markdown_fences, AgentError};

// ── strip_markdown_fences ───────────────────────────────────────────────────

#[test]
fn returns_a_bare_json_string_unchanged() {
    assert_eq!(strip_markdown_fences(r#"["a","b"]"#), r#"["a","b"]"#);
}

#[test]
fn strips_a_json_fenced_block_to_its_inner_content() {
    assert_eq!(
        strip_markdown_fences("```json\n[\"a\",\"b\"]\n```"),
        r#"["a","b"]"#
    );
}

#[test]
fn strips_a_bare_fenced_block_to_its_inner_content() {
    assert_eq!(strip_markdown_fences("```\n[\"a\"]\n```"), r#"["a"]"#);
}

#[test]
fn strips_a_fence_regardless_of_language_tag_casing() {
    assert_eq!(strip_markdown_fences("```JSON\n[\"a\"]\n```"), r#"["a"]"#);
}

#[test]
fn trims_whitespace_around_a_fenced_block() {
    assert_eq!(
        strip_markdown_fences("\n\n```json\n[\"a\"]\n```\n\n"),
        r#"["a"]"#
    );
}

#[test]
fn trims_surrounding_whitespace_on_unfenced_text() {
    assert_eq!(strip_markdown_fences(r#"   ["a"]   "#), r#"["a"]"#);
}

#[test]
fn strips_an_opening_fence_even_when_the_closing_fence_is_missing() {
    assert_eq!(strip_markdown_fences("```json\n[\"a\"]"), r#"["a"]"#);
}

#[test]
fn returns_empty_for_empty_or_whitespace_only_input() {
    assert_eq!(strip_markdown_fences(""), "");
    assert_eq!(strip_markdown_fences("   \n  "), "");
}

#[test]
fn round_trips_stripped_fenced_json_into_the_original_array() {
    let stripped = strip_markdown_fences("```json\n[\"alpha\",\"beta\",\"gamma\"]\n```");
    let parsed = safe_parse_json(&stripped).unwrap();
    let arr = parsed.as_array().unwrap();
    let values: Vec<&str> = arr.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(values, ["alpha", "beta", "gamma"]);
}

// ── safe_parse_json ─────────────────────────────────────────────────────────

#[test]
fn parses_a_valid_json_array() {
    let parsed = safe_parse_json(r#"["a","b"]"#).unwrap();
    assert_eq!(parsed, serde_json::json!(["a", "b"]));
}

#[test]
fn parses_a_valid_json_object() {
    let parsed = safe_parse_json(r#"{"answer":"hi","n":2}"#).unwrap();
    assert_eq!(parsed, serde_json::json!({"answer": "hi", "n": 2}));
}

#[test]
fn composes_with_strip_markdown_fences_for_fenced_output() {
    let parsed = safe_parse_json(&strip_markdown_fences("```json\n[\"x\",\"y\"]\n```")).unwrap();
    assert_eq!(parsed, serde_json::json!(["x", "y"]));
}

#[test]
fn errors_on_unparseable_input() {
    let err = safe_parse_json("not json at all").unwrap_err();
    assert!(err.is_response_format());
}

#[test]
fn attaches_the_offending_raw_content_to_the_error() {
    let err = safe_parse_json("definitely-not-json").unwrap_err();
    assert!(err.is_response_format());
    assert!(err.raw_content().unwrap().contains("definitely-not-json"));
}

#[test]
fn rejects_a_top_level_proto_key() {
    let err = safe_parse_json(r#"{"__proto__":{"polluted":true}}"#).unwrap_err();
    assert!(err.is_response_format());
}

#[test]
fn rejects_a_top_level_constructor_key() {
    let err = safe_parse_json(r#"{"constructor":{"x":1}}"#).unwrap_err();
    assert!(err.is_response_format());
}

#[test]
fn does_not_evaluate_a_function_expression_payload() {
    // `serde_json` parses data only — a JS function-expression is just invalid
    // JSON and is rejected, never executed.
    let err = safe_parse_json("(() => 1)()").unwrap_err();
    assert!(err.is_response_format());
}

#[test]
fn forbidden_key_error_is_an_agent_error() {
    let err = safe_parse_json(r#"{"prototype":1}"#).unwrap_err();
    // Catchable as the single AgentError type (the reference's AgentError base).
    let _is_agent_error: &AgentError = &err;
    assert!(err.is_response_format());
}
