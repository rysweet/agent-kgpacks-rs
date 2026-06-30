//! `kgpacks-agent` — error taxonomy.
//!
//! Rust port of `@kgpacks/agent`'s `errors.ts`. The agent fails closed: it
//! returns valid, shape-checked data or one of these errors. The TypeScript
//! reference models a class hierarchy (`AgentError` base with
//! `AgentNotStartedError` / `AgentTransportError` / `AgentResponseFormatError`
//! subclasses); the idiomatic Rust analogue is a single [`AgentError`] enum
//! whose variants carry the same payloads, plus predicate accessors so callers
//! (and the mirrored parity tests) can match each case.
//!
//! Errors never carry BYOK secrets — the [`AgentError::Transport`] message is
//! redacted of provider config by the agent before it is surfaced, and
//! [`AgentError::ResponseFormat`]'s `raw_content` is size-capped for safe
//! diagnostics.

use crate::constants::MAX_RAW_CONTENT_CHARS;

/// Every error this crate produces. `instanceof AgentError` in the reference
/// maps to "any [`AgentError`] value" here.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentError {
    /// An operation was used before `start()` or after `stop()`.
    ///
    /// Mirrors `AgentNotStartedError`.
    #[error("CopilotAgent is not started — call start() before using it.")]
    NotStarted,

    /// Any transport start/session/send/timeout/stop failure. The `message` has
    /// already been redacted of provider config (apiKey / bearerToken / header
    /// values) before construction.
    ///
    /// Mirrors `AgentTransportError`.
    #[error("{message}")]
    Transport {
        /// The redacted, human-readable failure description.
        message: String,
    },

    /// Model content was empty, not valid JSON after fence-stripping, or not the
    /// expected shape (e.g. not an array of strings). Carries a size-capped copy
    /// of the offending output for diagnostics.
    ///
    /// Mirrors `AgentResponseFormatError`.
    #[error("{message}")]
    ResponseFormat {
        /// What was wrong with the model output.
        message: String,
        /// The offending output, capped to [`MAX_RAW_CONTENT_CHARS`] characters.
        raw_content: String,
    },
}

impl AgentError {
    /// Construct a [`AgentError::NotStarted`].
    pub fn not_started() -> Self {
        Self::NotStarted
    }

    /// Construct a [`AgentError::Transport`] from an already-redacted message.
    pub fn transport(message: impl Into<String>) -> Self {
        Self::Transport {
            message: message.into(),
        }
    }

    /// Construct a [`AgentError::ResponseFormat`], capping `raw_content` to
    /// [`MAX_RAW_CONTENT_CHARS`] characters so a huge model payload cannot bloat
    /// diagnostics.
    pub fn response_format(message: impl Into<String>, raw_content: impl Into<String>) -> Self {
        Self::ResponseFormat {
            message: message.into(),
            raw_content: cap_raw(raw_content.into()),
        }
    }

    /// `true` for [`AgentError::NotStarted`].
    pub fn is_not_started(&self) -> bool {
        matches!(self, Self::NotStarted)
    }

    /// `true` for [`AgentError::Transport`].
    pub fn is_transport(&self) -> bool {
        matches!(self, Self::Transport { .. })
    }

    /// `true` for [`AgentError::ResponseFormat`].
    pub fn is_response_format(&self) -> bool {
        matches!(self, Self::ResponseFormat { .. })
    }

    /// The size-capped offending output, when this is a
    /// [`AgentError::ResponseFormat`].
    pub fn raw_content(&self) -> Option<&str> {
        match self {
            Self::ResponseFormat { raw_content, .. } => Some(raw_content),
            _ => None,
        }
    }
}

/// Truncate `raw` to at most [`MAX_RAW_CONTENT_CHARS`] characters (on a `char`
/// boundary), mirroring the reference's `slice(0, MAX_RAW_CONTENT_CHARS)`.
fn cap_raw(raw: String) -> String {
    if raw.chars().count() > MAX_RAW_CONTENT_CHARS {
        raw.chars().take(MAX_RAW_CONTENT_CHARS).collect()
    } else {
        raw
    }
}
