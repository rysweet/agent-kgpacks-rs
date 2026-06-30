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
    common::load_vector_ext(conn);
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
    common::load_vector_ext(conn);
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

// ── Custom node table + vector index (config is actually interpolated) ───────

/// Build a NON-default schema (`Doc` node table, `vec_idx` index) so a retriever
/// configured with a custom [`RetrieverConfig`] is proven to interpolate those
/// names into all three issued queries (`QUERY_VECTOR_INDEX`, the `LINKS_TO`
/// traversal, and the title-keyword match) rather than the defaults.
fn setup_custom(conn: &Connection<'_>) {
    common::load_vector_ext(conn);
    conn.run(
        "CREATE NODE TABLE Doc(id INT64, title STRING, content STRING, \
         embedding FLOAT[768], PRIMARY KEY(id))",
    )
    .expect("create Doc table");
    conn.run("CREATE REL TABLE LINKS_TO(FROM Doc TO Doc)")
        .expect("create LINKS_TO table");
    for (id, title, emb) in [(1i64, "Alpha Doc", one_hot(0)), (2, "Beta Doc", one_hot(1))] {
        conn.run_params(
            "CREATE (:Doc {id: $id, title: $title, content: $content, embedding: $emb})",
            vec![
                ("id", Value::Int64(id)),
                ("title", Value::String(title.into())),
                ("content", Value::String(format!("{title} body"))),
                ("emb", float_array(&emb)),
            ],
        )
        .expect("insert Doc");
    }
    conn.run_params(
        "MATCH (a:Doc {id: $a}), (b:Doc {id: $b}) CREATE (a)-[:LINKS_TO]->(b)",
        vec![("a", Value::Int64(1)), ("b", Value::Int64(2))],
    )
    .expect("link docs");
    conn.run("CALL CREATE_VECTOR_INDEX('Doc', 'vec_idx', 'embedding', metric := 'cosine')")
        .expect("create custom-named vector index");
}

#[test]
fn uses_a_custom_node_table_and_vector_index() {
    let db = Database::in_memory().expect("db");
    let conn = db.connect().expect("conn");
    setup_custom(&conn);

    let retriever = PackRetriever::with_embedder(
        &conn,
        FixedQueryEmbedder { vector: one_hot(0) },
        RetrieverConfig {
            node_table: "Doc".to_string(),
            vector_index: "vec_idx".to_string(),
            stop_words: kgpacks_query::default_stop_words(),
        },
    );

    // All three signals must resolve against the custom names: doc 1 = vector 0.5
    // (QUERY_VECTOR_INDEX('Doc','vec_idx')) + keyword 0.14 ("alpha" CONTAINS its
    // title) = 0.64; doc 2 = graph 0.15 (LINKS_TO over Doc). A wrong table/index
    // name would error or return nothing.
    let results = retriever
        .retrieve(
            "alpha story",
            &RetrieveOptions {
                k: Some(2),
                mode: RetrieveMode::Hybrid,
                weights: None,
            },
        )
        .expect("hybrid retrieve over custom schema");

    assert_eq!(results[0].id, "1");
    assert!(
        (score_of(&results, "1") - 0.64).abs() < 1e-4,
        "doc1 = {}",
        score_of(&results, "1")
    );
    assert!(
        (score_of(&results, "2") - 0.15).abs() < 1e-4,
        "doc2 = {}",
        score_of(&results, "2")
    );
}

// ── Graph-seed cap: only the first MAX_GRAPH_SEEDS (3) scored nodes seed ──────

/// Five vector hits with strictly decreasing cosine (1.0, 0.9, 0.8, 0.7 to
/// `one_hot(0)`), so insertion order is deterministic, plus orthogonal nodes
/// reached only via `LINKS_TO`. Edges `1 -> 6` (1st seed), `3 -> 5` (3rd seed),
/// and `4 -> 7` (4th node, NOT a seed). Nodes 6 and 5 must receive a graph boost
/// (their sources are among the first three seeds) while node 7 must not — which
/// pins the seed cap at EXACTLY three: the test would fail if the cap were 2
/// (node 5 unboosted) or 4 (node 7 boosted).
fn setup_seed_cap(conn: &Connection<'_>) {
    common::load_vector_ext(conn);
    common::create_int_schema(conn);
    // sqrt(1 - cos^2) chosen so each mix vector is unit-norm with the stated cosine.
    common::insert_int_section(conn, 1, "N1", "c", &one_hot(0)); // cos 1.0
    common::insert_int_section(conn, 2, "N2", "c", &mix(0, 1, 0.9, 0.435_889_9)); // cos 0.9
    common::insert_int_section(conn, 3, "N3", "c", &mix(0, 2, 0.8, 0.6)); // cos 0.8
    common::insert_int_section(conn, 4, "N4", "c", &mix(0, 3, 0.7, 0.714_142_8)); // cos 0.7
    common::insert_int_section(conn, 5, "N5", "c", &one_hot(8)); // neighbor of seed 3
    common::insert_int_section(conn, 6, "N6", "c", &one_hot(9)); // neighbor of seed 1
    common::insert_int_section(conn, 7, "N7", "c", &one_hot(10)); // neighbor of non-seed 4
    common::link_int(conn, 1, 6);
    common::link_int(conn, 3, 5);
    common::link_int(conn, 4, 7);
    common::create_vector_index(conn);
}

#[test]
fn graph_seeds_are_capped_at_the_first_three_scored_nodes() {
    let db = Database::in_memory().expect("db");
    let conn = db.connect().expect("conn");
    setup_seed_cap(&conn);

    let retriever = PackRetriever::with_embedder(
        &conn,
        FixedQueryEmbedder { vector: one_hot(0) },
        RetrieverConfig::default(),
    );
    // "qqqq" matches no title, so the keyword signal is silent and only vector +
    // graph contribute.
    let results = retriever
        .retrieve(
            "qqqq",
            &RetrieveOptions {
                k: Some(10),
                mode: RetrieveMode::Hybrid,
                weights: None,
            },
        )
        .expect("hybrid retrieve");

    // Node 6 (neighbor of seed node 1) gets the graph boost.
    assert!(
        (score_of(&results, "6") - 0.15).abs() < 1e-4,
        "n6 = {}",
        score_of(&results, "6")
    );
    // Node 5 (neighbor of seed node 3, the THIRD scored node) also gets boosted —
    // proving the 3rd node IS a seed (the cap is not 2).
    assert!(
        (score_of(&results, "5") - 0.15).abs() < 1e-4,
        "n5 = {}",
        score_of(&results, "5")
    );
    // Node 7 (neighbor of node 4, the FOURTH scored node) gets nothing — node 4
    // is past the 3-seed cap, so its edges are never traversed (the cap is not 4).
    assert!(
        score_of(&results, "7").abs() < 1e-4,
        "n7 = {}",
        score_of(&results, "7")
    );
}
