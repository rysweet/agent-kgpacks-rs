//! Schema/acceptance test for the committed CVE eval question set.
//!
//! Encodes issue #16's acceptance: the CVE eval set must load through the
//! path-confined [`DirQuestionLoader`] from `data/packs/cve/eval_questions.json`
//! and be a non-trivial, well-formed set EXTENDED with real, recent (2024/2025)
//! CVEs that carry reference answers — so the eval exercises knowledge the base
//! model is least likely to already hold (maximizing `comparison.delta_accuracy`).

use std::path::PathBuf;

use kgpacks_eval::{DirQuestionLoader, EvalQuestion, QuestionLoader};

/// `<repo>/data/packs` — the crate is at `<repo>/crates/kgpacks-eval`.
fn packs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("data")
        .join("packs")
}

fn load_cve_questions() -> Vec<EvalQuestion> {
    DirQuestionLoader::new(packs_dir())
        .load("cve")
        .expect("cve eval questions load through the path-confined loader")
}

#[test]
fn loads_a_non_trivial_set_of_well_formed_questions() {
    let questions = load_cve_questions();
    assert!(
        questions.len() >= 12,
        "expected >= 12 questions, got {}",
        questions.len()
    );
    for q in &questions {
        assert!(!q.id.is_empty(), "every question has a non-empty id");
        assert!(
            !q.question.is_empty(),
            "question '{}' has a non-empty prompt",
            q.id
        );
        assert_eq!(q.pack_id, "cve", "loader stamps pack_id = cve");
    }
}

#[test]
fn has_unique_question_ids() {
    let questions = load_cve_questions();
    let mut ids: Vec<&str> = questions.iter().map(|q| q.id.as_str()).collect();
    ids.sort_unstable();
    let unique = ids.len();
    ids.dedup();
    assert_eq!(unique, ids.len(), "question ids must be unique");
}

#[test]
fn is_extended_with_recent_cves_carrying_reference_answers() {
    let questions = load_cve_questions();
    let recent: Vec<&EvalQuestion> = questions
        .iter()
        .filter(|q| q.references_recent_cve())
        .collect();
    assert!(
        recent.len() >= 6,
        "expected >= 6 recent (2024/2025) CVE questions, got {}",
        recent.len()
    );
    for q in &recent {
        let reference = q.reference_answer.as_deref().unwrap_or_else(|| {
            panic!(
                "recent CVE question '{}' must carry a reference answer",
                q.id
            )
        });
        assert!(
            !reference.is_empty(),
            "recent CVE question '{}' has a non-empty reference answer",
            q.id
        );
    }
}

#[test]
fn has_at_least_six_verifiable_cve_ids_with_reference_answers() {
    // Stronger than the year-tag check: require >= 6 questions that carry a
    // well-formed, verifiable CVE-2024/2025 identifier AND a non-empty reference
    // answer (the "real, verifiable 2024/2025 CVEs" acceptance in #16).
    let questions = load_cve_questions();
    let verifiable: Vec<&EvalQuestion> = questions
        .iter()
        .filter(|q| {
            q.recent_cve_id().is_some()
                && q.reference_answer.as_deref().is_some_and(|r| !r.is_empty())
        })
        .collect();
    assert!(
        verifiable.len() >= 6,
        "expected >= 6 verifiable recent-CVE questions with reference answers, got {}",
        verifiable.len()
    );
    // Each verifiable CVE id is distinct (no duplicate CVEs padding the count).
    let mut cves: Vec<&str> = verifiable
        .iter()
        .filter_map(|q| q.recent_cve_id())
        .collect();
    cves.sort_unstable();
    let distinct = cves.len();
    cves.dedup();
    assert_eq!(distinct, cves.len(), "verifiable CVE ids must be distinct");
}
