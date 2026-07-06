//! Typed error hierarchy for `kgpacks-eval`.
//!
//! Ports `packages/eval/src/errors.ts`. Every fallible surface in the crate —
//! question loading, sampling configuration, and the run orchestrator — shares a
//! single [`EvalError`] so callers can match the whole family or discriminate by
//! variant.

use thiserror::Error;

/// The error family for evaluation question loading, sampling and running.
#[derive(Error, Debug)]
pub enum EvalError {
    /// A pack's eval questions could not be located, read, parsed, or validated.
    #[error("eval question load error: {0}")]
    QuestionLoad(String),

    /// An invalid run/sampling configuration (e.g. a non-positive `per_pack`, or
    /// neither/both of `questions` and `loader`).
    #[error("eval configuration error: {0}")]
    Config(String),
}

/// Convenience result alias for fallible `kgpacks-eval` operations.
pub type Result<T> = std::result::Result<T, EvalError>;
