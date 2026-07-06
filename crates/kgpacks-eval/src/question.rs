//! Eval question model + the path-confined directory loader.
//!
//! Ports `packages/eval/src/{types.ts,loader.ts}`. An [`EvalQuestion`] is one
//! prompt fed to both eval arms and graded by the judge; a [`QuestionLoader`] is
//! the injectable seam that supplies a pack's question set (tests inject
//! in-memory fixtures and never touch disk).
//!
//! [`DirQuestionLoader`] is the default, disk-backed loader. It reads a pack's
//! questions from `<base_dir>/<pack_id>/eval_questions.json` and confines all
//! file access to `base_dir`: `pack_id` is validated against the pack-name
//! grammar ([`kgpacks_packs::pack_name_re`]) — which rejects absolute paths,
//! `..` traversal, path separators and NUL — and the resolved path is
//! re-checked for containment as defence-in-depth.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use kgpacks_packs::pack_name_re;

use crate::errors::{EvalError, Result};

/// The per-pack file [`DirQuestionLoader`] reads eval questions from.
pub const EVAL_QUESTIONS_FILENAME: &str = "eval_questions.json";

/// One evaluation question fed to both arms and graded by the judge.
///
/// Mirrors the TypeScript `EvalQuestion`. `id` and `question` are required; the
/// on-disk JSON carries snake_case keys (`id`, `question`, `reference_answer`,
/// `skill`, `metadata`) and the loader stamps [`pack_id`](EvalQuestion::pack_id)
/// from the requested pack.
#[derive(Debug, Clone, PartialEq)]
pub struct EvalQuestion {
    /// Stable question id (used for traceability and deterministic sampling).
    pub id: String,
    /// The question prompt fed to both arms.
    pub question: String,
    /// Optional gold answer, passed to the judge/evaluator when present.
    pub reference_answer: Option<String>,
    /// Owning pack id — the stratification key for sampling.
    pub pack_id: String,
    /// Optional skill tag selecting a per-skill evaluator.
    pub skill: Option<String>,
    /// Optional opaque metadata carried through to the report.
    pub metadata: Option<Map<String, Value>>,
}

impl EvalQuestion {
    /// The CVE identifier this question targets, read from `metadata.cve`, if any.
    pub fn cve(&self) -> Option<&str> {
        self.metadata
            .as_ref()
            .and_then(|m| m.get("cve"))
            .and_then(Value::as_str)
    }

    /// Whether this question references a real, recent (2024/2025) CVE.
    ///
    /// True when the question or reference answer mentions a `CVE-2024-…` /
    /// `CVE-2025-…` id, or `metadata.year` is 2024 or 2025 — mirroring the schema
    /// test's `RECENT_CVE_RE` / `year` check in the reference.
    pub fn references_recent_cve(&self) -> bool {
        if mentions_recent_cve(&self.question) {
            return true;
        }
        if self
            .reference_answer
            .as_deref()
            .is_some_and(mentions_recent_cve)
        {
            return true;
        }
        matches!(self.metadata_year(), Some(2024) | Some(2025))
    }

    fn metadata_year(&self) -> Option<i64> {
        self.metadata
            .as_ref()
            .and_then(|m| m.get("year"))
            .and_then(Value::as_i64)
    }
}

/// True when `text` mentions a recent (2024/2025) CVE identifier.
fn mentions_recent_cve(text: &str) -> bool {
    has_cve_with_prefix(text, "CVE-2024-") || has_cve_with_prefix(text, "CVE-2025-")
}

/// True when `text` contains `prefix` immediately followed by ≥3 digits (the
/// `CVE-20YY-\d{3,}` shape, matched without a regex dependency).
fn has_cve_with_prefix(text: &str, prefix: &str) -> bool {
    text.match_indices(prefix).any(|(idx, _)| {
        let rest = &text[idx + prefix.len()..];
        rest.chars().take_while(|c| c.is_ascii_digit()).count() >= 3
    })
}

/// Loads a pack's eval questions. Injectable so tests use in-memory fixtures.
pub trait QuestionLoader {
    /// Load the eval questions for one pack.
    fn load(&self, pack_id: &str) -> Result<Vec<EvalQuestion>>;
}

/// The default, path-confined [`QuestionLoader`] rooted at a base directory.
///
/// Each [`load`](DirQuestionLoader::load) validates the id, resolves
/// `<base_dir>/<pack_id>/eval_questions.json`, asserts it stays under `base_dir`,
/// reads it, and normalises each entry into an [`EvalQuestion`] (with `pack_id`
/// forced to the requested pack).
pub struct DirQuestionLoader {
    root: PathBuf,
}

impl DirQuestionLoader {
    /// Build a loader confined to `base_dir`.
    pub fn new(base_dir: impl AsRef<Path>) -> Self {
        Self {
            root: base_dir.as_ref().to_path_buf(),
        }
    }
}

impl QuestionLoader for DirQuestionLoader {
    fn load(&self, pack_id: &str) -> Result<Vec<EvalQuestion>> {
        assert_safe_pack_id(pack_id)?;

        let pack_dir = self.root.join(pack_id);
        // Defence-in-depth: `assert_safe_pack_id` already rejects traversal, but
        // re-check that the resolved directory is exactly one child of the root.
        if !is_direct_child(&self.root, &pack_dir, pack_id) {
            return Err(EvalError::QuestionLoad(format!(
                "packId '{pack_id}' escapes the loader base directory"
            )));
        }

        let file = pack_dir.join(EVAL_QUESTIONS_FILENAME);
        let text = std::fs::read_to_string(&file).map_err(|err| {
            EvalError::QuestionLoad(format!(
                "failed to read eval questions for pack '{pack_id}': {err}"
            ))
        })?;

        let parsed: Value = serde_json::from_str(&text).map_err(|err| {
            EvalError::QuestionLoad(format!(
                "eval questions for pack '{pack_id}' are not valid JSON: {err}"
            ))
        })?;
        let array = parsed.as_array().ok_or_else(|| {
            EvalError::QuestionLoad(format!(
                "eval questions for pack '{pack_id}' must be a JSON array of questions"
            ))
        })?;

        array
            .iter()
            .enumerate()
            .map(|(index, entry)| normalise_question(entry, pack_id, index))
            .collect()
    }
}

/// Reject any `pack_id` that is not a bare, safe pack name.
fn assert_safe_pack_id(pack_id: &str) -> Result<()> {
    if pack_name_re().is_match(pack_id) {
        Ok(())
    } else {
        Err(EvalError::QuestionLoad(format!(
            "invalid packId '{pack_id}': must match the pack-name grammar \
             (no path separators, '..', absolute paths, or NUL)"
        )))
    }
}

/// True when `pack_dir` is exactly `root/<pack_id>` (a single normal child).
fn is_direct_child(root: &Path, pack_dir: &Path, pack_id: &str) -> bool {
    match pack_dir.strip_prefix(root) {
        Ok(rest) => rest == Path::new(pack_id),
        Err(_) => false,
    }
}

/// Validate one raw entry and stamp it with the owning pack id.
fn normalise_question(entry: &Value, pack_id: &str, index: usize) -> Result<EvalQuestion> {
    let obj = entry.as_object().ok_or_else(|| {
        EvalError::QuestionLoad(format!(
            "question {index} in pack '{pack_id}' is not an object"
        ))
    })?;

    let id = obj
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            EvalError::QuestionLoad(format!(
                "question {index} in pack '{pack_id}' is missing a string 'id'"
            ))
        })?
        .to_string();

    let question = obj
        .get("question")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            EvalError::QuestionLoad(format!(
                "question '{id}' in pack '{pack_id}' is missing a string 'question'"
            ))
        })?
        .to_string();

    let reference_answer = obj
        .get("reference_answer")
        .and_then(Value::as_str)
        .map(str::to_string);
    let skill = obj.get("skill").and_then(Value::as_str).map(str::to_string);
    let metadata = match obj.get("metadata") {
        Some(Value::Object(map)) => Some(map.clone()),
        _ => None,
    };

    Ok(EvalQuestion {
        id,
        question,
        reference_answer,
        pack_id: pack_id.to_string(),
        skill,
        metadata,
    })
}
