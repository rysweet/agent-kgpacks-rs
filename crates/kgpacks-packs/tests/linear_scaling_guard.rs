//! WS5 — linear-scaling guard for the pack streaming loader.
//!
//! Structural (not timing) guard over the pack loaders:
//!
//! 1. Loading 2N records via [`kgpacks_packs::plan_load_statements`] issues at
//!    most a small constant factor (≤ 3×) more DB statements than loading N —
//!    i.e. the loader is linear, never O(N²).
//! 2. No edge-creation statement uses the O(N²) comma two-pattern
//!    `MATCH (a {..}), (b {..})`; edges are created with PK-indexed
//!    single-`MATCH` clauses. This covers **both** load paths: the M2
//!    [`plan_load_statements`] planner AND the WS6 pipelined CVE builder
//!    ([`kgpacks_packs::cve_build`]'s `load_record`), whose `HAS_ENTITY` and
//!    `--with-entity-relations` `ENTITY_RELATION` edges are the
//!    scale-sensitive path WS8 targets on the 343k-record CVE corpus.

use kgpacks_packs::{
    plan_load_statements, Article, Entity, PackContent, CREATE_ENTITY_RELATION_CYPHER,
    CREATE_HAS_ENTITY_CYPHER,
};

/// Synthetic content with `n` articles, `n` entities and `n` `HAS_ENTITY` edges.
fn content_with(n: usize) -> PackContent {
    let articles = (0..n)
        .map(|i| Article {
            title: format!("Article {i}"),
            category: "C".into(),
            word_count: i as i64,
            expansion_depth: 1,
        })
        .collect();
    let entities = (0..n)
        .map(|i| Entity {
            entity_id: format!("ent:{i}"),
            name: format!("Entity {i}"),
            type_: "T".into(),
            description: "d".into(),
        })
        .collect();
    let article_entities = (0..n)
        .map(|i| (format!("Article {i}"), format!("ent:{i}")))
        .collect();
    PackContent {
        articles,
        entities,
        article_entities,
    }
}

/// The comma two-pattern joins two node patterns with a comma: after removing
/// whitespace, a `)` (closing one node pattern) immediately followed by `,(`
/// (starting the next). Catches both `MATCH (a {..}), (b {..})` and the
/// property-less `MATCH (a:Article), (e:Entity)` cartesian shape.
fn has_comma_two_pattern(cypher: &str) -> bool {
    let compact: String = cypher.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains("),(")
}

#[test]
fn loader_statement_count_is_linear_in_records() {
    let n = 200usize;
    let stmts_n = plan_load_statements(&content_with(n)).len();
    let stmts_2n = plan_load_statements(&content_with(2 * n)).len();

    // Exact shape: a fixed schema prefix plus one statement per record.
    // (schema DDL) + articles + entities + edges = SCHEMA + 3N.
    assert_eq!(
        stmts_2n - stmts_n,
        3 * n,
        "each extra record => a constant #statements"
    );

    // The guard itself: doubling the records at most triples the statements.
    assert!(
        stmts_2n as f64 <= 3.0 * stmts_n as f64,
        "loading 2N ({stmts_2n}) must be <= 3x loading N ({stmts_n}) — non-linear regression"
    );
}

#[test]
fn no_planned_statement_uses_the_on2_comma_two_pattern() {
    for stmt in plan_load_statements(&content_with(8)) {
        assert!(
            !has_comma_two_pattern(&stmt.cypher),
            "planned statement uses the O(N^2) comma two-pattern: {}",
            stmt.cypher
        );
    }
}

#[test]
fn edge_creation_uses_pk_indexed_single_match() {
    // The HAS_ENTITY edge Cypher must locate each endpoint with its own single
    // MATCH (two MATCH clauses), never a comma two-pattern.
    assert!(!has_comma_two_pattern(CREATE_HAS_ENTITY_CYPHER));
    assert_eq!(
        CREATE_HAS_ENTITY_CYPHER.matches("MATCH ").count(),
        2,
        "expected two PK-indexed MATCH clauses"
    );
    assert!(CREATE_HAS_ENTITY_CYPHER.contains("CREATE (a)-[:HAS_ENTITY]->(e)"));

    // Every edge statement the planner emits is exactly this PK-indexed form.
    // (Filter on the exact edge Cypher so the `HAS_ENTITY` REL-table DDL, which
    // also mentions the label, is not counted.)
    let edge_statements: Vec<_> = plan_load_statements(&content_with(3))
        .into_iter()
        .filter(|s| s.cypher == CREATE_HAS_ENTITY_CYPHER)
        .collect();
    assert_eq!(edge_statements.len(), 3);
    for stmt in edge_statements {
        assert_eq!(stmt.cypher, CREATE_HAS_ENTITY_CYPHER);
    }
}

/// The WS6 pipelined CVE builder (`cve_build::load_record`) issues its edges from
/// two named constants rather than the planner: it reuses
/// [`CREATE_HAS_ENTITY_CYPHER`] for `HAS_ENTITY` and [`CREATE_ENTITY_RELATION_CYPHER`]
/// for the `--with-entity-relations` `ENTITY_RELATION` edge. Both must keep the
/// PK-indexed single-`MATCH` shape (never the O(N²) comma two-pattern), so this
/// pins them at their definition site — the same guarantee WS8 makes for the
/// scalable relation load, extended to the production CVE build path.
#[test]
fn cve_builder_edge_cyphers_use_pk_indexed_single_match() {
    for (label, cypher, edge) in [
        ("HAS_ENTITY", CREATE_HAS_ENTITY_CYPHER, "[:HAS_ENTITY]"),
        (
            "ENTITY_RELATION",
            CREATE_ENTITY_RELATION_CYPHER,
            "[:ENTITY_RELATION",
        ),
    ] {
        assert!(
            !has_comma_two_pattern(cypher),
            "{label} CVE-builder edge Cypher uses the O(N^2) comma two-pattern; \
             rewrite it with PK-indexed single-MATCH clauses:\n{cypher}"
        );
        assert_eq!(
            cypher.matches("MATCH ").count(),
            2,
            "{label} CVE-builder edge Cypher must use two PK-indexed MATCH clauses:\n{cypher}"
        );
        assert!(
            cypher.contains("CREATE ") && cypher.contains(edge),
            "{label} CVE-builder edge Cypher must create the {label} edge:\n{cypher}"
        );
    }
}
