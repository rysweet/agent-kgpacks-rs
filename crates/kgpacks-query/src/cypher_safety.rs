//! `kgpacks-query` — Cypher safety validation.
//!
//! Rust port of `@kgpacks/query`'s `cypher-safety.ts`, itself a strict-parity
//! port of the reference `KnowledgeGraphAgent._validate_cypher`. A read-only
//! allow-list / write-blocklist for Cypher that *fails closed*: a query is
//! rejected unless it provably matches the read-only contract.
//!
//! The CORE retrieval path ([`crate::PackRetriever::retrieve`]) does NOT route
//! user text into Cypher — it runs fixed, parameter-bound vector/graph queries —
//! so this is exported as a standalone guard for any caller that builds Cypher
//! from untrusted input (the M5 Cypher-RAG stage will use it).

use std::sync::OnceLock;

use regex::Regex;

use crate::constants::{CYPHER_ALLOWED_PREFIXES, CYPHER_BLOCKED_OPS};
use crate::errors::CypherValidationError;

/// Matches a double-quoted string literal (parity with the reference
/// module-level constant).
fn re_double_quoted() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#""[^"]*""#).expect("valid regex"))
}

/// Matches a single-quoted string literal.
fn re_single_quoted() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"'[^']*'"#).expect("valid regex"))
}

/// Matches a run of ASCII letters (a "bare token" for keyword scanning).
fn re_alpha_tokens() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[A-Za-z]+").expect("valid regex"))
}

/// Matches any bracketed variable-length path segment (contains `*`). The
/// reference name says "unbounded" but the pattern also matches bounded forms
/// like `[:LINKS_TO*1..3]`; strict parity rejects both. The character class is
/// spelled out (`[A-Za-z0-9_:]`) to match the JavaScript `[\w:]` (ASCII `\w`)
/// exactly rather than Rust's Unicode-aware `\w`.
fn re_variable_length_path() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[[A-Za-z0-9_:]*\*[^\]]*\]").expect("valid regex"))
}

/// Validates that `cypher` is a read-only query, returning `Err` otherwise.
///
/// The checks, in order (matching the reference exactly):
///  1. String literals are stripped so quoted keywords (e.g. `"DELETE ME"`)
///     never trip the blocklist.
///  2. The trimmed, upper-cased remainder must start with `MATCH` or `CALL`.
///  3. No bare alphabetic token may be a write/DDL keyword
///     (`CREATE`/`DELETE`/`DROP`/`SET`/`MERGE`/`REMOVE`/`DETACH`); the first hit
///     in token order wins.
///  4. The original query must contain no variable-length path (`[...*...]`).
///
/// Returns [`CypherValidationError`] naming the first failing check.
pub fn validate_cypher(cypher: &str) -> std::result::Result<(), CypherValidationError> {
    // 1. Strip string literals so quoted content can't trip the blocklist.
    //    Double-quoted first, then single-quoted on the result (reference order).
    let stripped = re_double_quoted().replace_all(cypher, "\"\"");
    let stripped = re_single_quoted().replace_all(&stripped, "''");

    // 2. Prefix check — must start with an allowed read keyword.
    let upper = stripped.trim().to_uppercase();
    if !CYPHER_ALLOWED_PREFIXES
        .iter()
        .any(|prefix| upper.starts_with(prefix))
    {
        return Err(CypherValidationError(
            "Cypher query must start with MATCH or CALL".to_string(),
        ));
    }

    // 3. Block dangerous write/DDL keywords (first hit wins, like the reference).
    for token in re_alpha_tokens().find_iter(&stripped) {
        let keyword = token.as_str().to_uppercase();
        if CYPHER_BLOCKED_OPS.contains(&keyword.as_str()) {
            return Err(CypherValidationError(format!(
                "Write operation rejected: {keyword}"
            )));
        }
    }

    // 4. Block variable-length paths (checked against the ORIGINAL query).
    if re_variable_length_path().is_match(cypher) {
        return Err(CypherValidationError(
            "Unbounded variable-length path detected in query".to_string(),
        ));
    }

    Ok(())
}
