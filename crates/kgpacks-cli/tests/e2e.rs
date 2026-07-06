//! End-to-end parity test for the CLI graph-RAG flow:
//! `build pack → (vector + FTS) retrieval → graph-RAG query`, driven through the
//! actual `kgpacks` command surface.
//!
//! It materializes a LadybugDB pack (a `Section` table with embeddings + a cosine
//! vector index and `LINKS_TO` edges) the way a built/ingested pack is shaped,
//! then exercises the `query` command (ranked retrieval JSON) and the `ask`
//! command (grounded graph-RAG synthesis) — the latter against an OFFLINE mock
//! transport, so the full flow runs with no network or model.

use std::fs;
use std::path::Path;
use std::process::Command;

use kgpacks_agent::{
    Transport, TransportError, TransportOpenConfig, TransportResponse, TransportSession, Usage,
};
use kgpacks_cli::{run, run_with_transport};
use kgpacks_db::{Database, LogicalType, Value};
use kgpacks_embeddings::Embedder;
use tempfile::tempdir;

/// A mock transport that cites the FIRST context chunk id in the prompt.
struct CiteFirstChunk;
struct CiteFirstChunkSession;

impl Transport for CiteFirstChunk {
    fn open(
        &self,
        _config: &TransportOpenConfig,
    ) -> Result<Box<dyn TransportSession>, TransportError> {
        Ok(Box::new(CiteFirstChunkSession))
    }
    fn shutdown(&self) -> Result<(), TransportError> {
        Ok(())
    }
}

impl TransportSession for CiteFirstChunkSession {
    fn send(
        &self,
        prompt: &str,
        _timeout_ms: Option<u64>,
    ) -> Result<TransportResponse, TransportError> {
        let id = prompt
            .find("id=\"")
            .map(|i| &prompt[i + 4..])
            .and_then(|rest| rest.find('"').map(|e| rest[..e].to_string()))
            .unwrap_or_else(|| "none".to_string());
        Ok(TransportResponse {
            content: format!("Grounded answer per {id}."),
            usage: Usage::new(9, 13, 0),
        })
    }
    fn close(&self) -> Result<(), TransportError> {
        Ok(())
    }
}

fn float_array(v: &[f32]) -> Value {
    Value::Array(
        LogicalType::Float,
        v.iter().map(|&x| Value::Float(x)).collect(),
    )
}

/// Build a retrievable pack database at `db_path` (a `Section` table with bge
/// embeddings + a cosine vector index and one `LINKS_TO` edge).
fn build_pack_db(db_path: &Path) {
    let db = Database::open(db_path).expect("open pack db");
    let conn = db.connect().expect("connect");
    conn.load_extension("vector").expect("load vector ext");
    conn.run(
        "CREATE NODE TABLE Section(id STRING, title STRING, content STRING, \
         embedding FLOAT[768], PRIMARY KEY(id))",
    )
    .expect("create Section");
    conn.run("CREATE REL TABLE LINKS_TO(FROM Section TO Section, link_type STRING)")
        .expect("create LINKS_TO");

    let embedder = Embedder::bge();
    for (id, title, content) in [
        (
            "s1",
            "Rust",
            "Rust is a systems language with ownership and borrowing.",
        ),
        (
            "s2",
            "Cargo",
            "Cargo builds, tests, and publishes Rust crates.",
        ),
    ] {
        let embedding = embedder.embed(content);
        conn.run_params(
            "CREATE (:Section {id: $id, title: $title, content: $content, embedding: $emb})",
            vec![
                ("id", Value::String(id.to_string())),
                ("title", Value::String(title.to_string())),
                ("content", Value::String(content.to_string())),
                ("emb", float_array(&embedding)),
            ],
        )
        .expect("insert Section");
    }
    conn.run_params(
        "MATCH (a:Section {id: $a}), (b:Section {id: $b}) \
         CREATE (a)-[:LINKS_TO {link_type: 'related'}]->(b)",
        vec![
            ("a", Value::String("s1".to_string())),
            ("b", Value::String("s2".to_string())),
        ],
    )
    .expect("create LINKS_TO edge");
    conn.run(
        "CALL CREATE_VECTOR_INDEX('Section', 'embedding_idx', 'embedding', metric := 'cosine')",
    )
    .expect("create vector index");
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// Extract the `packsDir` value the CLI reports from a `status` JSON blob.
fn packs_dir_field(status_json: &str) -> &str {
    let key = "\"packsDir\":";
    let after = status_json
        .split_once(key)
        .unwrap_or_else(|| panic!("no packsDir in status output: {status_json}"))
        .1;
    let start = after.find('"').expect("packsDir opening quote") + 1;
    let rest = &after[start..];
    let end = rest.find('"').expect("packsDir closing quote");
    &rest[..end]
}

/// End-to-end proof of the WS4 packs-dir precedence, driven through the REAL
/// `kgpacks` binary: `--packs-dir` flag beats `KGPACKS_PACKS_DIR`, which beats
/// the XDG default (`$XDG_DATA_HOME/kgpacks`, else `$HOME/.local/share/kgpacks`).
/// The `status` command reports the resolved `packsDir`, so each case asserts
/// the winning directory (and, for the precedence cases, that the losing
/// directory is not chosen).
///
/// Each invocation sets its environment on the CHILD [`Command`] and never
/// mutates this test process's own environment. Mutating the shared process env
/// (`set_var`/`remove_var`) would data-race the native `getenv` LadybugDB
/// performs when the sibling e2e tests load their `~/.lbdb` extensions.
#[test]
fn cli_resolves_packs_dir_by_precedence() {
    let bin = env!("CARGO_BIN_EXE_kgpacks");
    let root = tempdir().expect("tempdir");
    let flag_dir = root.path().join("flag");
    let env_dir = root.path().join("env");
    let xdg_home = root.path().join("xdg");
    let home_dir = root.path().join("home");

    // Run `kgpacks <args> status` with `env_overrides` applied to the CHILD only
    // (`None` removes the variable), returning the reported `packsDir`.
    let packs_dir = |env_overrides: &[(&str, Option<&Path>)], args: &[&str]| -> String {
        let mut cmd = Command::new(bin);
        cmd.args(args).arg("status");
        for (key, value) in env_overrides {
            match value {
                Some(path) => {
                    cmd.env(key, path);
                }
                None => {
                    cmd.env_remove(key);
                }
            }
        }
        let output = cmd.output().expect("run kgpacks status");
        assert!(
            output.status.success(),
            "kgpacks status failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
        packs_dir_field(&stdout).to_string()
    };

    let both_overrides = [
        ("KGPACKS_PACKS_DIR", Some(env_dir.as_path())),
        ("XDG_DATA_HOME", Some(xdg_home.as_path())),
    ];

    // (1) The `--packs-dir` flag wins over BOTH the env override and the XDG
    // default (which are both set to different, losing directories).
    let resolved = packs_dir(
        &both_overrides,
        &["--packs-dir", flag_dir.to_str().unwrap()],
    );
    assert_eq!(Path::new(&resolved), flag_dir, "flag beats env + xdg");

    // (2) `KGPACKS_PACKS_DIR` wins over the XDG default (no flag).
    let resolved = packs_dir(&both_overrides, &[]);
    assert_eq!(Path::new(&resolved), env_dir, "env beats xdg default");

    // (3) With no flag and no env override, the default is `$XDG_DATA_HOME/kgpacks`.
    let resolved = packs_dir(
        &[
            ("KGPACKS_PACKS_DIR", None),
            ("XDG_DATA_HOME", Some(xdg_home.as_path())),
        ],
        &[],
    );
    assert_eq!(
        Path::new(&resolved),
        xdg_home.join("kgpacks"),
        "xdg default"
    );

    // (4) With XDG unset too, the default falls back to `$HOME/.local/share/kgpacks`.
    let resolved = packs_dir(
        &[
            ("KGPACKS_PACKS_DIR", None),
            ("XDG_DATA_HOME", None),
            ("HOME", Some(home_dir.as_path())),
        ],
        &[],
    );
    assert_eq!(
        Path::new(&resolved),
        home_dir.join(".local").join("share").join("kgpacks"),
        "home fallback default"
    );
}

#[test]
fn cli_query_then_ask_runs_the_graph_rag_flow_end_to_end() {
    let tmp = tempdir().expect("tempdir");
    let packs_dir = tmp.path();
    let pack_dir = packs_dir.join("rustpack");
    fs::create_dir_all(&pack_dir).expect("mkdir pack");
    let db_path = pack_dir.join("graph.lbug");
    build_pack_db(&db_path);

    let packs = packs_dir.to_str().unwrap();
    let factory = || -> Box<dyn Transport> { Box::new(CiteFirstChunk) };

    // `query`: ranked retrieval JSON over the built pack.
    let query_out = run_with_transport(
        &argv(&[
            "--packs-dir",
            packs,
            "query",
            "rustpack",
            "what is rust?",
            "-k",
            "5",
        ]),
        &factory,
    )
    .expect("query command");
    assert!(query_out.contains("\"results\""), "query JSON: {query_out}");
    assert!(
        query_out.contains("\"s1\""),
        "query JSON missing s1: {query_out}"
    );
    assert!(
        query_out.contains("\"s2\""),
        "query JSON missing s2: {query_out}"
    );

    // `ask`: full graph-RAG — retrieve, then synthesize a grounded answer.
    let ask_out = run_with_transport(
        &argv(&[
            "--packs-dir",
            packs,
            "ask",
            "rustpack",
            "what is rust?",
            "-k",
            "5",
        ]),
        &factory,
    )
    .expect("ask command");
    assert!(
        ask_out.contains("Grounded answer per"),
        "ask JSON: {ask_out}"
    );
    assert!(ask_out.contains("\"citedIds\""), "ask JSON: {ask_out}");
    assert!(ask_out.contains("\"answer\""), "ask JSON: {ask_out}");
    // The cited id must be one of the retrieved sections.
    assert!(
        ask_out.contains("\"s1\"") || ask_out.contains("\"s2\""),
        "ask JSON missing a cited section: {ask_out}"
    );
}

#[test]
fn cli_status_lists_installed_packs_through_the_command_surface() {
    // `status` needs no transport and never opens the store — it only checks the
    // graph store's presence. Drive it through the production `run` entry to
    // prove the dispatch + registry read-path end to end over a real packs
    // directory (one pack with a graph store present, one without, plus a
    // manifest-less directory that must be skipped). The "present" store is a
    // plain marker file: `status` only stats it, so there is no need to build a
    // real LadybugDB (which would load the shared `vector` extension and race
    // the other e2e test).
    let tmp = tempdir().expect("tempdir");
    let packs_dir = tmp.path();

    let with_db = packs_dir.join("rustpack");
    fs::create_dir_all(&with_db).expect("mkdir rustpack");
    fs::write(
        with_db.join("manifest.json"),
        r#"{"name":"rustpack","version":"1.4.0"}"#,
    )
    .expect("write manifest");
    fs::write(with_db.join("graph.lbug"), b"").expect("write graph store marker");

    let no_db = packs_dir.join("emptypack");
    fs::create_dir_all(&no_db).expect("mkdir emptypack");
    fs::write(
        no_db.join("manifest.json"),
        r#"{"name":"emptypack","version":"0.1.0"}"#,
    )
    .expect("write manifest");

    fs::create_dir_all(packs_dir.join("junk")).expect("mkdir junk");

    let out = run(&argv(&[
        "--packs-dir",
        packs_dir.to_str().unwrap(),
        "status",
    ]))
    .expect("status command");

    let value: serde_json::Value = serde_json::from_str(&out).expect("status JSON");
    assert_eq!(value["packsDir"], packs_dir.display().to_string());
    assert_eq!(value["count"], 2, "status JSON: {out}");
    let packs = value["packs"].as_array().expect("packs array");
    // Sorted by name: emptypack before rustpack.
    assert_eq!(packs[0]["name"], "emptypack");
    assert_eq!(packs[0]["dbPresent"], false);
    assert_eq!(packs[1]["name"], "rustpack");
    assert_eq!(packs[1]["version"], "1.4.0");
    assert_eq!(packs[1]["dbPresent"], true);
}

/// Helper: write a manifest into `<packs_dir>/<name>/manifest.json`.
fn write_pack_manifest(packs_dir: &Path, name: &str, manifest_json: &str) {
    let dir = packs_dir.join(name);
    fs::create_dir_all(&dir).expect("mkdir pack");
    fs::write(dir.join("manifest.json"), manifest_json).expect("write manifest");
}

#[test]
fn cli_pack_list_projects_and_sorts_installed_packs() {
    // `pack list` reads the registry only — no store, no transport. Drive it
    // through the production `run` entry over a real packs directory with a mix
    // of present/absent descriptions, sort-discriminating names, and skipped
    // entries (an invalid manifest and a manifest-less directory).
    let tmp = tempdir().expect("tempdir");
    let packs_dir = tmp.path();

    write_pack_manifest(
        packs_dir,
        "bravo",
        r#"{"name":"bravo","version":"0.5.0","description":"Bravo pack"}"#,
    );
    write_pack_manifest(packs_dir, "alpha", r#"{"name":"alpha","version":"1.0.0"}"#);
    // Invalid manifest (missing version) and a manifest-less directory: skipped.
    write_pack_manifest(packs_dir, "broken", r#"{"name":"broken"}"#);
    fs::create_dir_all(packs_dir.join("no-manifest")).expect("mkdir no-manifest");

    let out = run(&argv(&[
        "--packs-dir",
        packs_dir.to_str().unwrap(),
        "pack",
        "list",
    ]))
    .expect("pack list command");

    let value: serde_json::Value = serde_json::from_str(&out).expect("pack list JSON");
    let packs = value.as_array().expect("pack list is an array");
    assert_eq!(packs.len(), 2, "pack list JSON: {out}");
    // Sorted by name: alpha before bravo.
    assert_eq!(packs[0]["name"], "alpha");
    assert_eq!(packs[0]["version"], "1.0.0");
    // Absent description defaults to "" (never null / missing).
    assert_eq!(packs[0]["description"], "");
    assert_eq!(packs[1]["name"], "bravo");
    assert_eq!(packs[1]["description"], "Bravo pack");
}

#[test]
fn cli_pack_info_prints_the_full_manifest() {
    // `pack info <name>` prints the pack's full manifest, preserving optional
    // sections and unknown keys verbatim (the on-disk snake_case shape).
    let tmp = tempdir().expect("tempdir");
    let packs_dir = tmp.path();
    write_pack_manifest(
        packs_dir,
        "rich",
        r#"{"name":"rich","version":"1.2.3","description":"Rich pack","graph_stats":{"articles":294,"size_mb":12.5},"channel":"stable"}"#,
    );

    let out = run(&argv(&[
        "--packs-dir",
        packs_dir.to_str().unwrap(),
        "pack",
        "info",
        "rich",
    ]))
    .expect("pack info command");

    let value: serde_json::Value = serde_json::from_str(&out).expect("pack info JSON");
    assert_eq!(value["name"], "rich");
    assert_eq!(value["version"], "1.2.3");
    assert_eq!(value["description"], "Rich pack");
    // Integral graph stats are emitted as integers, floats stay floats.
    assert_eq!(value["graph_stats"]["articles"], 294);
    assert_eq!(value["graph_stats"]["size_mb"], 12.5);
    // Unknown top-level keys are preserved verbatim.
    assert_eq!(value["channel"], "stable");
}

#[test]
fn cli_pack_info_reports_missing_pack() {
    let tmp = tempdir().expect("tempdir");
    let err = run(&argv(&[
        "--packs-dir",
        tmp.path().to_str().unwrap(),
        "pack",
        "info",
        "nope",
    ]))
    .expect_err("missing pack must error");
    assert_eq!(err, "pack not found: nope");
}

#[test]
fn cli_pack_validate_accepts_a_valid_pack() {
    let tmp = tempdir().expect("tempdir");
    let packs_dir = tmp.path();
    write_pack_manifest(
        packs_dir,
        "goodpack",
        r#"{"name":"goodpack","version":"2.1.0"}"#,
    );

    let out = run(&argv(&[
        "--packs-dir",
        packs_dir.to_str().unwrap(),
        "pack",
        "validate",
        "goodpack",
    ]))
    .expect("pack validate command");

    let value: serde_json::Value = serde_json::from_str(&out).expect("pack validate JSON");
    assert_eq!(value["valid"], true);
    assert_eq!(value["name"], "goodpack");
    assert_eq!(value["version"], "2.1.0");
}

#[test]
fn cli_pack_validate_rejects_an_invalid_manifest() {
    let tmp = tempdir().expect("tempdir");
    let packs_dir = tmp.path();
    // Present but schema-invalid (missing required `version`).
    write_pack_manifest(packs_dir, "brokenpack", r#"{"name":"brokenpack"}"#);

    let err = run(&argv(&[
        "--packs-dir",
        packs_dir.to_str().unwrap(),
        "pack",
        "validate",
        "brokenpack",
    ]))
    .expect_err("invalid manifest must error");
    assert!(
        err.to_lowercase().contains("version"),
        "expected a version validation error, got: {err}"
    );
}

#[test]
fn cli_pack_validate_rejects_a_traversal_name() {
    // A path-traversal name is rejected as "pack not found" BEFORE any path is
    // built (name is gated on PACK_NAME_RE), so it can never escape the packs dir.
    let tmp = tempdir().expect("tempdir");
    let err = run(&argv(&[
        "--packs-dir",
        tmp.path().to_str().unwrap(),
        "pack",
        "validate",
        "../secrets",
    ]))
    .expect_err("traversal name must error");
    assert_eq!(err, "pack not found: ../secrets");
}

#[test]
fn cli_pack_requires_a_known_subcommand() {
    let tmp = tempdir().expect("tempdir");
    let base = tmp.path().to_str().unwrap();

    let missing = run(&argv(&["--packs-dir", base, "pack"])).expect_err("bare `pack` must error");
    assert!(
        missing.contains("missing pack subcommand"),
        "got: {missing}"
    );

    let unknown = run(&argv(&["--packs-dir", base, "pack", "install"]))
        .expect_err("unknown subcommand must error");
    assert!(
        unknown.contains("unknown pack subcommand: install"),
        "got: {unknown}"
    );
}
