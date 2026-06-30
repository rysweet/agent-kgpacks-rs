//! Verifies the three-signal hybrid scoring formula (vector + graph + keyword),
//! a direct mirror of `@kgpacks/query`'s `hybrid.test.ts`.
//!
//! To assert the exact arithmetic, retrieval is driven by a DETERMINISTIC
//! injected embedder and explicit one-hot section vectors, so cosine
//! similarities are exactly known and the only variability is the formula.
//!
//! Fixture (`FLOAT[768]` one-hot embeddings, `LINKS_TO` edge 1 → 2):
//!   Section 1 "Alpha Physics"  e0
//!   Section 2 "Beta Chemistry" e1  (LINKS_TO neighbor of section 1)
//!   Section 3 "Gamma Biology"  e2
//!   Section 4 "Delta History"  e3
//!
//! Query "Alpha quantum theory" with the injected embedder returning e0:
//!   vector:  section 1 similarity 1 → 0.5 * 1   = 0.50
//!   keyword: "Alpha" CONTAINS title 1          += 0.2 * 0.7 = 0.14
//!   graph:   seed 1 → neighbor 2               += 0.3 * 0.5 = 0.15
//!   ⇒ scores: {1: 0.64, 2: 0.15, 3: 0, 4: 0}

mod common;

use common::{create_vector_index, insert_int_section, link_int, one_hot, FixedQueryEmbedder};
use kgpacks_db::{Connection, Database};
use kgpacks_query::{HybridWeights, PackRetriever, RetrieveMode, RetrieveOptions, RetrieverConfig};

fn setup(conn: &Connection<'_>) {
    conn.load_extension("vector").expect("load vector ext");
    common::create_int_schema(conn);
    insert_int_section(conn, 1, "Alpha Physics", "Alpha content", &one_hot(0));
    insert_int_section(conn, 2, "Beta Chemistry", "Beta content", &one_hot(1));
    insert_int_section(conn, 3, "Gamma Biology", "Gamma content", &one_hot(2));
    insert_int_section(conn, 4, "Delta History", "Delta content", &one_hot(3));
    link_int(conn, 1, 2);
    create_vector_index(conn);
}

fn retriever<'c, 'd>(conn: &'c Connection<'d>) -> PackRetriever<'c, 'd, FixedQueryEmbedder> {
    PackRetriever::with_embedder(
        conn,
        FixedQueryEmbedder { vector: one_hot(0) },
        RetrieverConfig::default(),
    )
}

fn rank(results: &[kgpacks_query::RetrieverResult], id: &str) -> usize {
    results.iter().position(|r| r.id == id).expect("id present")
}

#[test]
fn combines_all_three_signals_and_ranks_by_the_weighted_sum() {
    let db = Database::in_memory().expect("db");
    let conn = db.connect().expect("conn");
    setup(&conn);

    let results = retriever(&conn)
        .retrieve(
            "Alpha quantum theory",
            &RetrieveOptions {
                k: Some(4),
                mode: RetrieveMode::Hybrid,
                weights: None,
            },
        )
        .expect("hybrid retrieve");

    // Section 1: vector 0.5 + keyword 0.14 = 0.64 (top).
    assert_eq!(results[0].id, "1");
    assert_eq!(results[0].content, "Alpha content");
    assert!(
        (results[0].score - 0.64).abs() < 1e-4,
        "score = {}",
        results[0].score
    );

    // Section 2: graph-only 0.15 — ranked second on the LINKS_TO signal alone
    // (its vector similarity is 0).
    assert_eq!(results[1].id, "2");
    let s2 = results.iter().find(|r| r.id == "2").unwrap().score;
    assert!((s2 - 0.15).abs() < 1e-4, "score = {s2}");

    // The graph signal lifts section 2 above the zero-scored sections 3 and 4.
    assert!(rank(&results, "2") < rank(&results, "3"));
    assert!(rank(&results, "2") < rank(&results, "4"));
}

#[test]
fn drops_the_keyword_signal_when_its_weight_is_zero() {
    let db = Database::in_memory().expect("db");
    let conn = db.connect().expect("conn");
    setup(&conn);

    let results = retriever(&conn)
        .retrieve(
            "Alpha quantum theory",
            &RetrieveOptions {
                k: Some(4),
                mode: RetrieveMode::Hybrid,
                weights: Some(HybridWeights {
                    vector: 0.5,
                    graph: 0.3,
                    keyword: 0.0,
                }),
            },
        )
        .expect("hybrid retrieve");

    // Without the +0.14 keyword boost, section 1 is purely its vector signal.
    assert_eq!(results[0].id, "1");
    assert!(
        (results[0].score - 0.5).abs() < 1e-4,
        "score = {}",
        results[0].score
    );
    assert_eq!(results[1].id, "2");
}

#[test]
fn still_answers_vector_mode_against_the_same_fixture() {
    let db = Database::in_memory().expect("db");
    let conn = db.connect().expect("conn");
    setup(&conn);

    let results = retriever(&conn)
        .retrieve(
            "anything",
            &RetrieveOptions {
                k: Some(2),
                ..Default::default()
            },
        )
        .expect("vector retrieve");

    assert_eq!(results[0].id, "1");
    assert!(
        (results[0].score - 1.0).abs() < 1e-4,
        "score = {}",
        results[0].score
    );
}
