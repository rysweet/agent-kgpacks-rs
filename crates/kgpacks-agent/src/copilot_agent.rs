//! `kgpacks-agent` — the `CopilotAgent` client.
//!
//! Rust port of `@kgpacks/agent`'s `copilot-agent.ts`. A thin wrapper around a
//! Copilot session (via the injectable [`Transport`] seam) exposing the four
//! ported operations — answer synthesis, query expansion, multi-query
//! generation, and seed-article identification — plus token/usage accounting
//! equivalent to the reference agent's `_track_response`.
//!
//! The agent owns one session for its lifetime (`start` → use → `stop`), pins
//! the held-constant model, fails closed (returns shape-checked data or an
//! [`AgentError`]), and redacts BYOK secrets from any surfaced transport error.

use std::cell::RefCell;
use std::collections::HashSet;

use serde_json::Value;

use crate::constants::{
    DEFAULT_LIST_COUNT, DEFAULT_SYNTHESIS_MODEL, MAX_CHUNK_CHARS, MAX_CONTEXT_CHARS,
    MAX_CONTEXT_CHUNKS, MAX_LIST_COUNT, MAX_SEED_LIMIT, MIN_LIST_COUNT,
};
use crate::errors::AgentError;
use crate::json::{safe_parse_json, strip_markdown_fences};
use crate::prompts::{
    build_expand_query_prompt, build_multi_query_prompt, build_seed_article_prompt,
    build_synthesis_prompt,
};
use crate::types::{
    ContextChunk, CopilotAgentOptions, ExpandQueryOptions, MultiQueryOptions, ProviderConfig,
    SeedArticleRequest, SynthesisMetadata, SynthesisRequest, SynthesisResult, Transport,
    TransportOpenConfig, TransportResponse, TransportSession, UsageSnapshot,
};
use crate::usage::UsageTracker;

/// A graph-RAG agent over the Copilot SDK (via the injectable [`Transport`]).
///
/// Mirrors the TypeScript `CopilotAgent`. Construct with
/// [`with_transport`](CopilotAgent::with_transport) (used by tests and the CLI's
/// graph-RAG path) or, with the `copilot` feature, [`new`](CopilotAgent::new)
/// for the real RustyClawd-backed transport.
pub struct CopilotAgent {
    model: String,
    transport: Box<dyn Transport>,
    provider: Option<ProviderConfig>,
    default_timeout_ms: Option<u64>,
    usage: RefCell<UsageTracker>,
    session: Option<Box<dyn TransportSession>>,
    started: bool,
}

impl CopilotAgent {
    /// Construct an agent over an explicit [`Transport`] (the injectable seam).
    ///
    /// Construction is side-effect-free: no session is opened until
    /// [`start`](CopilotAgent::start) is called.
    pub fn with_transport(transport: Box<dyn Transport>, options: CopilotAgentOptions) -> Self {
        Self {
            model: options
                .model
                .unwrap_or_else(|| DEFAULT_SYNTHESIS_MODEL.to_string()),
            transport,
            provider: options.provider,
            default_timeout_ms: options.timeout_ms,
            usage: RefCell::new(UsageTracker::new()),
            session: None,
            started: false,
        }
    }

    /// Construct an agent over the real RustyClawd / Copilot-SDK transport.
    ///
    /// Available with the `copilot` feature. Like the reference, construction is
    /// lazy: the transport client and subprocess are only created on the first
    /// [`start`](CopilotAgent::start).
    #[cfg(feature = "copilot")]
    pub fn new(options: CopilotAgentOptions) -> Self {
        Self::with_transport(Box::new(crate::transport::copilot_transport()), options)
    }

    /// The configured (held-constant) model identifier.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Open a session pinned to the held-constant model. Idempotent.
    pub fn start(&mut self) -> Result<(), AgentError> {
        if self.started {
            return Ok(());
        }
        let config = TransportOpenConfig {
            model: self.model.clone(),
            provider: self.provider.clone(),
        };
        match self.transport.open(&config) {
            Ok(session) => {
                self.session = Some(session);
                self.started = true;
                Ok(())
            }
            Err(err) => Err(self.wrap_transport_error("start", err.message())),
        }
    }

    /// Close the session and shut the transport down. Idempotent and
    /// `start()`-safe.
    ///
    /// `shutdown()` MUST run even if `close()` fails, or the underlying client
    /// (and its resources) leak — so the transport is always shut down, and the
    /// shutdown error (if any) takes precedence, exactly as the reference's
    /// `try { try { close } finally { shutdown } }`.
    pub fn stop(&mut self) -> Result<(), AgentError> {
        if !self.started {
            return Ok(());
        }
        self.started = false;
        let close_result = match self.session.take() {
            Some(session) => session.close(),
            None => Ok(()),
        };
        let shutdown_result = self.transport.shutdown();

        match (close_result, shutdown_result) {
            (_, Err(err)) => Err(self.wrap_transport_error("stop", err.message())),
            (Err(err), Ok(())) => Err(self.wrap_transport_error("stop", err.message())),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    /// Synthesize a grounded, citation-bearing answer from retrieved context.
    pub fn synthesize_answer(
        &self,
        request: &SynthesisRequest,
    ) -> Result<SynthesisResult, AgentError> {
        let session = self.require_session()?;
        let context = bound_context(&request.context);
        let prompt = build_synthesis_prompt(&request.question, &context, request.closed_book);

        let response = self.exchange(session, &prompt, request.timeout_ms)?;
        let answer = response.content;
        if answer.trim().is_empty() {
            return Err(AgentError::response_format(
                "Model returned an empty synthesis answer.",
                &answer,
            ));
        }

        let cited_ids = derive_cited_ids(&answer, &context);
        Ok(SynthesisResult {
            metadata: SynthesisMetadata {
                cited_ids,
                model: self.model.clone(),
            },
            answer,
            usage: response.usage,
        })
    }

    /// Expand one query into related reformulations.
    pub fn expand_query(
        &self,
        query: &str,
        options: ExpandQueryOptions,
    ) -> Result<Vec<String>, AgentError> {
        let session = self.require_session()?;
        let count = clamp_count(options.count);
        let prompt = build_expand_query_prompt(query, count);
        let response = self.exchange(session, &prompt, options.timeout_ms)?;
        parse_string_array(&response.content)
    }

    /// Generate multiple paraphrased retrieval queries.
    pub fn multi_query(
        &self,
        query: &str,
        options: MultiQueryOptions,
    ) -> Result<Vec<String>, AgentError> {
        let session = self.require_session()?;
        let count = clamp_count(options.count);
        let prompt = build_multi_query_prompt(query, count);
        let response = self.exchange(session, &prompt, options.timeout_ms)?;
        parse_string_array(&response.content)
    }

    /// Select the most relevant seed-article titles for a topic.
    pub fn identify_seed_articles(
        &self,
        request: &SeedArticleRequest,
    ) -> Result<Vec<String>, AgentError> {
        let session = self.require_session()?;
        let prompt = build_seed_article_prompt(&request.topic, &request.candidates, request.limit);
        let response = self.exchange(session, &prompt, None)?;
        let titles = parse_string_array(&response.content)?;
        match request.limit {
            Some(limit) => Ok(titles.into_iter().take(clamp_limit(limit)).collect()),
            None => Ok(titles),
        }
    }

    /// Return a copy of the cumulative token/usage totals since construction.
    pub fn get_usage(&self) -> UsageSnapshot {
        self.usage.borrow().snapshot()
    }

    // ------------------------------------------------------------------------
    // Internals
    // ------------------------------------------------------------------------

    fn require_session(&self) -> Result<&dyn TransportSession, AgentError> {
        if !self.started {
            return Err(AgentError::not_started());
        }
        match &self.session {
            Some(session) => Ok(session.as_ref()),
            None => Err(AgentError::not_started()),
        }
    }

    /// Single send path: forwards the resolved timeout, records usage (so totals
    /// stay accurate even when downstream shape-validation later fails), and
    /// wraps any transport failure as a redacted [`AgentError::Transport`].
    fn exchange(
        &self,
        session: &dyn TransportSession,
        prompt: &str,
        per_call_timeout_ms: Option<u64>,
    ) -> Result<TransportResponse, AgentError> {
        let timeout_ms = per_call_timeout_ms.or(self.default_timeout_ms);
        let response = session
            .send(prompt, timeout_ms)
            .map_err(|err| self.wrap_transport_error("send", err.message()))?;
        self.usage.borrow_mut().record(&response.usage);
        Ok(response)
    }

    /// Build an [`AgentError::Transport`] whose message is scrubbed of secrets.
    fn wrap_transport_error(&self, op: &str, raw_message: &str) -> AgentError {
        let secrets = self.collect_secrets();
        let message = redact_secrets(raw_message, &secrets);
        AgentError::transport(format!("Copilot transport failed during {op}(): {message}"))
    }

    /// Gather every BYOK secret value the agent holds, for redaction.
    fn collect_secrets(&self) -> Vec<String> {
        self.provider
            .as_ref()
            .map(|p| p.secrets().into_iter().map(String::from).collect())
            .unwrap_or_default()
    }
}

/// Deterministically bound context to contain cost / DoS surface (head
/// truncation of chunks and total characters).
///
/// Character counts mirror the reference's JavaScript `String.length` /
/// `.slice()`, which count UTF-16 code units — so the caps are applied in UTF-16
/// units here too (identical to byte/char counts for the common BMP case).
fn bound_context(context: &[ContextChunk]) -> Vec<ContextChunk> {
    let mut bounded = Vec::new();
    let mut total_units = 0usize;
    for chunk in context.iter().take(MAX_CONTEXT_CHUNKS) {
        let text = truncate_utf16(&chunk.text, MAX_CHUNK_CHARS);
        let units: usize = text.chars().map(char::len_utf16).sum();
        if total_units + units > MAX_CONTEXT_CHARS {
            break;
        }
        total_units += units;
        bounded.push(ContextChunk {
            id: chunk.id.clone(),
            text,
            title: chunk.title.clone(),
        });
    }
    bounded
}

/// Truncate `text` to at most `max_units` UTF-16 code units, never splitting a
/// `char` (mirrors `text.slice(0, max_units)` for the in-range cases).
fn truncate_utf16(text: &str, max_units: usize) -> String {
    let mut units = 0usize;
    let mut out = String::new();
    for ch in text.chars() {
        let w = ch.len_utf16();
        if units + w > max_units {
            break;
        }
        units += w;
        out.push(ch);
    }
    out
}

/// Replace every known secret substring with a redaction marker.
fn redact_secrets(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for secret in secrets {
        if !secret.is_empty() {
            out = out.replace(secret.as_str(), "[REDACTED]");
        }
    }
    out
}

/// Parse fenced/bare model output into a validated `Vec<String>` (fails closed).
fn parse_string_array(content: &str) -> Result<Vec<String>, AgentError> {
    let stripped = strip_markdown_fences(content);
    let parsed = safe_parse_json(&stripped)?;
    let items = match parsed {
        Value::Array(items) => items,
        _ => {
            return Err(AgentError::response_format(
                "Expected a JSON array of strings from the model.",
                &stripped,
            ))
        }
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Value::String(s) => out.push(s),
            _ => {
                return Err(AgentError::response_format(
                    "Expected a JSON array of strings from the model.",
                    &stripped,
                ))
            }
        }
    }
    Ok(out)
}

/// Ids appearing in the answer, in first-appearance order, deduplicated.
fn derive_cited_ids(answer: &str, context: &[ContextChunk]) -> Vec<String> {
    let mut found: Vec<(usize, &str)> = context
        .iter()
        .filter_map(|chunk| index_of_id(answer, &chunk.id).map(|at| (at, chunk.id.as_str())))
        .collect();
    found.sort_by_key(|(at, _)| *at);

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (_, id) in found {
        if seen.insert(id) {
            out.push(id.to_string());
        }
    }
    out
}

/// First index of `id` in `answer` on an id boundary, so that `"Topic#1"` does
/// NOT match inside `"Topic#10"`. A real citation is bounded by a
/// non-`[A-Za-z0-9_#]` character (or string edge) on the right.
fn index_of_id(answer: &str, id: &str) -> Option<usize> {
    if id.is_empty() {
        return None;
    }
    let mut from = 0usize;
    while let Some(rel) = answer[from..].find(id) {
        let at = from + rel;
        let after = answer[at + id.len()..].chars().next();
        match after {
            None => return Some(at),
            Some(c) if !is_id_char(c) => return Some(at),
            _ => {
                // Advance past the first char of this match (a char boundary) so
                // the next `find` cannot panic on a multi-byte split.
                let step = answer[at..].chars().next().map_or(1, char::len_utf8);
                from = at + step;
            }
        }
    }
    None
}

/// Whether `c` is part of a section-id token (`[A-Za-z0-9_#]`).
fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '#'
}

/// Clamp a caller-supplied list count into the supported range.
fn clamp_count(count: Option<usize>) -> usize {
    match count {
        Some(c) => c.clamp(MIN_LIST_COUNT, MAX_LIST_COUNT),
        None => DEFAULT_LIST_COUNT,
    }
}

/// Clamp a caller-supplied seed limit into the supported range.
fn clamp_limit(limit: usize) -> usize {
    limit.min(MAX_SEED_LIMIT)
}
