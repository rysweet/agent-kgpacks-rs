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
    common::load_vector_ext(conn);
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

/// Fixture for the keyword-extraction parity test: six sections whose titles each
/// contain exactly one candidate query term, NO `LINKS_TO` edges, and embeddings
/// orthogonal to the query vector (`one_hot(100)`) so every vector score is 0 —
/// isolating the keyword signal so only the kept keywords can contribute.
fn setup_keywords(conn: &Connection<'_>) {
    common::load_vector_ext(conn);
    common::create_int_schema(conn);
    insert_int_section(conn, 1, "Photosynthesis", "c", &one_hot(0));
    insert_int_section(conn, 2, "Plants", "c", &one_hot(1));
    insert_int_section(conn, 3, "Relates", "c", &one_hot(2));
    insert_int_section(conn, 4, "Those Days", "c", &one_hot(3)); // "those" is a stop word
    insert_int_section(conn, 5, "Sunlight", "c", &one_hot(4)); // truncated by take(3)
    insert_int_section(conn, 6, "Cat Facts", "c", &one_hot(5)); // "cat" too short (len 3)
    create_vector_index(conn);
}

#[test]
fn extracts_at_most_three_significant_keywords_dropping_short_and_stop_words() {
    // Mirrors the reference `retriever.test.ts` keyword-extraction case. Query
    // candidate terms in order: cat (len 3, dropped), those (stop word, dropped),
    // photosynthesis, plants, relates, sunlight. After filtering, the kept terms
    // are [photosynthesis, plants, relates, sunlight]; `take(3)` keeps the first
    // three, so ONLY photosynthesis/plants/relates contribute a keyword boost.
    let db = Database::in_memory().expect("db");
    let conn = db.connect().expect("conn");
    setup_keywords(&conn);

    let retriever = PackRetriever::with_embedder(
        &conn,
        FixedQueryEmbedder {
            vector: one_hot(100), // orthogonal to all section embeddings
        },
        RetrieverConfig::default(),
    );
    let results = retriever
        .retrieve(
            "cat those photosynthesis plants relates sunlight",
            &RetrieveOptions {
                k: Some(10),
                mode: RetrieveMode::Hybrid,
                weights: None,
            },
        )
        .expect("hybrid retrieve");

    let score = |id: &str| {
        results
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.score)
            .unwrap_or(0.0)
    };

    // Kept keywords (first three significant terms) each add keyword 0.2*0.7 = 0.14.
    assert!(
        (score("1") - 0.14).abs() < 1e-4,
        "photosynthesis = {}",
        score("1")
    );
    assert!((score("2") - 0.14).abs() < 1e-4, "plants = {}", score("2"));
    assert!((score("3") - 0.14).abs() < 1e-4, "relates = {}", score("3"));

    // Dropped terms contribute nothing:
    //   id4 "those"    — dropped as a stop word (length > 3 but in the stop set),
    //   id5 "sunlight" — dropped by the take(3) truncation,
    //   id6 "cat"      — dropped for being too short (length not > 3).
    assert!(
        score("4").abs() < 1e-4,
        "those (stop word) = {}",
        score("4")
    );
    assert!(
        score("5").abs() < 1e-4,
        "sunlight (truncated) = {}",
        score("5")
    );
    assert!(score("6").abs() < 1e-4, "cat (too short) = {}", score("6"));
}

#[test]
fn embeds_the_query_exactly_once_per_retrieve() {
    let db = Database::in_memory().expect("db");
    let conn = db.connect().expect("conn");
    setup(&conn);

    let calls = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let retriever = PackRetriever::with_embedder(
        &conn,
        common::CountingEmbedder {
            vector: one_hot(0),
            calls: calls.clone(),
        },
        RetrieverConfig::default(),
    );

    retriever
        .retrieve(
            "alpha",
            &RetrieveOptions {
                k: Some(2),
                ..Default::default()
            },
        )
        .expect("vector retrieve");
    assert_eq!(calls.get(), 1, "vector mode embeds the query exactly once");

    retriever
        .retrieve(
            "alpha",
            &RetrieveOptions {
                k: Some(2),
                mode: RetrieveMode::Hybrid,
                weights: None,
            },
        )
        .expect("hybrid retrieve");
    assert_eq!(
        calls.get(),
        2,
        "hybrid mode also embeds the query exactly once"
    );
}
