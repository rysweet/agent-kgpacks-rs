//! `kgpacks-agent` — graph-RAG agent over the GitHub Copilot SDK.
//!
//! Rust port of `@kgpacks/agent`. The LLM layer of the port: a wrapper around a
//! Copilot session exposing four operations — answer synthesis, query expansion,
//! multi-query generation, and seed-article identification — plus usage
//! accounting. The transport is injectable ([`Transport`]), so the whole suite
//! runs offline against a mock; the real `rustyclawd-core` / Copilot-SDK adapter
//! is provided behind the `copilot` feature.
//!
//! ```
//! use kgpacks_agent::{CopilotAgent, CopilotAgentOptions, SynthesisRequest, ContextChunk};
//! # use kgpacks_agent::{Transport, TransportSession, TransportOpenConfig, TransportResponse, TransportError, Usage};
//! # struct OneShot;
//! # impl TransportSession for OneShot {
//! #   fn send(&self, _p: &str, _t: Option<u64>) -> Result<TransportResponse, TransportError> {
//! #     Ok(TransportResponse { content: "Grounded answer citing doc:1.".into(), usage: Usage::new(1, 1, 0) })
//! #   }
//! #   fn close(&self) -> Result<(), TransportError> { Ok(()) }
//! # }
//! # struct MockTransport;
//! # impl Transport for MockTransport {
//! #   fn open(&self, _c: &TransportOpenConfig) -> Result<Box<dyn TransportSession>, TransportError> { Ok(Box::new(OneShot)) }
//! #   fn shutdown(&self) -> Result<(), TransportError> { Ok(()) }
//! # }
//! let mut agent = CopilotAgent::with_transport(Box::new(MockTransport), CopilotAgentOptions::default());
//! agent.start().unwrap();
//! let result = agent.synthesize_answer(&SynthesisRequest {
//!     question: "What is HNSW?".into(),
//!     context: vec![ContextChunk::new("doc:1", "HNSW is a navigable small-world graph.")],
//!     ..SynthesisRequest::default()
//! }).unwrap();
//! assert_eq!(result.metadata.cited_ids, ["doc:1"]);
//! agent.stop().unwrap();
//! ```

mod constants;
mod copilot_agent;
mod errors;
mod json;
mod legacy;
mod prompts;
#[cfg(feature = "copilot")]
mod transport;
mod types;
mod usage;

// ── M5 graph-RAG agent surface ─────────────────────────────────────────────

pub use constants::DEFAULT_SYNTHESIS_MODEL;
pub use copilot_agent::CopilotAgent;
pub use errors::AgentError;
pub use json::{safe_parse_json, strip_markdown_fences};
pub use prompts::{
    build_expand_query_prompt, build_multi_query_prompt, build_seed_article_prompt,
    build_synthesis_prompt,
};
pub use types::{
    ContextChunk, CopilotAgentOptions, ExpandQueryOptions, MultiQueryOptions, ProviderConfig,
    SeedArticleRequest, SynthesisMetadata, SynthesisRequest, SynthesisResult, Transport,
    TransportError, TransportOpenConfig, TransportResponse, TransportSession, Usage, UsageSnapshot,
};
pub use usage::UsageTracker;

#[cfg(feature = "copilot")]
pub use transport::{copilot_transport, CopilotSession, CopilotTransport};

// ── M1 placeholder (retained for the not-yet-wired sibling crates) ──────────

pub use legacy::Agent;
