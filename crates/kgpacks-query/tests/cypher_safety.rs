//! Parity + adversarial tests for `validate_cypher` (read-only Cypher allow-list).
//!
//! The "allowed" and "rejected" base cases are ported from the reference security
//! suite (`@kgpacks/query`'s `cypher-safety.test.ts`, itself ported from the
//! Python `test_validate_cypher.py`). Additional adversarial negatives cover the
//! remaining blocked keywords, stacked-statement injection, case/comment evasion,
//! and variable-length path forms.

use kgpacks_query::{validate_cypher, CypherValidationError, QueryError};

fn expect_err(cypher: &str) -> CypherValidationError {
    validate_cypher(cypher).expect_err("expected a CypherValidationError")
}

// ── Allowed read-only queries (parity) ──────────────────────────────────────

#[test]
fn allows_a_match_query() {
    assert!(validate_cypher("MATCH (a:Article) RETURN a LIMIT 10").is_ok());
}

#[test]
fn allows_a_call_query_vector_index_query() {
    assert!(validate_cypher(
        "CALL QUERY_VECTOR_INDEX('Section', 'embedding_idx', $query, 10) \
         RETURN node.title, node.content, score"
    )
    .is_ok());
}

#[test]
fn allows_a_relationship_traversal_with_limit() {
    assert!(validate_cypher(
        "MATCH (a:Article)-[:HAS_SECTION]->(s:Section) \
         RETURN a.title, s.heading, s.content LIMIT 25"
    )
    .is_ok());
}

#[test]
fn ignores_blocked_keywords_inside_double_quoted_literals() {
    assert!(validate_cypher(r#"MATCH (a:Article) WHERE a.name = "DELETE ME" RETURN a"#).is_ok());
}

#[test]
fn ignores_blocked_keywords_inside_single_quoted_literals() {
    assert!(validate_cypher("MATCH (a:Article) WHERE a.note = 'CREATE later' RETURN a").is_ok());
}

// ── Blocked write/DDL keywords (parity) ─────────────────────────────────────

#[test]
fn rejects_create() {
    let err = expect_err("MATCH (a:Article) CREATE (b:Article {title: 'hack'})");
    assert!(err.0.contains("Write operation rejected"));
    assert!(err.0.contains("CREATE"));
}

#[test]
fn rejects_delete() {
    let err = expect_err("MATCH (a:Article) DELETE a");
    assert!(err.0.contains("Write operation rejected") && err.0.contains("DELETE"));
}

#[test]
fn rejects_drop() {
    let err = expect_err("MATCH (a:Article) DROP a");
    assert!(err.0.contains("Write operation rejected") && err.0.contains("DROP"));
}

#[test]
fn rejects_set() {
    let err = expect_err("MATCH (a:Article) SET a.title = 'pwned'");
    assert!(err.0.contains("Write operation rejected") && err.0.contains("SET"));
}

// ── Prefix and path validation (parity) ─────────────────────────────────────

#[test]
fn rejects_a_non_match_call_prefix() {
    let err = expect_err("RETURN 1 AS one");
    assert!(err.0.contains("must start with MATCH"));
}

#[test]
fn rejects_an_unbounded_variable_length_path() {
    let err = expect_err("MATCH (a)-[:LINKS_TO*]->(b) RETURN b LIMIT 10");
    assert!(err.0.contains("Unbounded variable-length path"));
}

// ── Adversarial negatives ───────────────────────────────────────────────────

#[test]
fn rejects_remaining_blocked_keywords() {
    assert!(expect_err("MATCH (a) MERGE (b:Article {title: 'x'})")
        .0
        .contains("MERGE"));
    assert!(expect_err("MATCH (a:Article) REMOVE a.title")
        .0
        .contains("REMOVE"));
    // DETACH precedes DELETE in token order, so it is the reported keyword.
    assert!(expect_err("MATCH (a:Article) DETACH DELETE a")
        .0
        .contains("DETACH"));
}

#[test]
fn rejects_stacked_statement_injection() {
    assert!(expect_err("MATCH (a) RETURN a; DROP TABLE Article")
        .0
        .contains("DROP"));
}

#[test]
fn rejects_case_evasion_of_a_blocked_keyword() {
    assert!(expect_err("mAtCh (a) cReAtE (b:Article) RETURN a")
        .0
        .contains("CREATE"));
}

#[test]
fn rejects_a_blocked_keyword_smuggled_in_a_line_comment() {
    assert!(expect_err("MATCH (a) //DELETE\nRETURN a")
        .0
        .contains("DELETE"));
}

#[test]
fn rejects_bounded_variable_length_paths_too() {
    // The reference regex matches any `[...*...]`, so bounded forms are also
    // rejected — failing closed is the intended, parity-faithful behavior.
    assert!(expect_err("MATCH (a)-[:LINKS_TO*1..3]->(b) RETURN b")
        .0
        .contains("Unbounded variable-length path"));
}

#[test]
fn cypher_validation_error_converts_into_query_error() {
    // Mirrors the TS `CypherValidationError extends QueryError`: the specific
    // error participates in the crate-wide taxonomy via `From`.
    let err = expect_err("MATCH (a:Article) DELETE a");
    let as_query: QueryError = err.into();
    assert!(matches!(as_query, QueryError::CypherValidation(_)));
}
