//! CLI coverage for the `build` command (resumable, pipelined CVE pack build).
//!
//! Drives the real `kgpacks build` surface end to end: writes a JSON corpus,
//! builds a pack from it, and asserts the reported counts, the on-disk pack, and
//! that the checkpoint sidecar is cleared on a clean finish. Also covers the
//! argument-validation error paths.

use std::fs;

use kgpacks_cli::run;
use kgpacks_db::{Database, DatabaseOptions, Value};
use tempfile::tempdir;

const CORPUS: &str = r#"[
  {
    "id": "CVE-2025-1000",
    "description": "Remote code execution in the Acme web server request parser.",
    "published_year": 2025,
    "entities": [
      {"entity_id": "product:acme-web", "name": "Acme Web", "type": "product", "description": "server"},
      {"entity_id": "vendor:acme", "name": "Acme", "type": "vendor", "description": "vendor"}
    ],
    "relations": [
      {"source_id": "product:acme-web", "target_id": "vendor:acme", "relation": "made_by", "context": "vendor"}
    ]
  },
  {
    "id": "CVE-2025-1001",
    "description": "SQL injection in the Acme reporting module.",
    "published_year": 2025,
    "entities": [
      {"entity_id": "product:acme-report", "name": "Acme Report", "type": "product", "description": "module"},
      {"entity_id": "vendor:acme", "name": "Acme", "type": "vendor", "description": "vendor"}
    ],
    "relations": []
  },
  {
    "id": "CVE-2025-1002",
    "description": "Path traversal in the Beta file service.",
    "published_year": 2025,
    "entities": [
      {"entity_id": "product:beta-fs", "name": "Beta FS", "type": "product", "description": "service"}
    ],
    "relations": []
  }
]"#;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn build_command_materializes_a_pack_and_clears_the_checkpoint() {
    let dir = tempdir().expect("tempdir");
    let corpus_path = dir.path().join("corpus.json");
    fs::write(&corpus_path, CORPUS).expect("write corpus");
    let out = dir.path().join("cve");

    let output = run(&args(&[
        "build",
        "cve",
        "--corpus",
        corpus_path.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--batch",
        "2",
        "--with-entity-relations",
    ]))
    .expect("build succeeds");

    let json: serde_json::Value = serde_json::from_str(&output).expect("json report");
    assert_eq!(json["command"], "build");
    assert_eq!(json["pack"], "cve");
    assert_eq!(json["interrupted"], false);
    assert_eq!(json["resumed"], false);
    assert_eq!(json["counts"]["articles"], 3);
    // vendor:acme is shared; distinct entities = acme-web, acme-report, beta-fs, acme = 4.
    assert_eq!(json["counts"]["entities"], 4);
    assert_eq!(json["counts"]["relationships"], 5);
    assert!(json["paramsHash"].as_str().unwrap().len() == 64);

    // The manifest + store exist and the checkpoint sidecar is gone.
    assert!(out.join("manifest.json").is_file());
    assert!(out.join("graph.lbug").is_file());
    assert!(!out.join("graph.lbug.build-checkpoint.json").exists());

    // The store really contains the CVE articles with their entity relations.
    let db = Database::open_with_options(
        out.join("graph.lbug"),
        DatabaseOptions {
            read_only: Some(true),
            ..DatabaseOptions::default()
        },
    )
    .expect("open");
    let conn = db.connect().expect("connect");
    let rel = &conn
        .run("MATCH (:Entity)-[r:ENTITY_RELATION]->(:Entity) RETURN count(r) AS n")
        .expect("query")[0]["n"];
    assert!(matches!(rel, Value::Int64(1) | Value::Int32(1)));
}

#[test]
fn build_uses_the_packs_dir_when_no_out_is_given() {
    let dir = tempdir().expect("tempdir");
    let corpus_path = dir.path().join("corpus.json");
    fs::write(&corpus_path, CORPUS).expect("write corpus");
    let packs_dir = dir.path().join("packs");

    run(&args(&[
        "--packs-dir",
        packs_dir.to_str().unwrap(),
        "build",
        "cve",
        "--corpus",
        corpus_path.to_str().unwrap(),
    ]))
    .expect("build succeeds");

    assert!(packs_dir.join("cve").join("graph.lbug").is_file());
}

#[test]
fn build_refuses_to_clobber_an_existing_pack_without_resume() {
    let dir = tempdir().expect("tempdir");
    let corpus_path = dir.path().join("corpus.json");
    fs::write(&corpus_path, CORPUS).expect("write corpus");
    let out = dir.path().join("cve");
    let build = || {
        run(&args(&[
            "build",
            "cve",
            "--corpus",
            corpus_path.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ]))
    };
    build().expect("first build");
    assert!(build().is_err(), "a second build must not clobber the pack");
}

#[test]
fn build_requires_the_corpus_flag() {
    let err = run(&args(&["build", "cve"])).unwrap_err();
    assert!(err.contains("--corpus"), "got: {err}");
}

#[test]
fn build_reports_a_missing_corpus_file() {
    let err = run(&args(&["build", "cve", "--corpus", "/no/such/corpus.json"])).unwrap_err();
    assert!(err.contains("corpus"), "got: {err}");
}

#[test]
fn build_rejects_a_non_positive_batch() {
    let dir = tempdir().expect("tempdir");
    let corpus_path = dir.path().join("corpus.json");
    fs::write(&corpus_path, CORPUS).expect("write corpus");
    let err = run(&args(&[
        "build",
        "cve",
        "--corpus",
        corpus_path.to_str().unwrap(),
        "--batch",
        "0",
    ]))
    .unwrap_err();
    assert!(err.contains("--batch"), "got: {err}");
}

#[test]
fn help_lists_the_build_command() {
    let out = run(&args(&["help"])).expect("help");
    assert!(out.contains("build"));
    assert!(out.contains("--resume"));
    assert!(out.contains("--corpus"));
}

#[test]
fn build_help_prints_usage() {
    let out = run(&args(&["build", "--help"])).expect("build --help");
    assert!(out.contains("kgpacks build"));
    assert!(out.contains("--corpus"));
    assert!(out.contains("--year"));
}

#[test]
fn build_rejects_an_unknown_flag() {
    let dir = tempdir().expect("tempdir");
    let corpus_path = dir.path().join("corpus.json");
    fs::write(&corpus_path, CORPUS).expect("write corpus");
    let err = run(&args(&[
        "build",
        "cve",
        "--corpus",
        corpus_path.to_str().unwrap(),
        "--yeer",
        "2025",
    ]))
    .unwrap_err();
    assert!(err.contains("unknown flag"), "got: {err}");
}

#[test]
fn build_rejects_a_flag_shaped_value() {
    // `--out --corpus x` must not silently make `--corpus` the output dir.
    let dir = tempdir().expect("tempdir");
    let corpus_path = dir.path().join("corpus.json");
    fs::write(&corpus_path, CORPUS).expect("write corpus");
    let err = run(&args(&[
        "build",
        "cve",
        "--out",
        "--corpus",
        corpus_path.to_str().unwrap(),
    ]))
    .unwrap_err();
    assert!(err.contains("missing value for --out"), "got: {err}");
}

#[test]
fn build_rejects_an_extra_positional() {
    let dir = tempdir().expect("tempdir");
    let corpus_path = dir.path().join("corpus.json");
    fs::write(&corpus_path, CORPUS).expect("write corpus");
    let err = run(&args(&[
        "build",
        "packA",
        "packB",
        "--corpus",
        corpus_path.to_str().unwrap(),
    ]))
    .unwrap_err();
    assert!(err.contains("unexpected argument"), "got: {err}");
}

#[test]
fn build_rejects_an_out_of_range_year() {
    let dir = tempdir().expect("tempdir");
    let corpus_path = dir.path().join("corpus.json");
    fs::write(&corpus_path, CORPUS).expect("write corpus");
    let err = run(&args(&[
        "build",
        "cve",
        "--corpus",
        corpus_path.to_str().unwrap(),
        "--year",
        "20255",
    ]))
    .unwrap_err();
    assert!(err.contains("--year"), "got: {err}");
}
