//! The combined retrieval query path: all three signals (vector cosine, `LINKS_TO`
//! graph proximity, and title-keyword / full-text) isolated AND summed in a
//! single hybrid ranking — plus retrieval over the RS pack's STRING-keyed
//! `Section` schema (the reference fixtures use `INT64` ids; the published RS
//! pack keys `Section` by a `STRING` id, and the retriever must handle both).

mod common;

use common::{float_array, mix, one_hot, FixedQueryEmbedder};
use kgpacks_db::{Connection, Database, Value};
use kgpacks_query::{
    PackRetriever, RetrieveMode, RetrieveOptions, RetrieverConfig, RetrieverResult,
};

fn score_of(results: &[RetrieverResult], id: &str) -> f64 {
    results
        .iter()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("id {id} present in {results:?}"))
        .score
}

fn rank(results: &[RetrieverResult], id: &str) -> usize {
    results.iter().position(|r| r.id == id).expect("id present")
}

/// Fixture isolating each signal AND combining all three on one node.
///
/// Query embedder returns `e0`; query text is `"elephant giraffe"`.
///   id 1 "Solo Anchor"     e0                       → vector 0.5 ; LINKS_TO 2 & 4
///   id 2 "Elephant Park"   0.6·e0+0.8·e1 (cos 0.6)  → vector 0.3 + graph 0.15 + keyword 0.14 = 0.59
///   id 3 "Giraffe Plains"  e4 (orthogonal)          → keyword 0.14 (FTS-only)
///   id 4 "Hidden Neighbor" e5 (orthogonal)          → graph 0.15 (graph-only)
fn setup_int(conn: &Connection<'_>) {
    conn.load_extension("vector").expect("load vector ext");
    common::create_int_schema(conn);
    common::insert_int_section(conn, 1, "Solo Anchor", "anchor", &one_hot(0));
    common::insert_int_section(conn, 2, "Elephant Park", "park", &mix(0, 1, 0.6, 0.8));
    common::insert_int_section(conn, 3, "Giraffe Plains", "plains", &one_hot(4));
    common::insert_int_section(conn, 4, "Hidden Neighbor", "hidden", &one_hot(5));
    common::link_int(conn, 1, 2);
    common::link_int(conn, 1, 4);
    common::create_vector_index(conn);
}

#[test]
fn isolates_and_sums_vector_graph_and_keyword_signals() {
    let db = Database::in_memory().expect("db");
    let conn = db.connect().expect("conn");
    setup_int(&conn);

    let retriever = PackRetriever::with_embedder(
        &conn,
        FixedQueryEmbedder { vector: one_hot(0) },
        RetrieverConfig::default(),
    );
    let results = retriever
        .retrieve(
            "elephant giraffe",
            &RetrieveOptions {
                k: Some(10),
                mode: RetrieveMode::Hybrid,
                weights: None,
            },
        )
        .expect("hybrid retrieve");

    // Combined node: all three signals summed (0.3 + 0.15 + 0.14 = 0.59) — top.
    assert_eq!(results[0].id, "2");
    assert!(
        (score_of(&results, "2") - 0.59).abs() < 1e-4,
        "id2 = {}",
        score_of(&results, "2")
    );

    // Vector-only isolation (0.5).
    assert!(
        (score_of(&results, "1") - 0.50).abs() < 1e-4,
        "id1 = {}",
        score_of(&results, "1")
    );
    // Graph-only isolation (0.15).
    assert!(
        (score_of(&results, "4") - 0.15).abs() < 1e-4,
        "id4 = {}",
        score_of(&results, "4")
    );
    // Keyword / full-text-only isolation (0.14).
    assert!(
        (score_of(&results, "3") - 0.14).abs() < 1e-4,
        "id3 = {}",
        score_of(&results, "3")
    );

    // Combined ranking: id2 (all three) > id1 (vector) > id4 (graph) > id3 (keyword).
    assert!(rank(&results, "2") < rank(&results, "1"));
    assert!(rank(&results, "1") < rank(&results, "4"));
    assert!(rank(&results, "4") < rank(&results, "3"));
}

// ── RS pack STRING-keyed Section schema ─────────────────────────────────────

/// Build the published-pack `Section` shape (`id STRING` PK) with the M4
/// embedding column + cosine index and a `LINKS_TO` edge, mirroring
/// `kgpacks-packs`' `Section(id STRING, title, content, …)` plus the embedding
/// column M4 adds for retrieval.
fn setup_string(conn: &Connection<'_>) {
    conn.load_extension("vector").expect("load vector ext");
    conn.run(
        "CREATE NODE TABLE Section(id STRING, title STRING, content STRING, \
         embedding FLOAT[768], PRIMARY KEY(id))",
    )
    .expect("create Section table");
    conn.run("CREATE REL TABLE LINKS_TO(FROM Section TO Section)")
        .expect("create LINKS_TO table");

    for (id, title, content, emb) in [
        ("sec-alpha", "Alpha Topic", "alpha body", one_hot(0)),
        ("sec-beta", "Beta Topic", "beta body", one_hot(1)),
    ] {
        conn.run_params(
            "CREATE (:Section {id: $id, title: $title, content: $content, embedding: $emb})",
            vec![
                ("id", Value::String(id.into())),
                ("title", Value::String(title.into())),
                ("content", Value::String(content.into())),
                ("emb", float_array(&emb)),
            ],
        )
        .expect("insert string-keyed Section");
    }
    conn.run_params(
        "MATCH (a:Section {id: $a}), (b:Section {id: $b}) CREATE (a)-[:LINKS_TO]->(b)",
        vec![
            ("a", Value::String("sec-alpha".into())),
            ("b", Value::String("sec-beta".into())),
        ],
    )
    .expect("link string-keyed sections");
    conn.run(
        "CALL CREATE_VECTOR_INDEX('Section', 'embedding_idx', 'embedding', metric := 'cosine')",
    )
    .expect("create vector index");
}

#[test]
fn retrieves_over_a_string_keyed_pack_schema() {
    let db = Database::in_memory().expect("db");
    let conn = db.connect().expect("conn");
    setup_string(&conn);

    let retriever = PackRetriever::with_embedder(
        &conn,
        FixedQueryEmbedder { vector: one_hot(0) },
        RetrieverConfig::default(),
    );

    // Vector mode: STRING id passes through unchanged (to_id_string).
    let vres = retriever
        .retrieve(
            "alpha",
            &RetrieveOptions {
                k: Some(2),
                ..Default::default()
            },
        )
        .expect("vector retrieve");
    assert_eq!(vres[0].id, "sec-alpha");
    assert!((vres[0].score - 1.0).abs() < 1e-4);

    // Hybrid mode over STRING ids: alpha (vector 0.5 + keyword 0.14) tops beta
    // (graph-only 0.15) — proving graph + vector + FTS all key correctly on
    // STRING primary keys.
    let hres = retriever
        .retrieve(
            "alpha story",
            &RetrieveOptions {
                k: Some(2),
                mode: RetrieveMode::Hybrid,
                weights: None,
            },
        )
        .expect("hybrid retrieve");
    assert_eq!(hres[0].id, "sec-alpha");
    assert!(
        (score_of(&hres, "sec-alpha") - 0.64).abs() < 1e-4,
        "alpha = {}",
        score_of(&hres, "sec-alpha")
    );
    assert!(
        (score_of(&hres, "sec-beta") - 0.15).abs() < 1e-4,
        "beta = {}",
        score_of(&hres, "sec-beta")
    );
}
