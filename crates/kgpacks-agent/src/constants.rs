//! `kgpacks-agent` — constants.
//!
//! Rust port of `@kgpacks/agent`'s `constants.ts`: the pinned BYOK synthesis
//! model and the safety/DoS caps applied before any prompt reaches the
//! transport. The model is *held constant* so the transport changes but the
//! model does not; changing [`DEFAULT_SYNTHESIS_MODEL`] is a re-baseline event,
//! not a routine config change.

/// BYOK model used for every operation, held constant per run. The documented,
/// constructor-overridable default (mirrors the reference
/// `DEFAULT_SYNTHESIS_MODEL`).
pub const DEFAULT_SYNTHESIS_MODEL: &str = "claude-opus-4.8";

/// Max retrieved chunks forwarded to synthesis (deterministic head truncation).
pub const MAX_CONTEXT_CHUNKS: usize = 50;

/// Max characters of any single chunk's text included in a prompt.
pub const MAX_CHUNK_CHARS: usize = 8_000;

/// Max total characters of context text included across all chunks.
pub const MAX_CONTEXT_CHARS: usize = 60_000;

/// Default number of reformulations/variants for expand/multi-query.
pub const DEFAULT_LIST_COUNT: usize = 3;

/// Lower clamp for caller-supplied list counts.
pub const MIN_LIST_COUNT: usize = 1;

/// Upper clamp for caller-supplied list counts.
pub const MAX_LIST_COUNT: usize = 20;

/// Upper clamp for a caller-supplied seed-article limit.
pub const MAX_SEED_LIMIT: usize = 100;

/// Diagnostics cap: how much offending model output an error may carry.
pub const MAX_RAW_CONTENT_CHARS: usize = 2_048;
