//! `kgpacks-eval` — evaluation harness (baselines, judge, metrics).
//!
//! Rust port of `@kgpacks/eval`. The M1 scaffold scores a case with a simple
//! substring match; LLM-judge grading and metric aggregation land in M4.

use kgpacks_agent::Agent;
use kgpacks_packs::PackManifest;
use kgpacks_query::Retriever;

/// A single evaluation case: a question and an expected substring.
#[derive(Debug, Clone)]
pub struct EvalCase {
    /// Question posed to the retriever.
    pub question: String,
    /// Substring expected to appear in the answer.
    pub expected: String,
}

/// Evaluation harness bound to a pack and an LLM judge.
pub struct Harness {
    pack: PackManifest,
    judge: Agent,
}

impl Harness {
    /// Build a harness for `pack` using `judge` to grade answers.
    pub fn new(pack: PackManifest, judge: Agent) -> Self {
        Self { pack, judge }
    }

    /// Identifier of the pack under evaluation.
    pub fn pack_id(&self) -> String {
        self.pack.id()
    }

    /// Model backing the judge.
    pub fn judge_model(&self) -> &str {
        self.judge.model()
    }

    /// Score a single case (M1 placeholder exact-substring match in [0, 1]).
    pub fn score(&self, retriever: &Retriever, case: &EvalCase) -> f32 {
        let answer = retriever.answer(&case.question);
        if answer.contains(&case.expected) {
            1.0
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kgpacks_db::GraphStore;
    use kgpacks_embeddings::Embedder;

    #[test]
    fn scores_matching_case() {
        let retriever = Retriever::new(
            GraphStore::open_in_memory(),
            Embedder::new(4),
            Agent::new("stub"),
        );
        let harness = Harness::new(PackManifest::new("demo", "0.1.0"), Agent::new("judge"));
        let case = EvalCase {
            question: "hi".into(),
            expected: "nodes".into(),
        };
        assert_eq!(harness.score(&retriever, &case), 1.0);
        assert_eq!(harness.judge_model(), "judge");
        assert_eq!(harness.pack_id(), "demo@0.1.0");
    }
}
