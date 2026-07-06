//! `kgpacks-eval` — the evaluation harness (questions, sampling, judging, metrics).
//!
//! Rust port of `@kgpacks/eval`. It turns a pack's committed question set into a
//! with-pack-vs-training-only comparison:
//!
//! * [`question`] — the [`EvalQuestion`] model and the path-confined
//!   [`DirQuestionLoader`] (`loader.ts` / `types.ts`).
//! * [`sampling`] — deterministic [`SampleMode::Full`] (full-pack validation) vs.
//!   [`SampleMode::Stratified`] selection (`sampling.ts`).
//! * [`metrics`] — pure per-arm and head-to-head aggregation (`metrics.ts`).
//! * [`runner`] — [`run_eval`], the sequential orchestrator over injectable
//!   [`Arm`]/[`Judge`] seams (`runner.ts`).
//! * [`recall`] — the deterministic, LLM-free retrieval-recall (hit@k) fallback
//!   metric used to validate the full CVE pack offline when no pinned judge
//!   transport is available.
//!
//! Every seam is injectable, so the whole suite runs offline against mocks; the
//! real retrieval+synthesis / closed-book arms and the pinned LLM judge are wired
//! where a Copilot transport is available.
//!
//! The M1 placeholder [`Harness`] / [`EvalCase`] are retained in the [`legacy`]
//! module for the CLI `demo` smoke test.

pub mod errors;
mod legacy;
pub mod metrics;
pub mod question;
pub mod recall;
pub mod runner;
pub mod sampling;

pub use errors::{EvalError, Result};

pub use question::{DirQuestionLoader, EvalQuestion, QuestionLoader, EVAL_QUESTIONS_FILENAME};

pub use sampling::{select_sample, SampleMode, DEFAULT_PER_PACK};

pub use metrics::{aggregate_arm, compare_arms, ArmReport, ComparisonReport, JudgeVerdict};

pub use runner::{
    run_eval, Arm, ArmOutcome, EvalReport, Judge, JudgeInput, QuestionResult, QuestionSource,
    RunEval,
};

pub use recall::{recall_at_k, RecallDoc, RecallQuery};

// ── M1 placeholder (retained for the CLI `demo` smoke test) ─────────────────

pub use legacy::{EvalCase, Harness};
