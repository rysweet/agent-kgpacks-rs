//! `kgpacks-agent` — public type contracts and the injectable transport seam.
//!
//! Rust port of `@kgpacks/agent`'s `types.ts`. These are the crate's stability
//! surface: the four operations' request/result shapes, the usage records, and
//! the injectable [`Transport`] seam that lets tests run fully offline against a
//! mock (never opening a real Copilot session).

use std::collections::BTreeMap;
use std::fmt;

/// A retrieved context passage made available to synthesis.
///
/// Mirrors the TypeScript `ContextChunk`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContextChunk {
    /// Stable node/document id used for citation (e.g. `"doc:42"`).
    pub id: String,
    /// The passage text.
    pub text: String,
    /// Optional human-facing title/source.
    pub title: Option<String>,
}

impl ContextChunk {
    /// A chunk with an `id` and `text` and no title.
    pub fn new(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            title: None,
        }
    }

    /// Builder: attach a title.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
}

/// Token counts for a single call. Mirrors the reference agent's
/// `_track_response` fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    /// Prompt/input tokens.
    pub prompt_tokens: u64,
    /// Completion/output tokens.
    pub completion_tokens: u64,
    /// Reasoning tokens (0 when absent).
    pub reasoning_tokens: u64,
    /// `prompt + completion + reasoning`.
    pub total_tokens: u64,
}

impl Usage {
    /// Build a [`Usage`] with a self-consistent `total_tokens`.
    pub fn new(prompt_tokens: u64, completion_tokens: u64, reasoning_tokens: u64) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            reasoning_tokens,
            total_tokens: prompt_tokens + completion_tokens + reasoning_tokens,
        }
    }
}

/// Cumulative usage + request count since the agent was constructed.
///
/// Mirrors the TypeScript `UsageSnapshot` (which `extends Usage`); the fields
/// are flattened here so equality assertions read directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UsageSnapshot {
    /// Cumulative prompt/input tokens.
    pub prompt_tokens: u64,
    /// Cumulative completion/output tokens.
    pub completion_tokens: u64,
    /// Cumulative reasoning tokens.
    pub reasoning_tokens: u64,
    /// Cumulative total tokens.
    pub total_tokens: u64,
    /// Number of model exchanges recorded.
    pub request_count: u64,
}

/// A request to synthesize a grounded answer. Mirrors `SynthesisRequest`.
#[derive(Debug, Clone, Default)]
pub struct SynthesisRequest {
    /// The user question to answer.
    pub question: String,
    /// Retrieved context, in retrieval order. Empty ⇒ the model is told it lacks
    /// grounding (unless `closed_book`).
    pub context: Vec<ContextChunk>,
    /// Optional per-call timeout override (ms).
    pub timeout_ms: Option<u64>,
    /// Closed-book mode (default `false`). When `true` AND `context` is empty,
    /// the model is asked to answer from its OWN training knowledge instead of
    /// refusing — used by the eval's no-pack baseline to measure parametric
    /// knowledge. Production RAG leaves this `false`.
    pub closed_book: bool,
}

/// Structured metadata about a synthesis. Mirrors `SynthesisMetadata`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SynthesisMetadata {
    /// Context ids the answer cited, in first-appearance order within the answer.
    pub cited_ids: Vec<String>,
    /// Model id that produced the answer (the held-constant BYOK model).
    pub model: String,
}

/// The result of a synthesis. Mirrors `SynthesisResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthesisResult {
    /// The synthesized answer text.
    pub answer: String,
    /// Structured metadata about the synthesis.
    pub metadata: SynthesisMetadata,
    /// Tokens attributable to THIS call (not cumulative).
    pub usage: Usage,
}

/// Options for [`crate::CopilotAgent::expand_query`]. Mirrors
/// `ExpandQueryOptions`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExpandQueryOptions {
    /// Target number of reformulations (default 3, clamped to a sane range).
    pub count: Option<usize>,
    /// Per-call timeout override (ms).
    pub timeout_ms: Option<u64>,
}

/// Options for [`crate::CopilotAgent::multi_query`]. Mirrors `MultiQueryOptions`.
#[derive(Debug, Clone, Copy, Default)]
pub struct MultiQueryOptions {
    /// Number of paraphrased variants (default 3, clamped).
    pub count: Option<usize>,
    /// Per-call timeout override (ms).
    pub timeout_ms: Option<u64>,
}

/// A request to select the most relevant seed-article titles. Mirrors
/// `SeedArticleRequest`.
#[derive(Debug, Clone, Default)]
pub struct SeedArticleRequest {
    /// The domain/topic to find seeds for.
    pub topic: String,
    /// Candidate article titles to choose from.
    pub candidates: Vec<String>,
    /// Optional cap on the number of titles returned.
    pub limit: Option<usize>,
}

/// BYOK provider config for the held-constant model. Sourced only from
/// env/secret store; never logged, never placed in [`Usage`], and redacted from
/// errors. Mirrors `ProviderConfig`.
#[derive(Debug, Clone, Default)]
pub struct ProviderConfig {
    /// Provider type (`"openai"` / `"azure"` / `"anthropic"`).
    pub type_: Option<String>,
    /// Override base URL.
    pub base_url: Option<String>,
    /// Secret API key.
    pub api_key: Option<String>,
    /// Secret bearer token.
    pub bearer_token: Option<String>,
    /// Extra headers (values are treated as secret for redaction).
    pub headers: BTreeMap<String, String>,
}

impl ProviderConfig {
    /// Collect every secret value this provider holds, for redaction.
    pub(crate) fn secrets(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        if let Some(k) = self.api_key.as_deref() {
            out.push(k);
        }
        if let Some(t) = self.bearer_token.as_deref() {
            out.push(t);
        }
        for value in self.headers.values() {
            out.push(value.as_str());
        }
        out
    }
}

/// Config used to open a tool-less, model-pinned session. Mirrors
/// `TransportOpenConfig`.
#[derive(Debug, Clone, Default)]
pub struct TransportOpenConfig {
    /// The held-constant model id.
    pub model: String,
    /// BYOK provider (endpoint + key/token) for the held-constant model.
    pub provider: Option<ProviderConfig>,
}

/// The assistant text plus token usage from one exchange. Mirrors
/// `TransportResponse`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportResponse {
    /// The assistant's text content.
    pub content: String,
    /// Token usage attributable to this exchange.
    pub usage: Usage,
}

/// An opaque failure from the transport layer. Carries only a human-readable
/// message; the agent redacts secrets from it before surfacing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError {
    message: String,
}

impl TransportError {
    /// Build a transport error from a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The raw (un-redacted) failure message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TransportError {}

impl From<String> for TransportError {
    fn from(message: String) -> Self {
        Self { message }
    }
}

impl From<&str> for TransportError {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

/// One in-flight model exchange. Maps 1:1 onto a Copilot session. Mirrors
/// `TransportSession`.
pub trait TransportSession {
    /// Send a prompt and return the assistant text + token usage.
    fn send(
        &self,
        prompt: &str,
        timeout_ms: Option<u64>,
    ) -> Result<TransportResponse, TransportError>;
    /// Tear the session down.
    fn close(&self) -> Result<(), TransportError>;
}

/// The injectable boundary. The real adapter wraps the Copilot client. Mirrors
/// `Transport`.
pub trait Transport {
    /// Open a tool-less session pinned to a model/provider.
    fn open(
        &self,
        config: &TransportOpenConfig,
    ) -> Result<Box<dyn TransportSession>, TransportError>;
    /// Stop the underlying client.
    fn shutdown(&self) -> Result<(), TransportError>;
}

/// Construction options for [`crate::CopilotAgent`]. Mirrors
/// `CopilotAgentOptions` (the `transport` seam is injected separately via
/// [`crate::CopilotAgent::with_transport`] to keep it strongly typed).
#[derive(Debug, Clone, Default)]
pub struct CopilotAgentOptions {
    /// BYOK model id used for all operations, held constant per run.
    pub model: Option<String>,
    /// BYOK provider (endpoint + key/token) for the held-constant model.
    pub provider: Option<ProviderConfig>,
    /// Default per-request timeout (ms), overridable per call.
    pub timeout_ms: Option<u64>,
}
