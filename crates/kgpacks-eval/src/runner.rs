//! The run orchestrator: run both arms over a (sampled) question set, grade each
//! answer, and aggregate the report.
//!
//! Ports `packages/eval/src/runner.ts`. [`run_eval`]:
//!   1. resolves the questions from an in-memory slice or an injected loader;
//!   2. applies the sampler BEFORE answering (so cost is truly bounded);
//!   3. for each sampled question runs BOTH arms;
//!   4. grades each arm's answer with the injected [`Judge`];
//!   5. aggregates per-arm metrics and the with-pack-vs-training comparison.
//!
//! Execution is SEQUENTIAL and in-memory only: both arms and the judge typically
//! share ONE Copilot session, and concurrent sends on a single session are not
//! safe. The [`Arm`] and [`Judge`] seams are injectable, so the whole path runs
//! offline against mocks (production wires the retrieval+synthesis and
//! closed-book arms plus the pinned LLM judge).

use crate::errors::{EvalError, Result};
use crate::metrics::{aggregate_arm, compare_arms, ArmReport, ComparisonReport, JudgeVerdict};
use crate::question::{EvalQuestion, QuestionLoader};
use crate::sampling::{select_sample, SampleMode};

/// An injectable answer producer. Two arms are compared per run.
pub trait Arm {
    /// Stable arm label, surfaced in the report (`with-pack` / `training-only`).
    fn name(&self) -> &str;
    /// Produce this arm's answer for one question.
    fn answer(&self, question: &EvalQuestion) -> Result<String>;
}

/// The input handed to a [`Judge`].
#[derive(Debug, Clone)]
pub struct JudgeInput<'a> {
    /// The question being answered.
    pub question: &'a str,
    /// The candidate answer to grade.
    pub answer: &'a str,
    /// Optional gold/reference answer, included when present.
    pub reference_answer: Option<&'a str>,
}

/// The pinned grader. The same judge grades both arms.
pub trait Judge {
    /// Score one answer against its question (and optional reference).
    fn judge(&self, input: &JudgeInput<'_>) -> Result<JudgeVerdict>;
}

/// One arm's answer and its verdict for a question.
#[derive(Debug, Clone, PartialEq)]
pub struct ArmOutcome {
    /// The arm's answer text.
    pub answer: String,
    /// The judge's verdict for that answer.
    pub verdict: JudgeVerdict,
}

/// One question's per-arm answers and verdicts.
#[derive(Debug, Clone, PartialEq)]
pub struct QuestionResult {
    /// The question that was evaluated.
    pub question: EvalQuestion,
    /// The with-pack arm's outcome.
    pub with_pack: ArmOutcome,
    /// The training-only (closed-book) arm's outcome.
    pub training_only: ArmOutcome,
}

/// The full, in-memory result of a run.
#[derive(Debug, Clone, PartialEq)]
pub struct EvalReport {
    /// Per-question, per-arm answers and verdicts (in sampled order).
    pub results: Vec<QuestionResult>,
    /// The with-pack arm's aggregate.
    pub with_pack: ArmReport,
    /// The training-only arm's aggregate.
    pub training_only: ArmReport,
    /// with-pack vs training-only comparison.
    pub comparison: ComparisonReport,
    /// Questions evaluated after sampling.
    pub sampled: usize,
    /// Questions available before sampling.
    pub total: usize,
}

/// Where [`run_eval`] sources its questions from: EXACTLY ONE of an in-memory
/// slice or a loader + pack ids.
pub enum QuestionSource<'a> {
    /// Pre-loaded, in-memory questions.
    InMemory(&'a [EvalQuestion]),
    /// A loader plus the (non-empty) pack ids to load.
    Loader {
        /// The injectable loader.
        loader: &'a dyn QuestionLoader,
        /// The pack ids to load (must be non-empty).
        pack_ids: &'a [String],
    },
}

/// Options for [`run_eval`].
pub struct RunEval<'a> {
    /// Where the questions come from.
    pub source: QuestionSource<'a>,
    /// The full retrieve + synthesize arm.
    pub with_pack: &'a dyn Arm,
    /// The empty-context (no pack) baseline arm.
    pub training_only: &'a dyn Arm,
    /// The pinned judge that grades both arms.
    pub judge: &'a dyn Judge,
    /// Sampling mode (default [`SampleMode::Full`]).
    pub sample: SampleMode,
}

/// Run an evaluation end-to-end and return the in-memory report.
pub fn run_eval(options: RunEval<'_>) -> Result<EvalReport> {
    let all = resolve_questions(&options.source)?;
    let total = all.len();

    let sampled = select_sample(&all, &options.sample)?;

    let mut results: Vec<QuestionResult> = Vec::with_capacity(sampled.len());
    let mut with_pack_verdicts: Vec<JudgeVerdict> = Vec::with_capacity(sampled.len());
    let mut training_only_verdicts: Vec<JudgeVerdict> = Vec::with_capacity(sampled.len());

    for question in &sampled {
        let with_pack_answer = options.with_pack.answer(question)?;
        let with_pack_verdict = options.judge.judge(&JudgeInput {
            question: &question.question,
            answer: &with_pack_answer,
            reference_answer: question.reference_answer.as_deref(),
        })?;

        let training_only_answer = options.training_only.answer(question)?;
        let training_only_verdict = options.judge.judge(&JudgeInput {
            question: &question.question,
            answer: &training_only_answer,
            reference_answer: question.reference_answer.as_deref(),
        })?;

        with_pack_verdicts.push(with_pack_verdict.clone());
        training_only_verdicts.push(training_only_verdict.clone());
        results.push(QuestionResult {
            question: question.clone(),
            with_pack: ArmOutcome {
                answer: with_pack_answer,
                verdict: with_pack_verdict,
            },
            training_only: ArmOutcome {
                answer: training_only_answer,
                verdict: training_only_verdict,
            },
        });
    }

    Ok(EvalReport {
        with_pack: aggregate_arm(options.with_pack.name(), &with_pack_verdicts),
        training_only: aggregate_arm(options.training_only.name(), &training_only_verdicts),
        comparison: compare_arms(&with_pack_verdicts, &training_only_verdicts),
        sampled: sampled.len(),
        total,
        results,
    })
}

/// Resolve the question set from the chosen source. A `Loader` with empty
/// `pack_ids` is a configuration error caught before any arm/judge call.
fn resolve_questions(source: &QuestionSource<'_>) -> Result<Vec<EvalQuestion>> {
    match source {
        QuestionSource::InMemory(questions) => Ok(questions.to_vec()),
        QuestionSource::Loader { loader, pack_ids } => {
            if pack_ids.is_empty() {
                return Err(EvalError::Config(
                    "loader source requires a non-empty pack_ids list".into(),
                ));
            }
            let mut all = Vec::new();
            for pack_id in *pack_ids {
                all.extend(loader.load(pack_id)?);
            }
            Ok(all)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An arm that always answers with a fixed, label-tagged string.
    struct FixedArm {
        name: String,
        answer: String,
    }
    impl Arm for FixedArm {
        fn name(&self) -> &str {
            &self.name
        }
        fn answer(&self, _question: &EvalQuestion) -> Result<String> {
            Ok(self.answer.clone())
        }
    }

    /// A judge that marks an answer correct iff it contains the reference answer.
    struct SubstringJudge;
    impl Judge for SubstringJudge {
        fn judge(&self, input: &JudgeInput<'_>) -> Result<JudgeVerdict> {
            let correct = input
                .reference_answer
                .map(|r| input.answer.contains(r))
                .unwrap_or(false);
            Ok(JudgeVerdict {
                correct,
                score: if correct { 1.0 } else { 0.0 },
                reasoning: "substring match".into(),
            })
        }
    }

    fn question(id: &str, reference: &str) -> EvalQuestion {
        EvalQuestion {
            id: id.into(),
            question: format!("q {id}"),
            reference_answer: Some(reference.into()),
            pack_id: "cve".into(),
            skill: None,
            metadata: None,
        }
    }

    #[test]
    fn runs_both_arms_and_aggregates() {
        let questions = vec![question("a", "backdoor"), question("b", "overflow")];
        let with_pack = FixedArm {
            name: "with-pack".into(),
            answer: "a backdoor and an overflow".into(),
        };
        let training_only = FixedArm {
            name: "training-only".into(),
            answer: "no idea".into(),
        };
        let report = run_eval(RunEval {
            source: QuestionSource::InMemory(&questions),
            with_pack: &with_pack,
            training_only: &training_only,
            judge: &SubstringJudge,
            sample: SampleMode::Full,
        })
        .unwrap();

        assert_eq!(report.total, 2);
        assert_eq!(report.sampled, 2);
        assert_eq!(report.with_pack.accuracy, 1.0);
        assert_eq!(report.training_only.accuracy, 0.0);
        assert_eq!(report.comparison.delta_accuracy, 1.0);
        assert_eq!(report.comparison.wins, 2);
    }

    #[test]
    fn loader_source_requires_pack_ids() {
        struct EmptyLoader;
        impl QuestionLoader for EmptyLoader {
            fn load(&self, _pack_id: &str) -> Result<Vec<EvalQuestion>> {
                Ok(vec![])
            }
        }
        let with_pack = FixedArm {
            name: "with-pack".into(),
            answer: "x".into(),
        };
        let training_only = FixedArm {
            name: "training-only".into(),
            answer: "y".into(),
        };
        let out = run_eval(RunEval {
            source: QuestionSource::Loader {
                loader: &EmptyLoader,
                pack_ids: &[],
            },
            with_pack: &with_pack,
            training_only: &training_only,
            judge: &SubstringJudge,
            sample: SampleMode::Full,
        });
        assert!(out.is_err());
    }
}
