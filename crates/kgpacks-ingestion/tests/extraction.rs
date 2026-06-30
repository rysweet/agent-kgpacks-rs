//! Parity tests for LLM-response schema validation (SEC-08),
//! mirroring `bootstrap/src/extraction/tests/test_llm_schema_validation.py`,
//! plus relation normalization, domain detection, and JSON parsing.

use kgpacks_ingestion::extraction::{
    detect_domain, normalize_relation, parse_extraction_response, sanitize_entities,
    sanitize_key_facts, sanitize_relationships,
};
use serde_json::json;

// ── sanitize_entities ───────────────────────────────────────────────────────

#[test]
fn valid_entities_pass_through() {
    let raw = json!([
        {"name": "Azure", "type": "org", "properties": {}},
        {"name": "Lighthouse", "type": "concept"},
    ]);
    let result = sanitize_entities(&raw);
    assert_eq!(result.len(), 2);
    assert_eq!(result[0]["name"], "Azure");
    assert_eq!(result[1]["name"], "Lighthouse");
}

#[test]
fn entities_non_list_returns_empty() {
    assert!(sanitize_entities(&json!(null)).is_empty());
    assert!(sanitize_entities(&json!({})).is_empty());
    assert!(sanitize_entities(&json!("entities")).is_empty());
}

#[test]
fn entity_missing_or_empty_or_nonstring_name_is_dropped() {
    assert!(sanitize_entities(&json!([{"type": "concept", "properties": {}}])).is_empty());
    assert!(sanitize_entities(&json!([{"name": "   ", "type": "concept"}])).is_empty());
    assert!(sanitize_entities(&json!([{"name": 42, "type": "concept"}])).is_empty());
}

#[test]
fn entity_name_truncated_at_256() {
    let long = "A".repeat(300);
    let result = sanitize_entities(&json!([{"name": long, "type": "concept"}]));
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["name"].as_str().unwrap().chars().count(), 256);
}

#[test]
fn entity_missing_or_empty_type_defaults_to_concept() {
    let r1 = sanitize_entities(&json!([{"name": "Sentinel"}]));
    assert_eq!(r1[0]["type"], "concept");
    let r2 = sanitize_entities(&json!([{"name": "Sentinel", "type": ""}]));
    assert_eq!(r2[0]["type"], "concept");
}

#[test]
fn entity_non_dict_elements_skipped_and_properties_preserved() {
    let result = sanitize_entities(&json!([
        {"name": "Valid", "type": "org"}, "not-a-dict", null, 42,
    ]));
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["name"], "Valid");

    let with_props = sanitize_entities(
        &json!([{"name": "AKS", "type": "service", "properties": {"version": "1.28"}}]),
    );
    assert_eq!(with_props[0]["properties"], json!({"version": "1.28"}));
}

// ── sanitize_relationships ──────────────────────────────────────────────────

#[test]
fn valid_relationships_pass_through() {
    let result = sanitize_relationships(
        &json!([{"source": "A", "target": "B", "relation": "uses", "context": "A uses B"}]),
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["source"], "A");
}

#[test]
fn relationships_non_list_returns_empty() {
    assert!(sanitize_relationships(&json!(null)).is_empty());
    assert!(sanitize_relationships(&json!({})).is_empty());
    assert!(sanitize_relationships(&json!("rels")).is_empty());
}

#[test]
fn relationship_missing_empty_or_nonstring_fields_dropped() {
    assert!(sanitize_relationships(&json!([{"target": "B", "relation": "uses"}])).is_empty());
    assert!(
        sanitize_relationships(&json!([{"source": "", "target": "B", "relation": "uses"}]))
            .is_empty()
    );
    assert!(sanitize_relationships(&json!([{"source": "A", "relation": "uses"}])).is_empty());
    assert!(
        sanitize_relationships(&json!([{"source": "A", "target": "  ", "relation": "uses"}]))
            .is_empty()
    );
    assert!(sanitize_relationships(&json!([{"source": "A", "target": "B"}])).is_empty());
    assert!(
        sanitize_relationships(&json!([{"source": "A", "target": "B", "relation": ""}])).is_empty()
    );
    assert!(
        sanitize_relationships(&json!([{"source": 42, "target": "B", "relation": "uses"}]))
            .is_empty()
    );
}

#[test]
fn relationship_non_dict_skipped_and_context_optional() {
    let result = sanitize_relationships(&json!([
        {"source": "A", "target": "B", "relation": "uses"}, "not-a-dict", null,
    ]));
    assert_eq!(result.len(), 1);

    // A relationship without a context is still valid.
    let no_ctx =
        sanitize_relationships(&json!([{"source": "A", "target": "B", "relation": "part_of"}]));
    assert_eq!(no_ctx.len(), 1);
}

// ── sanitize_key_facts ──────────────────────────────────────────────────────

#[test]
fn valid_facts_pass_through() {
    let raw = json!([
        "Azure Lighthouse enables cross-tenant management.",
        "Delegated access is scoped.",
    ]);
    assert_eq!(
        sanitize_key_facts(&raw),
        vec![
            "Azure Lighthouse enables cross-tenant management.".to_string(),
            "Delegated access is scoped.".to_string(),
        ]
    );
}

#[test]
fn facts_non_list_returns_empty() {
    assert!(sanitize_key_facts(&json!(null)).is_empty());
    assert!(sanitize_key_facts(&json!({})).is_empty());
    assert!(sanitize_key_facts(&json!("a fact")).is_empty());
}

#[test]
fn facts_non_string_and_whitespace_dropped_and_stripped() {
    let result = sanitize_key_facts(&json!(["valid fact", 42, null, {"x": "y"}, "another fact"]));
    assert_eq!(result, vec!["valid fact", "another fact"]);

    let ws = sanitize_key_facts(&json!(["  ", "\t", "real fact"]));
    assert_eq!(ws, vec!["real fact"]);

    let trimmed = sanitize_key_facts(&json!(["  trimmed fact  "]));
    assert_eq!(trimmed, vec!["trimmed fact"]);
}

#[test]
fn long_fact_truncated_at_1024() {
    let long = "x".repeat(2000);
    let result = sanitize_key_facts(&json!([long]));
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].chars().count(), 1024);
}

// ── normalize_relation ──────────────────────────────────────────────────────

#[test]
fn normalize_relation_maps_synonyms_and_canonical_forms() {
    assert_eq!(normalize_relation("Established"), "founded");
    assert_eq!(normalize_relation("co-founded"), "founded");
    assert_eq!(normalize_relation("part of"), "part_of");
    assert_eq!(normalize_relation("uses"), "uses");
    // Unknown relations are kept as-is (normalized form).
    assert_eq!(normalize_relation("Sponsors"), "sponsors");
}

// ── detect_domain ───────────────────────────────────────────────────────────

#[test]
fn detect_domain_requires_two_keyword_hits() {
    assert_eq!(
        detect_domain(&["Military history".to_string(), "World War II".to_string()]),
        Some("history".to_string())
    );
    assert_eq!(
        detect_domain(&[
            "Computer science".to_string(),
            "Quantum algorithms".to_string()
        ]),
        Some("science".to_string())
    );
    // A single keyword hit is not enough.
    assert_eq!(detect_domain(&["War movies".to_string()]), None);
    assert_eq!(detect_domain(&[]), None);
}

#[test]
fn detect_domain_treats_underscore_as_a_word_character() {
    // Python's `\bkeyword\b` does not match across `_` (a word char), so an
    // all-underscore category yields no keyword hits.
    assert_eq!(
        detect_domain(&["World_War_II".to_string(), "Military_history".to_string()]),
        None
    );
}

// ── parse_extraction_response ───────────────────────────────────────────────

#[test]
fn parses_a_fenced_json_response_through_sanitization() {
    let response = r#"Here you go:
```json
{
  "entities": [
    {"name": "Rust", "type": "concept", "properties": {"description": "a language"}},
    {"name": "  ", "type": "x"}
  ],
  "relationships": [
    {"source": "Rust", "relation": "co-founded", "target": "Cargo", "context": "ctx"},
    {"source": "", "target": "B", "relation": "uses"}
  ],
  "key_facts": ["Rust is memory safe", "  "]
}
```
"#;
    let result = parse_extraction_response(response);
    assert_eq!(result.entities.len(), 1);
    assert_eq!(result.entities[0].name, "Rust");
    assert_eq!(result.relationships.len(), 1);
    // co-founded normalizes to the canonical "founded".
    assert_eq!(result.relationships[0].relation, "founded");
    assert_eq!(result.key_facts, vec!["Rust is memory safe".to_string()]);
}

#[test]
fn unparseable_response_yields_empty_result() {
    let result = parse_extraction_response("not json at all");
    assert!(result.entities.is_empty());
    assert!(result.relationships.is_empty());
    assert!(result.key_facts.is_empty());
}

#[test]
fn parses_prose_inside_a_fence_via_brace_narrowing() {
    // A code fence containing prose around the object still parses: after fence
    // stripping the first `{`..last `}` span is taken.
    let response = "```json\nHere is the data: {\"key_facts\": [\"fact one\"]}\n```";
    let result = parse_extraction_response(response);
    assert_eq!(result.key_facts, vec!["fact one".to_string()]);
}
