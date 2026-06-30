//! End-to-end vector retrieval over an in-memory LadybugDB pack (parity with
//! `@kgpacks/query`'s `vector.test.ts`).
//!
//! The reference suite drives the REAL BGE model to make a *semantic* ranking
//! claim ("plants → photosynthesis"). A transformer is neither hermetic nor fast
//! in CI, so this port instead injects a DETERMINISTIC embedder with known graded
//! vectors and asserts the same *retrieval-pipeline* contract the reference
//! checks: embed query → `QUERY_VECTOR_INDEX` → ranked `{id, score, content}`,
//! nearest first, scores in `[0, 1]` and non-increasing, and `top-k` honored.

mod common;

use common::{create_vector_index, insert_int_section, mix, one_hot, FixedQueryEmbedder, DIM};
use kgpacks_db::{Connection, Database};
use kgpacks_query::{PackRetriever, RetrieveOptions, RetrieverConfig};

/// Three sections with KNOWN graded cosine similarity to `one_hot(0)`:
///   id 1 — cosine 1.0  (identical vector)        → score 1.0
///   id 2 — cosine 0.6  (`0.6·e0 + 0.8·e1`)       → score 0.6
///   id 3 — cosine 0.0  (orthogonal `e2`)         → score 0.0
fn setup(conn: &Connection<'_>) {
    common::load_vector_ext(conn);
    common::create_int_schema(conn);
    insert_int_section(
        conn,
        1,
        "Photosynthesis",
        "Photosynthesis content",
        &one_hot(0),
    );
    insert_int_section(
        conn,
        2,
        "French Revolution",
        "Revolution content",
        &mix(0, 1, 0.6, 0.8),
    );
    insert_int_section(conn, 3, "Basketball", "Basketball content", &one_hot(2));
    create_vector_index(conn);
}

fn retriever<'c, 'd>(conn: &'c Connection<'d>) -> PackRetriever<'c, 'd, FixedQueryEmbedder> {
    PackRetriever::with_embedder(
        conn,
        FixedQueryEmbedder { vector: one_hot(0) },
        RetrieverConfig::default(),
    )
}

#[test]
fn returns_the_nearest_neighbor_first_with_valid_descending_scores() {
    assert_eq!(DIM, 768);
    let db = Database::in_memory().expect("db");
    let conn = db.connect().expect("conn");
    setup(&conn);

    let results = retriever(&conn)
        .retrieve(
            "any query — the injected embedder ignores the text",
            &RetrieveOptions {
                k: Some(3),
                ..Default::default()
            },
        )
        .expect("vector retrieve");

    assert_eq!(results.len(), 3);

    // Section 1 is the unambiguous top hit (cosine 1.0).
    assert_eq!(results[0].id, "1");
    assert!(results[0].content.contains("Photosynthesis"));

    // Scores are cosine similarities in [0, 1], ranked non-increasing.
    for r in &results {
        assert!(
            r.score >= 0.0 && r.score <= 1.0,
            "score out of range: {}",
            r.score
        );
    }
    for win in results.windows(2) {
        assert!(win[1].score <= win[0].score, "scores not non-increasing");
    }
    assert!(results[0].score > results[1].score);

    // The graded vectors yield the known similarities (within float tolerance).
    assert!(
        (results[0].score - 1.0).abs() < 1e-4,
        "score0 = {}",
        results[0].score
    );
    assert!(
        (results[1].score - 0.6).abs() < 1e-4,
        "score1 = {}",
        results[1].score
    );
    assert_eq!(results[1].id, "2");
}

#[test]
fn honors_top_k() {
    let db = Database::in_memory().expect("db");
    let conn = db.connect().expect("conn");
    setup(&conn);

    let results = retriever(&conn)
        .retrieve(
            "anything",
            &RetrieveOptions {
                k: Some(1),
                ..Default::default()
            },
        )
        .expect("vector retrieve");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "1");
}

#[test]
fn rejects_a_non_positive_k() {
    let db = Database::in_memory().expect("db");
    let conn = db.connect().expect("conn");
    setup(&conn);

    let err = retriever(&conn)
        .retrieve(
            "anything",
            &RetrieveOptions {
                k: Some(0),
                ..Default::default()
            },
        )
        .expect_err("k = 0 must be rejected");
    assert!(matches!(err, kgpacks_query::QueryError::InvalidArgument(_)));
}

#[test]
fn loads_the_vector_extension_on_a_fresh_read_connection() {
    // Build + index on disk with a writer connection (which loads the extension
    // itself), then reopen a FRESH database/connection that has NOT loaded any
    // extension and query through the retriever. This locks `ensure_extensions`:
    // if it were removed, `QUERY_VECTOR_INDEX` would not resolve on the reader.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("pack.lbug");

    {
        let db = Database::open(&path).expect("open writer db");
        let conn = db.connect().expect("writer conn");
        setup(&conn); // loads vector, creates schema + index, inserts rows
                      // writer db/conn drop here, flushing the pack (incl. the index) to disk.
    }

    let db = Database::open(&path).expect("reopen reader db");
    let conn = db.connect().expect("reader conn"); // no manual load_extension here

    let results = retriever(&conn)
        .retrieve(
            "anything",
            &RetrieveOptions {
                k: Some(1),
                ..Default::default()
            },
        )
        .expect("retrieve must load the vector extension on the read connection itself");

    assert_eq!(results[0].id, "1");
}
