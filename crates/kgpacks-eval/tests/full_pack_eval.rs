//! Full-pack CVE eval validation via the deterministic retrieval-recall metric.
//!
//! WS1's offline validation path (issue #16): with no pinned LLM judge transport
//! available, the eval report is the deterministic, LLM-free retrieval-recall
//! (hit@k) over the FULL CVE question set (no sampling). Each CVE question is
//! matched against a corpus built from the committed CVE records (their reference
//! answers), and recall@k is the fraction whose target record is in the top-k.
//!
//! This test both computes the metric and GUARDS the committed artifact
//! (`data/packs/cve/eval-results.json`) so the published numbers never drift from
//! what the code produces.

use std::collections::BTreeMap;
use std::path::PathBuf;

use kgpacks_embeddings::Embedder;
use kgpacks_eval::{
    recall_at_k, select_sample, DirQuestionLoader, QuestionLoader, RecallDoc, RecallQuery,
    SampleMode,
};
use serde_json::Value;

const KS: [usize; 3] = [1, 3, 5];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// The full-pack recall computation plus the corpus counts it was measured over.
struct FullPackRecall {
    total_questions: usize,
    cve_questions: usize,
    recall: BTreeMap<usize, f64>,
}

/// Compute recall@{1,3,5} over the full CVE question set, exactly as the
/// committed artifact reports it.
fn compute_recall() -> FullPackRecall {
    let packs_dir = repo_root().join("data").join("packs");
    let all = DirQuestionLoader::new(&packs_dir).load("cve").unwrap();

    // Full-pack: evaluate everything (no stratified sampling).
    let questions = select_sample(&all, &SampleMode::Full).unwrap();
    assert_eq!(
        questions.len(),
        all.len(),
        "full mode evaluates every question"
    );

    // The retrieval targets are the CVE-specific records; the corpus is one doc
    // per CVE question keyed by the question id, with the reference answer as its
    // text (the committed record describing that CVE).
    let cve_questions: Vec<_> = questions.iter().filter(|q| q.cve().is_some()).collect();

    let corpus: Vec<RecallDoc> = cve_questions
        .iter()
        .map(|q| RecallDoc {
            id: q.id.clone(),
            text: q
                .reference_answer
                .clone()
                .expect("CVE questions carry a reference answer"),
        })
        .collect();
    let queries: Vec<RecallQuery> = cve_questions
        .iter()
        .map(|q| RecallQuery {
            question: q.question.clone(),
            target_id: q.id.clone(),
        })
        .collect();

    FullPackRecall {
        total_questions: all.len(),
        cve_questions: queries.len(),
        recall: recall_at_k(&Embedder::bge(), &queries, &corpus, &KS),
    }
}

#[test]
fn recall_is_well_formed_and_deterministic() {
    let computed = compute_recall();
    // A value per k, each a valid fraction, monotonic non-decreasing in k.
    let mut prev = -1.0;
    for &k in &KS {
        let value = computed.recall[&k];
        assert!(
            (0.0..=1.0).contains(&value),
            "recall@{k} = {value} out of range"
        );
        assert!(value >= prev, "recall must not decrease with k");
        prev = value;
    }
    // Deterministic across recomputation.
    assert_eq!(computed.recall, compute_recall().recall);
}

#[test]
fn committed_artifact_matches_computed_recall() {
    let computed = compute_recall();
    let artifact = repo_root()
        .join("data")
        .join("packs")
        .join("cve")
        .join("eval-results.json");
    let text = std::fs::read_to_string(&artifact)
        .unwrap_or_else(|e| panic!("read {}: {e}", artifact.display()));
    let json: Value = serde_json::from_str(&text).unwrap();

    // The scalar claims must match the code, not just the recall values.
    assert_eq!(json.get("pack").and_then(Value::as_str), Some("cve"));
    assert_eq!(json.get("sampling").and_then(Value::as_str), Some("full"));
    assert_eq!(
        json.get("total_questions").and_then(Value::as_u64),
        Some(computed.total_questions as u64),
        "artifact total_questions must match the loaded corpus"
    );
    assert_eq!(
        json.get("cve_questions").and_then(Value::as_u64),
        Some(computed.cve_questions as u64),
        "artifact cve_questions must match the CVE-specific count"
    );

    let committed = json
        .get("retrieval_recall")
        .and_then(Value::as_object)
        .expect("artifact has a retrieval_recall object");

    let committed_ks: Vec<u64> = committed
        .get("k")
        .and_then(Value::as_array)
        .expect("retrieval_recall.k is an array")
        .iter()
        .filter_map(Value::as_u64)
        .collect();
    assert_eq!(
        committed_ks,
        KS.iter().map(|&k| k as u64).collect::<Vec<_>>(),
        "artifact k must match the computed ks"
    );

    for &k in &KS {
        let key = format!("recall_at_{k}");
        let committed_value = committed
            .get(&key)
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("artifact missing {key}"));
        let value = round3(computed.recall[&k]);
        assert!(
            (committed_value - value).abs() < 1e-9,
            "artifact {key} = {committed_value} but code computes {value}"
        );
    }
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}
