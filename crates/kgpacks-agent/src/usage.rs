//! `kgpacks-agent` — token/usage accountant.
//!
//! Rust port of `@kgpacks/agent`'s `usage.ts` (the TS analogue of the reference
//! agent's `_track_response`): accumulates prompt/completion/reasoning/total
//! tokens plus a request count across every call in a session.
//! [`crate::CopilotAgent::get_usage`] returns a snapshot of it.

use crate::types::{Usage, UsageSnapshot};

/// Running totals of token usage and request count for one agent's lifetime.
#[derive(Debug, Default, Clone)]
pub struct UsageTracker {
    prompt_tokens: u64,
    completion_tokens: u64,
    reasoning_tokens: u64,
    total_tokens: u64,
    request_count: u64,
}

impl UsageTracker {
    /// A fresh tracker with all counters at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one call's usage into the running totals and count the request.
    pub fn record(&mut self, usage: &Usage) {
        self.prompt_tokens += usage.prompt_tokens;
        self.completion_tokens += usage.completion_tokens;
        self.reasoning_tokens += usage.reasoning_tokens;
        self.total_tokens += usage.total_tokens;
        self.request_count += 1;
    }

    /// Return an independent copy of the cumulative totals (never the internals).
    pub fn snapshot(&self) -> UsageSnapshot {
        UsageSnapshot {
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
            reasoning_tokens: self.reasoning_tokens,
            total_tokens: self.total_tokens,
            request_count: self.request_count,
        }
    }
}
