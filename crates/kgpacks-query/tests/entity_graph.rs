//! Integration tests for `entity_graph` (port of the reference
//! `entity-graph.test.ts`): co-occurrence depth 1/2, type filter, auto-mode
//! selection, relation traversal, deterministic ordering, depth validation, and
//! the unknown-seed error — over a real in-memory LadybugDB.

use kgpacks_db::{Connection, Database, Value};
use kgpacks_query::{
    entity_graph, EntityGraphMode, EntityGraphOptions, QueryError, ResolvedEntityGraphMode,
};

/// Minimal Entity/Article/HAS_ENTITY/ENTITY_RELATION schema (a subset of the
/// ingestion working store, created inline so the query crate stays free of an
/// ingestion dep).
fn schema(conn: &Connection<'_>) {
    conn.run("CREATE NODE TABLE Article(title STRING, PRIMARY KEY(title))")
        .unwrap();
    conn.run(
        "CREATE NODE TABLE Entity(entity_id STRING, name STRING, type STRING, \
         description STRING, PRIMARY KEY(entity_id))",
    )
    .unwrap();
    conn.run("CREATE REL TABLE HAS_ENTITY(FROM Article TO Entity)")
        .unwrap();
    conn.run(
        "CREATE REL TABLE ENTITY_RELATION(FROM Entity TO Entity, relation STRING, context STRING)",
    )
    .unwrap();
}

fn entity(conn: &Connection<'_>, id: &str, name: &str, type_: &str) {
    conn.run_params(
        "CREATE (:Entity {entity_id: $id, name: $name, type: $type, description: ''})",
        vec![
            ("id", Value::String(id.into())),
            ("name", Value::String(name.into())),
            ("type", Value::String(type_.into())),
        ],
    )
    .unwrap();
}

fn article_with(conn: &Connection<'_>, title: &str, entity_ids: &[&str]) {
    conn.run_params(
        "CREATE (:Article {title: $title})",
        vec![("title", Value::String(title.into()))],
    )
    .unwrap();
    for id in entity_ids {
        conn.run_params(
            "MATCH (a:Article {title: $title}), (e:Entity {entity_id: $id}) \
             CREATE (a)-[:HAS_ENTITY]->(e)",
            vec![
                ("title", Value::String(title.into())),
                ("id", Value::String((*id).into())),
            ],
        )
        .unwrap();
    }
}

fn relation(conn: &Connection<'_>, source: &str, target: &str, rel: &str) {
    conn.run_params(
        "MATCH (a:Entity {entity_id: $s}), (b:Entity {entity_id: $t}) \
         CREATE (a)-[:ENTITY_RELATION {relation: $rel, context: ''}]->(b)",
        vec![
            ("s", Value::String(source.into())),
            ("t", Value::String(target.into())),
            ("rel", Value::String(rel.into())),
        ],
    )
    .unwrap();
}

/// Co-occurrence fixture: e1-e2 share A1, e2-e3 share A2. No ENTITY_RELATION.
fn cooccurrence_db() -> Database {
    let db = Database::in_memory().unwrap();
    {
        let conn = db.connect().unwrap();
        schema(&conn);
        entity(&conn, "e1", "Alpha", "person");
        entity(&conn, "e2", "Bravo", "person");
        entity(&conn, "e3", "Charlie", "person");
        article_with(&conn, "A1", &["e1", "e2"]);
        article_with(&conn, "A2", &["e2", "e3"]);
    }
    db
}

#[test]
fn cooccurrence_depth_1_returns_seed_and_direct_neighbors() {
    let db = cooccurrence_db();
    let conn = db.connect().unwrap();
    let result = entity_graph(&conn, &EntityGraphOptions::new("e1")).unwrap();

    assert_eq!(result.mode, ResolvedEntityGraphMode::CoOccurrence);
    assert_eq!(result.seed, "e1");
    // Seed (depth 0) + one direct co-occurrence neighbor (depth 1).
    let ids: Vec<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, vec!["e1", "e2"]);
    assert_eq!(result.nodes[0].depth, 0);
    assert_eq!(result.nodes[1].depth, 1);
    // e2 appears in A1 and A2 → articles_count == 2.
    assert_eq!(result.nodes[1].articles_count, 2);
    // One co-occurrence edge between the two selected nodes, weight = shared count.
    assert_eq!(result.total_edges, 1);
    let edge = &result.edges[0];
    assert_eq!(edge.relation.as_deref(), Some("co_occurs"));
    assert_eq!(edge.weight, 1);
}

#[test]
fn cooccurrence_depth_2_expands_two_hops_in_order() {
    let db = cooccurrence_db();
    let conn = db.connect().unwrap();
    let mut options = EntityGraphOptions::new("e1");
    options.depth = Some(2);
    let result = entity_graph(&conn, &options).unwrap();

    // Deterministic order: depth ASC, name ASC.
    let ids: Vec<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, vec!["e1", "e2", "e3"]);
    assert_eq!(result.nodes[2].depth, 2);
    // Two co-occurrence edges among the three nodes: e1-e2 (A1) and e2-e3 (A2).
    assert_eq!(result.total_edges, 2);
    assert!(result
        .edges
        .iter()
        .all(|e| e.relation.as_deref() == Some("co_occurs")));
}

#[test]
fn type_filter_restricts_neighbors_but_not_the_seed() {
    let db = Database::in_memory().unwrap();
    {
        let conn = db.connect().unwrap();
        schema(&conn);
        entity(&conn, "e1", "Alpha", "concept"); // seed, a concept
        entity(&conn, "e2", "Bravo", "person");
        entity(&conn, "e3", "Charlie", "place");
        article_with(&conn, "A1", &["e1", "e2", "e3"]);
    }
    let conn = db.connect().unwrap();
    let mut options = EntityGraphOptions::new("e1");
    options.type_filter = Some("person".to_string());
    let result = entity_graph(&conn, &options).unwrap();

    // Seed kept regardless of type; only the `person` neighbor (e2) is included.
    let ids: Vec<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, vec!["e1", "e2"]);
}

#[test]
fn auto_mode_selects_cooccurrence_without_relation_edges() {
    let db = cooccurrence_db();
    let conn = db.connect().unwrap();
    let result = entity_graph(&conn, &EntityGraphOptions::new("e1")).unwrap();
    assert_eq!(result.mode, ResolvedEntityGraphMode::CoOccurrence);
}

#[test]
fn auto_mode_selects_relation_when_relation_edges_exist() {
    let db = cooccurrence_db();
    {
        let conn = db.connect().unwrap();
        relation(&conn, "e1", "e2", "knows");
    }
    let conn = db.connect().unwrap();
    let result = entity_graph(&conn, &EntityGraphOptions::new("e1")).unwrap();
    assert_eq!(result.mode, ResolvedEntityGraphMode::Relation);
    // Relation traversal reaches e2 via the explicit edge.
    let ids: Vec<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, vec!["e1", "e2"]);
    // The relation-mode edge carries the stored label and weight 1.
    assert_eq!(result.total_edges, 1);
    assert_eq!(result.edges[0].relation.as_deref(), Some("knows"));
    assert_eq!(result.edges[0].weight, 1);
}

#[test]
fn forced_cooccurrence_mode_ignores_relation_edges() {
    let db = cooccurrence_db();
    {
        let conn = db.connect().unwrap();
        relation(&conn, "e1", "e3", "knows"); // would connect e1-e3 in relation mode
    }
    let conn = db.connect().unwrap();
    let mut options = EntityGraphOptions::new("e1");
    options.mode = EntityGraphMode::CoOccurrence;
    let result = entity_graph(&conn, &options).unwrap();
    assert_eq!(result.mode, ResolvedEntityGraphMode::CoOccurrence);
    // Depth 1 co-occurrence still reaches only e2 (not e3 via the relation edge).
    let ids: Vec<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, vec!["e1", "e2"]);
}

#[test]
fn depth_out_of_range_is_rejected() {
    let db = cooccurrence_db();
    let conn = db.connect().unwrap();
    for bad in [0, 4, -1] {
        let mut options = EntityGraphOptions::new("e1");
        options.depth = Some(bad);
        let err = entity_graph(&conn, &options).unwrap_err();
        assert!(matches!(err, QueryError::InvalidArgument(_)), "depth {bad}");
    }
}

#[test]
fn unknown_seed_is_entity_not_found() {
    let db = cooccurrence_db();
    let conn = db.connect().unwrap();
    let err = entity_graph(&conn, &EntityGraphOptions::new("does-not-exist")).unwrap_err();
    match err {
        QueryError::EntityNotFound(id) => assert_eq!(id, "does-not-exist"),
        other => panic!("expected EntityNotFound, got {other:?}"),
    }
}

#[test]
fn limit_caps_the_returned_nodes() {
    let db = Database::in_memory().unwrap();
    {
        let conn = db.connect().unwrap();
        schema(&conn);
        entity(&conn, "hub", "Hub", "concept");
        // A hub co-occurring with many neighbors in one article.
        let mut ids = vec!["hub"];
        let names: Vec<String> = (0..10).map(|i| format!("n{i:02}")).collect();
        for name in &names {
            entity(&conn, name, name, "concept");
            ids.push(name);
        }
        article_with(&conn, "A1", &ids);
    }
    let conn = db.connect().unwrap();
    let mut options = EntityGraphOptions::new("hub");
    options.limit = Some(3);
    let result = entity_graph(&conn, &options).unwrap();
    assert_eq!(result.total_nodes, 3);
    assert_eq!(result.nodes.len(), 3);
    // Seed is always first.
    assert_eq!(result.nodes[0].id, "hub");
}

#[test]
fn cooccurrence_edge_weight_counts_shared_articles() {
    // e1 and e2 co-occur in TWO articles → the co-occurrence edge weight is 2.
    let db = Database::in_memory().unwrap();
    {
        let conn = db.connect().unwrap();
        schema(&conn);
        entity(&conn, "e1", "Alpha", "person");
        entity(&conn, "e2", "Bravo", "person");
        article_with(&conn, "A1", &["e1", "e2"]);
        article_with(&conn, "A2", &["e1", "e2"]);
    }
    let conn = db.connect().unwrap();
    let result = entity_graph(&conn, &EntityGraphOptions::new("e1")).unwrap();
    assert_eq!(result.total_edges, 1);
    assert_eq!(result.edges[0].relation.as_deref(), Some("co_occurs"));
    assert_eq!(result.edges[0].weight, 2);
    // Each entity is in both articles.
    assert!(result.nodes.iter().all(|n| n.articles_count == 2));
}

#[test]
fn relation_mode_depth_2_traverses_two_hops() {
    // Explicit relation chain e1 -> e2 -> e3; auto mode picks relation traversal.
    let db = Database::in_memory().unwrap();
    {
        let conn = db.connect().unwrap();
        schema(&conn);
        entity(&conn, "e1", "Alpha", "person");
        entity(&conn, "e2", "Bravo", "person");
        entity(&conn, "e3", "Charlie", "person");
        relation(&conn, "e1", "e2", "knows");
        relation(&conn, "e2", "e3", "uses");
    }
    let conn = db.connect().unwrap();
    let mut options = EntityGraphOptions::new("e1");
    options.depth = Some(2);
    let result = entity_graph(&conn, &options).unwrap();

    assert_eq!(result.mode, ResolvedEntityGraphMode::Relation);
    let ids: Vec<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, vec!["e1", "e2", "e3"]);
    assert_eq!(result.nodes[2].depth, 2);
    // Both directed relation edges among the node set, each weight 1.
    assert_eq!(result.total_edges, 2);
    assert!(result.edges.iter().all(|e| e.weight == 1));
    let mut labels: Vec<&str> = result
        .edges
        .iter()
        .filter_map(|e| e.relation.as_deref())
        .collect();
    labels.sort_unstable();
    assert_eq!(labels, vec!["knows", "uses"]);
}
