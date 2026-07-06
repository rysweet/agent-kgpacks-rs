//! Deterministic question sampling (full vs. stratified).
//!
//! Ports `packages/eval/src/sampling.ts`. [`SampleMode::Full`] evaluates the
//! **complete** pack (the WS1 full-pack validation path); [`SampleMode::Stratified`]
//! bounds LLM cost during development by taking a few questions per pack. The
//! stratified selection is REPRODUCIBLE — group by pack (sub-stratify by skill
//! when present), preserve input order, round-robin across skill buckets, take
//! the first-N (no RNG) — so two runs over the same input evaluate the same
//! questions and assertions never flake.

use crate::errors::{EvalError, Result};
use crate::question::EvalQuestion;

/// Default questions-per-pack in [`SampleMode::Stratified`].
pub const DEFAULT_PER_PACK: usize = 3;

/// How many questions a run scores.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SampleMode {
    /// Evaluate every question (full-pack validation).
    #[default]
    Full,
    /// Take up to `per_pack` questions per pack, round-robin across skills.
    Stratified {
        /// Questions per pack. Must be a positive integer.
        per_pack: usize,
    },
}

/// Deterministically reduce a question set.
///
/// In [`SampleMode::Full`] the input is returned unchanged. In
/// [`SampleMode::Stratified`] questions are grouped by `pack_id` (in
/// first-appearance order, each pack's questions kept in input order),
/// sub-stratified by `skill`, and the first `per_pack` of each pack are taken —
/// so the result is reproducible and bounded by `per_pack × pack_count`.
///
/// Errors with [`EvalError::Config`] when `per_pack` is zero.
pub fn select_sample(questions: &[EvalQuestion], mode: &SampleMode) -> Result<Vec<EvalQuestion>> {
    let per_pack = match mode {
        SampleMode::Full => return Ok(questions.to_vec()),
        SampleMode::Stratified { per_pack } => *per_pack,
    };
    if per_pack == 0 {
        return Err(EvalError::Config(
            "sample per_pack must be a positive integer".into(),
        ));
    }

    // Group by pack in first-appearance order, keeping each pack's questions in
    // input order — this is what makes "first-N" deterministic.
    let mut pack_order: Vec<String> = Vec::new();
    let mut packs: Vec<Vec<&EvalQuestion>> = Vec::new();
    for question in questions {
        match pack_order.iter().position(|p| p == &question.pack_id) {
            Some(idx) => packs[idx].push(question),
            None => {
                pack_order.push(question.pack_id.clone());
                packs.push(vec![question]);
            }
        }
    }

    let mut selected: Vec<EvalQuestion> = Vec::new();
    for pack in &packs {
        for question in take_stratified_by_skill(pack, per_pack) {
            selected.push(question.clone());
        }
    }
    Ok(selected)
}

/// Sentinel skill bucket for questions without a `skill` tag.
const NO_SKILL: &str = "\u{0}__no_skill__";

/// Take up to `per_pack` questions from one pack, round-robining across skill
/// buckets (in first-appearance order) so a small `per_pack` samples ACROSS
/// skills rather than collapsing onto one. With no skills there is a single
/// bucket, so this degrades to a stable first-N.
fn take_stratified_by_skill<'a>(
    pack: &[&'a EvalQuestion],
    per_pack: usize,
) -> Vec<&'a EvalQuestion> {
    let mut skill_order: Vec<&str> = Vec::new();
    let mut buckets: Vec<Vec<&EvalQuestion>> = Vec::new();
    for question in pack {
        let key = question.skill.as_deref().unwrap_or(NO_SKILL);
        match skill_order.iter().position(|s| *s == key) {
            Some(idx) => buckets[idx].push(question),
            None => {
                skill_order.push(key);
                buckets.push(vec![question]);
            }
        }
    }

    let mut taken: Vec<&EvalQuestion> = Vec::new();
    let mut round = 0;
    while taken.len() < per_pack {
        let mut progressed = false;
        for bucket in &buckets {
            if let Some(question) = bucket.get(round) {
                taken.push(question);
                progressed = true;
                if taken.len() >= per_pack {
                    break;
                }
            }
        }
        if !progressed {
            break; // every bucket exhausted
        }
        round += 1;
    }
    taken
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(id: &str, pack: &str, skill: Option<&str>) -> EvalQuestion {
        EvalQuestion {
            id: id.into(),
            question: format!("question {id}"),
            reference_answer: None,
            pack_id: pack.into(),
            skill: skill.map(str::to_string),
            metadata: None,
        }
    }

    #[test]
    fn full_returns_everything_in_order() {
        let questions = vec![q("a", "cve", None), q("b", "cve", None)];
        let out = select_sample(&questions, &SampleMode::Full).unwrap();
        assert_eq!(out, questions);
    }

    #[test]
    fn stratified_bounds_per_pack_and_is_deterministic() {
        let questions = vec![
            q("a", "cve", Some("cve_lookup")),
            q("b", "cve", Some("cve_lookup")),
            q("c", "cve", Some("concept")),
            q("d", "other", None),
        ];
        let out = select_sample(&questions, &SampleMode::Stratified { per_pack: 2 }).unwrap();
        // 2 from `cve` (round-robin across the two skills => a, c), 1 from `other`.
        let ids: Vec<&str> = out.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, ["a", "c", "d"]);
        // Reproducible.
        let again = select_sample(&questions, &SampleMode::Stratified { per_pack: 2 }).unwrap();
        assert_eq!(out, again);
    }

    #[test]
    fn stratified_rejects_zero_per_pack() {
        let questions = vec![q("a", "cve", None)];
        assert!(select_sample(&questions, &SampleMode::Stratified { per_pack: 0 }).is_err());
    }
}
