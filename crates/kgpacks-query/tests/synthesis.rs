//! Parity test for the M5 graph-RAG query (`retrieve_and_synthesize`):
//! retrieval (graph + vector + FTS) feeding grounded agent synthesis.
//!
//! Mirrors the reference's agent-grounded `retrieveAndSynthesize`: it builds a
//! LadybugDB pack with a known embedding fixture (so retrieval is deterministic),
//! runs hybrid retrieval, and hands the ranked sections to a `CopilotAgent`
//! backed by an OFFLINE mock transport that cites the top retrieved section.

mod common;

use common::{mix, one_hot, FixedQueryEmbedder};
use kgpacks_agent::{
    CopilotAgent, CopilotAgentOptions, Transport, TransportError, TransportOpenConfig,
    TransportResponse, TransportSession, Usage, DEFAULT_SYNTHESIS_MODEL,
};
use kgpacks_db::Database;
use kgpacks_query::{
    retrieve_and_synthesize, PackRetriever, RetrieveMode, RetrieveOptions, RetrieverConfig,
};

/// A mock transport that grounds its answer by citing the FIRST context chunk id
/// embedded in the synthesis prompt (`<chunk id="...">`). This proves the
/// retrieval → synthesis wiring without any network or model.
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
        let id = first_chunk_id(prompt).unwrap_or_else(|| "none".to_string());
        Ok(TransportResponse {
            content: format!("Grounded answer per {id}."),
            usage: Usage::new(7, 11, 0),
        })
    }
    fn close(&self) -> Result<(), TransportError> {
        Ok(())
    }
}

/// Extract the first `id="..."` value from a rendered synthesis prompt.
fn first_chunk_id(prompt: &str) -> Option<String> {
    let start = prompt.find("id=\"")? + 4;
    let rest = &prompt[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

#[test]
fn graph_rag_query_retrieves_then_synthesizes_a_grounded_answer() {
    let db = Database::in_memory().expect("db");
    let conn = db.connect().expect("conn");
    common::load_vector_ext(&conn);
    common::create_int_schema(&conn);
    common::insert_int_section(
        &conn,
        1,
        "Rust Ownership",
        "Ownership and borrowing.",
        &one_hot(0),
    );
    common::insert_int_section(
        &conn,
        2,
        "Cargo",
        "Cargo builds crates.",
        &mix(0, 1, 0.6, 0.8),
    );
    common::link_int(&conn, 1, 2);
    common::create_vector_index(&conn);

    let retriever = PackRetriever::with_embedder(
        &conn,
        FixedQueryEmbedder { vector: one_hot(0) },
        RetrieverConfig::default(),
    );

    let mut agent =
        CopilotAgent::with_transport(Box::new(CiteFirstChunk), CopilotAgentOptions::default());
    agent.start().unwrap();

    let answer = retrieve_and_synthesize(
        &retriever,
        &agent,
        "what is ownership?",
        &RetrieveOptions {
            k: Some(10),
            mode: RetrieveMode::Hybrid,
            weights: None,
        },
    )
    .unwrap();

    agent.stop().unwrap();

    // Retrieval produced grounding, and the agent synthesized over it.
    assert!(!answer.results.is_empty(), "retrieval returned sections");
    assert!(answer.answer.contains("Grounded answer"));
    assert_eq!(answer.model, DEFAULT_SYNTHESIS_MODEL);
    assert_eq!(answer.usage, Usage::new(7, 11, 0));

    // The cited id is the top retrieved section, and it is one of the hits.
    assert_eq!(answer.cited_ids.len(), 1);
    let cited = &answer.cited_ids[0];
    assert_eq!(cited, &answer.results[0].id);
    assert!(answer.results.iter().any(|r| &r.id == cited));
}
