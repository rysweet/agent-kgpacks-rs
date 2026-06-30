//! `kgpacks-query` — graph-RAG query (retrieval → grounded synthesis).
//!
//! The M5 piece deferred from M4: [`retrieve_and_synthesize`] binds the M4
//! [`PackRetriever`](crate::PackRetriever) (graph + vector + FTS retrieval) to the
//! [`CopilotAgent`](kgpacks_agent::CopilotAgent) (graph-RAG synthesis), porting
//! the reference's agent-grounded `retrieveAndSynthesize`: it retrieves the top-k
//! ranked sections, hands them to the agent as citation-tagged context, and
//! returns the synthesized, grounded answer alongside the supporting hits.
//!
//! The agent's transport is injectable, so this whole path is exercised offline
//! by a mock in the parity tests; the CLI wires the real Copilot transport.

use kgpacks_agent::{ContextChunk, CopilotAgent, SynthesisRequest, Usage};

use crate::errors::Result;
use crate::types::{Embedder, RetrieveOptions, RetrieverResult};
use crate::PackRetriever;

/// A grounded graph-RAG answer: the synthesized text plus the supporting
/// retrieval evidence and token usage.
///
/// Mirrors the shape returned by the reference `retrieveAndSynthesize`.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphRagAnswer {
    /// The synthesized, grounded answer text.
    pub answer: String,
    /// Context ids the answer cited, in first-appearance order.
    pub cited_ids: Vec<String>,
    /// The held-constant model id that produced the answer.
    pub model: String,
    /// The ranked retrieval hits used as grounding (in retrieval order).
    pub results: Vec<RetrieverResult>,
    /// Token usage attributable to the synthesis call.
    pub usage: Usage,
}

/// Run the graph-RAG query: retrieve the top-k sections for `question`, then ask
/// the (already-started) `agent` to synthesize a grounded answer over them.
///
/// The retrieved sections are passed as [`ContextChunk`]s tagged by their node
/// id, so the agent can cite them; the cited ids are surfaced on the result.
/// Retrieval failures and agent failures both surface as [`crate::QueryError`].
pub fn retrieve_and_synthesize<E: Embedder>(
    retriever: &PackRetriever<'_, '_, E>,
    agent: &CopilotAgent,
    question: &str,
    opts: &RetrieveOptions,
) -> Result<GraphRagAnswer> {
    let results = retriever.retrieve(question, opts)?;

    let context: Vec<ContextChunk> = results
        .iter()
        .map(|hit| ContextChunk::new(hit.id.clone(), hit.content.clone()))
        .collect();

    let synthesis = agent.synthesize_answer(&SynthesisRequest {
        question: question.to_string(),
        context,
        timeout_ms: None,
        closed_book: false,
    })?;

    Ok(GraphRagAnswer {
        answer: synthesis.answer,
        cited_ids: synthesis.metadata.cited_ids,
        model: synthesis.metadata.model,
        results,
        usage: synthesis.usage,
    })
}
