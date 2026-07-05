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
    // `status` needs no transport: drive it through the production `run` entry
    // to prove the dispatch + registry read-path end to end over a real packs
    // directory (one pack with a graph store, one without, plus a manifest-less
    // directory that must be skipped).
    let tmp = tempdir().expect("tempdir");
    let packs_dir = tmp.path();

    let with_db = packs_dir.join("rustpack");
    fs::create_dir_all(&with_db).expect("mkdir rustpack");
    fs::write(
        with_db.join("manifest.json"),
        r#"{"name":"rustpack","version":"1.4.0"}"#,
    )
    .expect("write manifest");
    build_pack_db(&with_db.join("graph.lbug"));

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
