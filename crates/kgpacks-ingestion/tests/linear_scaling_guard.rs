//! WS5 — linear-scaling guard for the ingestion loader.
//!
//! The ingestion pipeline creates edges from a handful of named Cypher
//! templates. This structural guard pins every one of them: none may use the
//! O(N²) comma two-pattern `MATCH (a ..), (b ..)` (whose cartesian shape pushes
//! edge creation toward quadratic work), and each must locate its endpoints
//! with PK-indexed single-`MATCH` clauses. Checking the exported constants
//! directly (rather than scanning source text) keeps the guard precise and
//! catches any regression at the definition site.

use kgpacks_ingestion::link_discovery::CREATE_LINK_CYPHER;
use kgpacks_ingestion::processor::{
    CREATE_ENTITY_RELATION_CYPHER, CREATE_HAS_ENTITY_CYPHER, CREATE_HAS_FACT_CYPHER,
    CREATE_IN_CATEGORY_CYPHER,
};

/// Every edge-creation Cypher template the ingestion loader issues.
fn edge_cyphers() -> [(&'static str, &'static str); 5] {
    [
        ("LINKS_TO", CREATE_LINK_CYPHER),
        ("IN_CATEGORY", CREATE_IN_CATEGORY_CYPHER),
        ("HAS_ENTITY", CREATE_HAS_ENTITY_CYPHER),
        ("HAS_FACT", CREATE_HAS_FACT_CYPHER),
        ("ENTITY_RELATION", CREATE_ENTITY_RELATION_CYPHER),
    ]
}

/// The comma two-pattern joins two node patterns with a comma: after removing
/// whitespace, a `)` (closing one node pattern) immediately followed by `,(`
/// (starting the next). This catches both `MATCH (a {..}), (b {..})` and the
/// property-less `MATCH (a:Article), (e:Entity)` cartesian shape.
fn has_comma_two_pattern(cypher: &str) -> bool {
    let compact: String = cypher.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains("),(")
}

#[test]
fn no_ingestion_edge_cypher_uses_the_on2_comma_two_pattern() {
    for (label, cypher) in edge_cyphers() {
        assert!(
            !has_comma_two_pattern(cypher),
            "{label} edge Cypher uses the O(N^2) comma two-pattern; \
             rewrite it with PK-indexed single-MATCH clauses:\n{cypher}"
        );
    }
}

#[test]
fn every_ingestion_edge_cypher_uses_pk_indexed_single_match() {
    for (label, cypher) in edge_cyphers() {
        // Two endpoints, each located by its own single MATCH.
        assert_eq!(
            cypher.matches("MATCH ").count(),
            2,
            "{label} edge Cypher must use two PK-indexed MATCH clauses:\n{cypher}"
        );
        assert!(
            cypher.contains("CREATE "),
            "{label} edge Cypher must create the edge:\n{cypher}"
        );
    }
}
