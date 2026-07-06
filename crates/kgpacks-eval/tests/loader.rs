//! `DirQuestionLoader` behaviour: path confinement, normalisation and errors.

use std::fs;

use kgpacks_eval::{DirQuestionLoader, QuestionLoader};
use tempfile::tempdir;

fn write_pack(base: &std::path::Path, pack: &str, body: &str) {
    let dir = base.join(pack);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("eval_questions.json"), body).unwrap();
}

#[test]
fn loads_and_normalises_questions() {
    let base = tempdir().unwrap();
    write_pack(
        base.path(),
        "cve",
        r#"[
          { "id": "q1", "question": "What is CVE-2024-3094?", "reference_answer": "A backdoor.", "skill": "cve_lookup", "metadata": { "year": 2024 } },
          { "id": "q2", "question": "A concept?" }
        ]"#,
    );

    let questions = DirQuestionLoader::new(base.path()).load("cve").unwrap();
    assert_eq!(questions.len(), 2);
    assert_eq!(questions[0].id, "q1");
    assert_eq!(questions[0].pack_id, "cve");
    assert_eq!(
        questions[0].reference_answer.as_deref(),
        Some("A backdoor.")
    );
    assert_eq!(questions[0].skill.as_deref(), Some("cve_lookup"));
    assert!(questions[0].references_recent_cve());
    // Optional fields absent -> None; not a recent CVE.
    assert_eq!(questions[1].reference_answer, None);
    assert!(!questions[1].references_recent_cve());
}

#[test]
fn rejects_pack_ids_that_escape_the_base_directory() {
    let base = tempdir().unwrap();
    let loader = DirQuestionLoader::new(base.path());
    for bad in ["../secrets", "a/b", "..", "/etc", "a\\b", ""] {
        assert!(loader.load(bad).is_err(), "packId '{bad}' must be rejected");
    }
}

#[test]
fn errors_when_the_pack_file_is_missing() {
    let base = tempdir().unwrap();
    let err = DirQuestionLoader::new(base.path())
        .load("cve")
        .unwrap_err()
        .to_string();
    assert!(err.contains("failed to read eval questions"), "{err}");
}

#[test]
fn errors_on_non_array_json() {
    let base = tempdir().unwrap();
    write_pack(base.path(), "cve", r#"{ "not": "an array" }"#);
    let err = DirQuestionLoader::new(base.path())
        .load("cve")
        .unwrap_err()
        .to_string();
    assert!(err.contains("must be a JSON array"), "{err}");
}

#[test]
fn errors_when_an_entry_is_missing_its_id() {
    let base = tempdir().unwrap();
    write_pack(base.path(), "cve", r#"[ { "question": "no id here" } ]"#);
    let err = DirQuestionLoader::new(base.path())
        .load("cve")
        .unwrap_err()
        .to_string();
    assert!(err.contains("missing a string 'id'"), "{err}");
}
