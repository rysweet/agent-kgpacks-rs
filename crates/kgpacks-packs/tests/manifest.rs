//! Contract tests for the `kgpacks-packs` manifest model + validation.
//!
//! Ports `packages/packs/test/manifest.test.ts`: `pack_name_re`, `validate_manifest`
//! (accept + every documented rejection), the load/save round-trip (lossless,
//! trailing newline), and the dangerous-key guard.

use std::collections::BTreeMap;

use kgpacks_packs::{
    load_manifest, load_manifest_from_dir, manifest_path_in, pack_name_re, save_manifest,
    validate_manifest, PackManifest, MANIFEST_FILENAME,
};
use serde_json::{json, Value};

fn stats(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
}

fn valid_manifest() -> PackManifest {
    PackManifest {
        name: "world-history".into(),
        version: "1.2.0".into(),
        description: Some("World history knowledge pack".into()),
        graph_stats: Some(stats(&[
            ("articles", 12000.0),
            ("entities", 4800.0),
            ("relationships", 9100.0),
            ("size_mb", 18.4),
        ])),
        eval_scores: Some(stats(&[("recall_at_5", 0.81), ("faithfulness", 0.92)])),
        provenance: None,
        extra: BTreeMap::new(),
    }
}

#[test]
fn manifest_filename_is_canonical() {
    assert_eq!(MANIFEST_FILENAME, "manifest.json");
}

#[test]
fn pack_name_re_accepts_valid_names() {
    let long = "x".repeat(64);
    for name in ["world-history", "a", "A1_b-2", long.as_str(), "0pack"] {
        assert!(pack_name_re().is_match(name), "expected {name:?} accepted");
    }
}

#[test]
fn pack_name_re_rejects_invalid_names() {
    let too_long = "x".repeat(65);
    for name in [
        "",
        "../etc",
        "-leading",
        "_leading",
        "has space",
        "dot.name",
        too_long.as_str(),
        "slash/name",
    ] {
        assert!(!pack_name_re().is_match(name), "expected {name:?} rejected");
    }
}

#[test]
fn validate_manifest_returns_typed_manifest_for_valid_input() {
    let m = valid_manifest();
    assert_eq!(validate_manifest(&m.to_value()).expect("valid"), m);
}

#[test]
fn validate_manifest_accepts_minimal_name_and_version() {
    let value = json!({ "name": "mini", "version": "0.1.0" });
    let m = validate_manifest(&value).expect("valid");
    assert_eq!(m.name, "mini");
    assert_eq!(m.version, "0.1.0");
    assert!(m.description.is_none());
    assert!(m.graph_stats.is_none());
    assert!(m.eval_scores.is_none());
}

#[test]
fn validate_manifest_rejects_bad_identity() {
    let cases = [
        json!({ "version": "1.0.0" }),
        json!({ "name": 123, "version": "1.0.0" }),
        json!({ "name": "../evil", "version": "1.0.0" }),
        json!({ "name": "ok" }),
        json!({ "name": "ok", "version": 100 }),
        json!({ "name": "ok", "version": "1.0" }),
    ];
    for value in cases {
        assert!(
            validate_manifest(&value).is_err(),
            "expected rejection for {value}"
        );
    }
}

#[test]
fn validate_manifest_accepts_real_catalog_graph_stats_shape() {
    let value = json!({
        "name": "rust-expert",
        "version": "1.0.0",
        "graph_stats": { "articles": 294, "entities": 1388, "relationships": 1190, "size_mb": 2.08 },
    });
    let m = validate_manifest(&value).expect("valid");
    assert_eq!(m.graph_stats.unwrap()["size_mb"], 2.08);
}

#[test]
fn validate_manifest_rejects_malformed_graph_stats() {
    let cases = [
        json!({ "articles": -1 }),
        json!({ "articles": 5, "entities": "lots" }),
    ];
    for graph_stats in cases {
        let value = json!({ "name": "ok", "version": "1.0.0", "graph_stats": graph_stats });
        assert!(
            validate_manifest(&value).is_err(),
            "expected rejection for {value}"
        );
    }
}

#[test]
fn validate_manifest_rejects_malformed_eval_scores() {
    let value = json!({ "name": "ok", "version": "1.0.0", "eval_scores": { "recall": "high" } });
    assert!(validate_manifest(&value).is_err());
}

#[test]
fn validate_manifest_accepts_a_valid_provenance_block() {
    // Mirrors the reference `PackProvenance` shape: corpus/embedding/build with
    // declared string fields and a numeric `embedding.dimensions`.
    let value = json!({
        "name": "cve",
        "version": "1.0.0",
        "provenance": {
            "corpus": { "name": "cvelistV5", "commit": "abc123", "date": "2026-01-01" },
            "embedding": { "model": "bge-base-en-v1.5", "dimensions": 768 },
            "build": { "date": "2026-01-02T00:00:00Z", "tool_version": "0.1.0" }
        }
    });
    let m = validate_manifest(&value).expect("valid provenance accepted");
    let prov = m.provenance.clone().expect("provenance preserved");
    assert_eq!(prov["embedding"]["dimensions"], json!(768));
    // Survives a to_value/validate round trip unchanged.
    assert_eq!(validate_manifest(&m.to_value()).expect("round-trip"), m);
}

#[test]
fn validate_manifest_treats_present_null_provenance_as_absent_but_preserves_it() {
    // Real catalog manifests may carry `provenance: null`; the reference re-emits
    // it, so an explicit null is preserved (like `eval_scores: null`).
    let value = json!({ "name": "ok", "version": "1.0.0", "provenance": null });
    let m = validate_manifest(&value).expect("null provenance accepted");
    assert!(m.provenance.is_none());
    assert_eq!(m.extra.get("provenance"), Some(&Value::Null));
    assert_eq!(validate_manifest(&m.to_value()).expect("round-trip"), m);
}

#[test]
fn validate_manifest_allows_null_provenance_fields_and_preserves_unknown_ones() {
    // Undeterminable declared fields recorded as null are allowed, and unknown
    // sections/fields are tolerated + preserved (not validated), per the reference.
    let value = json!({
        "name": "ok",
        "version": "1.0.0",
        "provenance": {
            "corpus": { "name": "cvelistV5", "commit": null, "date": null },
            "embedding": { "model": "hash", "notes": "extra-key-tolerated" },
            "extra_section": { "anything": 1 }
        }
    });
    let m = validate_manifest(&value).expect("null/unknown provenance fields tolerated");
    let prov = m.provenance.expect("provenance preserved");
    assert_eq!(prov["embedding"]["notes"], json!("extra-key-tolerated"));
    assert_eq!(prov["extra_section"]["anything"], json!(1));
}

#[test]
fn validate_manifest_accepts_zero_absent_and_float_embedding_dimensions() {
    for dims in [json!(0), json!(768.0), json!(null)] {
        let value = json!({
            "name": "ok",
            "version": "1.0.0",
            "provenance": { "embedding": { "model": "m", "dimensions": dims } }
        });
        assert!(
            validate_manifest(&value).is_ok(),
            "expected acceptance for {value}"
        );
    }
}

#[test]
fn validate_manifest_rejects_malformed_provenance() {
    // Each case mirrors a documented reference `validateProvenance` rejection.
    let cases = [
        json!("not-an-object"),                       // provenance must be an object
        json!(["array"]),                             // provenance must be an object
        json!({ "corpus": "not-an-object" }),         // section must be an object
        json!({ "corpus": { "name": 123 } }),         // declared field must be a string
        json!({ "build": { "tool_version": true } }), // declared field must be a string
        json!({ "embedding": { "dimensions": -1 } }), // dimensions non-negative
        json!({ "embedding": { "dimensions": "768" } }), // dimensions must be a number
    ];
    for provenance in cases {
        let value = json!({ "name": "ok", "version": "1.0.0", "provenance": provenance });
        assert!(
            validate_manifest(&value).is_err(),
            "expected rejection for {value}"
        );
    }
}

#[test]
fn validate_manifest_deep_sanitizes_dangerous_keys_inside_provenance() {
    // The reference deep-sanitizes the provenance block; dangerous keys are
    // stripped at every nesting level while other keys are preserved.
    let value: Value = serde_json::from_str(
        r#"{"name":"ok","version":"1.0.0","provenance":{"corpus":{"name":"x","__proto__":{"polluted":true}},"__proto__":"evil"}}"#,
    )
    .expect("json");
    let m = validate_manifest(&value).expect("valid after sanitize");
    let prov = m.provenance.expect("provenance preserved");
    let obj = prov.as_object().expect("provenance object");
    assert!(
        !obj.contains_key("__proto__"),
        "top-level dangerous key should be stripped"
    );
    let corpus = obj["corpus"].as_object().expect("corpus object");
    assert!(
        !corpus.contains_key("__proto__"),
        "nested dangerous key should be stripped"
    );
    assert_eq!(corpus["name"], json!("x"));
}

#[test]
fn save_then_load_preserves_provenance_serialization() {
    let dir = tempfile::tempdir().expect("tempdir");
    let value = json!({
        "name": "cve",
        "version": "2025.6.0",
        "provenance": {
            "corpus": { "name": "cvelistV5", "commit": "abc123", "date": null },
            "embedding": { "model": "Xenova/bge-base-en-v1.5", "dimensions": 768 },
            "build": { "date": "2026-01-02T00:00:00Z", "tool_version": "agent-kgpacks-rs@0.1.0" }
        }
    });
    let manifest = validate_manifest(&value).expect("valid provenance");

    let path = manifest_path_in(dir.path());
    save_manifest(&path, &manifest).expect("save");
    let raw = std::fs::read_to_string(&path).expect("read");
    let saved: Value = serde_json::from_str(&raw).expect("parse saved manifest");
    assert_eq!(saved["provenance"], value["provenance"]);
    assert_eq!(load_manifest(&path).expect("load"), manifest);
}

#[test]
fn validate_manifest_strips_dangerous_keys() {
    let value =
        serde_json::from_str(r#"{"name":"safe","version":"1.0.0","__proto__":{"polluted":true}}"#)
            .expect("json");
    let m = validate_manifest(&value).expect("valid");
    assert!(!m.extra.contains_key("__proto__"));
    assert!(!m.extra.contains_key("polluted"));
}

#[test]
fn validate_manifest_treats_present_null_optional_sections_as_absent() {
    let value =
        json!({ "name": "ok", "version": "1.0.0", "graph_stats": null, "eval_scores": null });
    let m = validate_manifest(&value).expect("valid");
    assert!(m.graph_stats.is_none());
    assert!(m.eval_scores.is_none());
}

#[test]
fn explicit_null_optional_sections_survive_a_save_load_round_trip() {
    // Real catalog manifests carry `eval_scores: null`; the reference re-emits it.
    let value = json!({ "name": "ok", "version": "1.0.0", "eval_scores": null });
    let m = validate_manifest(&value).expect("valid");
    assert_eq!(m.extra.get("eval_scores"), Some(&Value::Null));

    let dir = tempfile::tempdir().expect("tempdir");
    let path = manifest_path_in(dir.path());
    save_manifest(&path, &m).expect("save");
    let content = std::fs::read_to_string(&path).expect("read");
    assert!(
        content.contains("\"eval_scores\": null"),
        "the explicit null key should be preserved on disk"
    );
    assert_eq!(load_manifest(&path).expect("load"), m);
}

#[test]
fn load_manifest_reads_parses_and_validates() {
    let dir = tempfile::tempdir().expect("tempdir");
    let m = valid_manifest();
    let path = dir.path().join("manifest.json");
    std::fs::write(&path, m.to_value().to_string()).expect("write");
    assert_eq!(load_manifest(&path).expect("load"), m);
}

#[test]
fn load_manifest_from_dir_resolves_filename_under_pack_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    let m = valid_manifest();
    save_manifest(manifest_path_in(dir.path()), &m).expect("save");
    assert_eq!(load_manifest_from_dir(dir.path()).expect("load"), m);
}

#[test]
fn save_then_load_preserves_unknown_keys_losslessly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut m = valid_manifest();
    m.extra
        .insert("tags".into(), json!(["history", "reference"]));
    m.extra.insert(
        "custom_meta".into(),
        json!({ "source": "wikipedia", "revision": 42 }),
    );

    let path = manifest_path_in(dir.path());
    save_manifest(&path, &m).expect("save");
    assert_eq!(load_manifest(&path).expect("load"), m);
}

#[test]
fn save_manifest_writes_pretty_json_with_single_trailing_newline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = manifest_path_in(dir.path());
    save_manifest(&path, &valid_manifest()).expect("save");
    let content = std::fs::read_to_string(&path).expect("read");
    assert!(content.ends_with('\n'));
    assert!(!content.ends_with("\n\n"));
    assert!(content.contains("\n  \"name\":"));
}

#[test]
fn save_manifest_validates_before_writing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = manifest_path_in(dir.path());
    let invalid = PackManifest {
        name: "../bad".into(),
        version: "1.0.0".into(),
        description: None,
        graph_stats: None,
        eval_scores: None,
        provenance: None,
        extra: BTreeMap::new(),
    };
    assert!(save_manifest(&path, &invalid).is_err());
    assert!(!path.exists(), "no file should be left behind");
}

#[test]
fn load_manifest_errors_for_a_missing_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(load_manifest(dir.path().join("nope.json")).is_err());
}

#[test]
fn load_manifest_errors_for_invalid_json() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = manifest_path_in(dir.path());
    std::fs::write(&path, "{ not valid json ").expect("write");
    assert!(load_manifest(&path).is_err());
}

#[test]
fn load_manifest_errors_for_schema_invalid_json() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = manifest_path_in(dir.path());
    std::fs::write(&path, r#"{"name":"ok","version":"not-semver"}"#).expect("write");
    assert!(load_manifest(&path).is_err());
}
