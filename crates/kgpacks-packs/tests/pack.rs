//! The M2 acceptance gate: pack **build → load round-trip over the graph store**.
//!
//! Builds a pack (manifest + LadybugDB graph store) from in-memory content, then
//! loads it back and proves the nodes, relationships and manifest survive the
//! round-trip over LadybugDB.

use std::collections::BTreeMap;

use kgpacks_packs::{
    build_pack, load_pack, Article, Entity, PackContent, PackManifest, GRAPH_STORE_FILENAME,
    MANIFEST_FILENAME, NODE_TABLE_DDL, REL_TABLE_DDL, SCHEMA,
};

fn sample_content() -> PackContent {
    PackContent {
        articles: vec![
            Article {
                title: "Rust (programming language)".into(),
                category: "Programming".into(),
                word_count: 1200,
                expansion_depth: 1,
            },
            Article {
                title: "Ownership".into(),
                category: "Programming".into(),
                word_count: 640,
                expansion_depth: 2,
            },
        ],
        entities: vec![
            Entity {
                entity_id: "ent:borrow-checker".into(),
                name: "Borrow checker".into(),
                type_: "Concept".into(),
                description: "Static analyzer enforcing ownership rules.".into(),
            },
            Entity {
                entity_id: "ent:lifetime".into(),
                name: "Lifetime".into(),
                type_: "Concept".into(),
                description: "A scope for which a reference is valid.".into(),
            },
        ],
        article_entities: vec![
            (
                "Rust (programming language)".into(),
                "ent:borrow-checker".into(),
            ),
            ("Ownership".into(), "ent:borrow-checker".into()),
            ("Ownership".into(), "ent:lifetime".into()),
        ],
    }
}

fn manifest() -> PackManifest {
    PackManifest::new("rust-expert", "1.0.0")
}

#[test]
fn schema_orders_node_tables_before_relationship_tables() {
    assert_eq!(SCHEMA.len(), NODE_TABLE_DDL.len() + REL_TABLE_DDL.len());
    assert_eq!(&SCHEMA[..NODE_TABLE_DDL.len()], &NODE_TABLE_DDL[..]);
    assert_eq!(&SCHEMA[NODE_TABLE_DDL.len()..], &REL_TABLE_DDL[..]);
}

#[test]
fn build_pack_writes_a_self_contained_pack() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pack_dir = dir.path().join("rust-expert");

    let built = build_pack(&pack_dir, &manifest(), &sample_content()).expect("build");
    assert_eq!(built.name, "rust-expert");
    assert_eq!(built.version, "1.0.0");

    // The pack directory holds the manifest and a single self-contained store.
    assert!(pack_dir.join(MANIFEST_FILENAME).is_file());
    assert!(pack_dir.join(GRAPH_STORE_FILENAME).is_file());
    let wal = pack_dir.join(format!("{GRAPH_STORE_FILENAME}.wal"));
    assert!(
        !wal.exists(),
        "expected no WAL sidecar at {}",
        wal.display()
    );

    // graph_stats is populated from the materialized content.
    let expected: BTreeMap<String, f64> = [
        ("articles".to_string(), 2.0),
        ("entities".to_string(), 2.0),
        ("relationships".to_string(), 3.0),
    ]
    .into_iter()
    .collect();
    assert_eq!(built.manifest.graph_stats, Some(expected));
}

#[test]
fn build_then_load_round_trips_nodes_relationships_and_manifest_over_the_graph_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pack_dir = dir.path().join("rust-expert");
    let content = sample_content();

    let built = build_pack(&pack_dir, &manifest(), &content).expect("build");

    // Load the pack back purely from disk.
    let loaded = load_pack(&pack_dir).expect("load");
    assert_eq!(loaded.manifest().name, "rust-expert");
    assert_eq!(loaded.manifest().version, "1.0.0");
    // The on-disk manifest carries the same graph_stats the build wrote.
    assert_eq!(loaded.manifest().graph_stats, built.manifest.graph_stats);

    // Live counts read back from the LadybugDB store match the built content.
    let stats = loaded.graph_stats().expect("graph stats");
    assert_eq!(stats["articles"], content.articles.len() as f64);
    assert_eq!(stats["entities"], content.entities.len() as f64);
    assert_eq!(
        stats["relationships"],
        content.article_entities.len() as f64
    );

    // And the manifest's recorded graph_stats agree with the live store.
    assert_eq!(loaded.manifest().graph_stats.as_ref(), Some(&stats));

    // Specific node data survives the round-trip (queried via Cypher).
    let conn = loaded.connect().expect("connect");
    let rows = conn
        .run("MATCH (a:Article) RETURN a.title AS title ORDER BY a.title")
        .expect("query articles");
    let titles: Vec<String> = rows
        .iter()
        .map(|row| match &row["title"] {
            kgpacks_db::Value::String(s) => s.clone(),
            other => panic!("expected a string title, got {other:?}"),
        })
        .collect();
    assert_eq!(titles, vec!["Ownership", "Rust (programming language)"]);

    // A specific relationship traversal returns the linked entity.
    let rows = conn
        .run_params(
            "MATCH (a:Article {title: $title})-[:HAS_ENTITY]->(e:Entity) \
             RETURN e.entity_id AS id ORDER BY e.entity_id",
            vec![("title", kgpacks_db::Value::String("Ownership".into()))],
        )
        .expect("query relationships");
    let entity_ids: Vec<String> = rows
        .iter()
        .map(|row| match &row["id"] {
            kgpacks_db::Value::String(s) => s.clone(),
            other => panic!("expected a string id, got {other:?}"),
        })
        .collect();
    assert_eq!(entity_ids, vec!["ent:borrow-checker", "ent:lifetime"]);
}

#[test]
fn build_pack_refuses_to_overwrite_an_existing_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pack_dir = dir.path().join("rust-expert");
    build_pack(&pack_dir, &manifest(), &sample_content()).expect("first build");
    assert!(
        build_pack(&pack_dir, &manifest(), &sample_content()).is_err(),
        "second build into the same dir must fail"
    );
}

#[test]
fn load_pack_errors_when_there_is_no_graph_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A directory with a manifest but no graph store is not a loadable pack.
    let pack_dir = dir.path().join("empty");
    std::fs::create_dir_all(&pack_dir).expect("mkdir");
    kgpacks_packs::save_manifest(kgpacks_packs::manifest_path_in(&pack_dir), &manifest())
        .expect("save manifest");
    assert!(load_pack(&pack_dir).is_err());
}
