//! Integration tests for the scalable bulk `ENTITY_RELATION` loader
//! ([`bulk_create_entity_relations`]): correctness of the `COPY` path, its append
//! semantics, CSV escaping of relation/context text, an empty no-op, and a
//! larger-batch throughput/correctness check — over a real in-memory LadybugDB.

use kgpacks_db::{Connection, Database, Value};
use kgpacks_ingestion::{
    bulk_create_entity_relations, create_entity_relations_batched, EntityRelationRow,
};

fn setup(conn: &Connection<'_>, entity_ids: &[&str]) {
    conn.run(
        "CREATE NODE TABLE Entity(entity_id STRING, name STRING, type STRING, \
         description STRING, PRIMARY KEY(entity_id))",
    )
    .unwrap();
    conn.run(
        "CREATE REL TABLE ENTITY_RELATION(FROM Entity TO Entity, relation STRING, context STRING)",
    )
    .unwrap();
    for id in entity_ids {
        conn.run_params(
            "CREATE (:Entity {entity_id: $id, name: $id, type: 'concept', description: ''})",
            vec![("id", Value::String((*id).into()))],
        )
        .unwrap();
    }
}

fn rel_count(conn: &Connection<'_>) -> i64 {
    let rows = conn
        .run("MATCH ()-[r:ENTITY_RELATION]->() RETURN count(r) AS n")
        .unwrap();
    match rows[0].get("n") {
        Some(Value::Int64(n)) => *n,
        Some(Value::Int32(n)) => i64::from(*n),
        other => panic!("unexpected count: {other:?}"),
    }
}

fn row(source: &str, target: &str, rel: &str, ctx: &str) -> EntityRelationRow {
    EntityRelationRow {
        source_id: source.into(),
        target_id: target.into(),
        relation: rel.into(),
        context: ctx.into(),
    }
}

#[test]
fn empty_input_is_a_no_op() {
    let db = Database::in_memory().unwrap();
    let conn = db.connect().unwrap();
    setup(&conn, &["a", "b"]);
    assert_eq!(bulk_create_entity_relations(&conn, &[]).unwrap(), 0);
    assert_eq!(rel_count(&conn), 0);
}

#[test]
fn bulk_load_creates_all_edges_with_properties() {
    let db = Database::in_memory().unwrap();
    let conn = db.connect().unwrap();
    setup(&conn, &["a", "b", "c"]);
    let rows = vec![
        row("a", "b", "knows", "a knows b"),
        row("b", "c", "uses", "b uses c"),
    ];
    let created = bulk_create_entity_relations(&conn, &rows).unwrap();
    assert_eq!(created, 2);
    assert_eq!(rel_count(&conn), 2);

    // Properties round-trip through the bulk load.
    let props = conn
        .run(
            "MATCH (:Entity {entity_id: 'a'})-[r:ENTITY_RELATION]->(:Entity {entity_id: 'b'}) \
             RETURN r.relation AS rel, r.context AS ctx",
        )
        .unwrap();
    assert!(matches!(props[0].get("rel"), Some(Value::String(s)) if s == "knows"));
    assert!(matches!(props[0].get("ctx"), Some(Value::String(s)) if s == "a knows b"));
}

#[test]
fn bulk_load_appends_into_a_nonempty_table() {
    let db = Database::in_memory().unwrap();
    let conn = db.connect().unwrap();
    setup(&conn, &["a", "b", "c"]);
    bulk_create_entity_relations(&conn, &[row("a", "b", "knows", "")]).unwrap();
    assert_eq!(rel_count(&conn), 1);
    // A second bulk call appends rather than replacing.
    bulk_create_entity_relations(&conn, &[row("b", "c", "uses", "")]).unwrap();
    assert_eq!(rel_count(&conn), 2);
}

#[test]
fn csv_special_characters_round_trip() {
    let db = Database::in_memory().unwrap();
    let conn = db.connect().unwrap();
    setup(&conn, &["a", "b"]);
    // Context with a comma, embedded double-quotes and a newline must survive the
    // RFC-4180 CSV encoding used by the COPY path.
    let tricky = "he said \"hi, there\"\nand left";
    bulk_create_entity_relations(&conn, &[row("a", "b", "said", tricky)]).unwrap();
    let rows = conn
        .run("MATCH ()-[r:ENTITY_RELATION]->() RETURN r.context AS ctx")
        .unwrap();
    assert!(matches!(rows[0].get("ctx"), Some(Value::String(s)) if s == tricky));
}

#[test]
fn bulk_load_scales_to_a_large_batch() {
    let db = Database::in_memory().unwrap();
    let conn = db.connect().unwrap();
    // A chain of 5_000 entities, each linked to the next: 4_999 relationship edges
    // loaded in a single bulk call (exercises the scalable path at size).
    let n = 5_000usize;
    let ids: Vec<String> = (0..n).map(|i| format!("e{i:05}")).collect();
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    setup(&conn, &id_refs);
    let rows: Vec<EntityRelationRow> = (0..n - 1)
        .map(|i| EntityRelationRow {
            source_id: ids[i].clone(),
            target_id: ids[i + 1].clone(),
            relation: "next".into(),
            context: String::new(),
        })
        .collect();
    let created = bulk_create_entity_relations(&conn, &rows).unwrap();
    assert_eq!(created, n - 1);
    assert_eq!(rel_count(&conn), (n - 1) as i64);
}

#[test]
fn batched_unwind_path_creates_edges_directly() {
    // Exercises the COPY-independent PK-indexed UNWIND batch loader on its own.
    let db = Database::in_memory().unwrap();
    let conn = db.connect().unwrap();
    setup(&conn, &["a", "b", "c"]);
    let rows = vec![
        row("a", "b", "knows", "ctx1"),
        row("b", "c", "uses", "ctx2"),
    ];
    let created = create_entity_relations_batched(&conn, &rows).unwrap();
    assert_eq!(created, 2);
    assert_eq!(rel_count(&conn), 2);
    let props = conn
        .run(
            "MATCH (:Entity {entity_id: 'b'})-[r:ENTITY_RELATION]->(:Entity {entity_id: 'c'}) \
             RETURN r.relation AS rel",
        )
        .unwrap();
    assert!(matches!(props[0].get("rel"), Some(Value::String(s)) if s == "uses"));
}

#[test]
fn batched_unwind_path_skips_dangling_endpoints() {
    // A row referencing a non-existent endpoint is silently skipped by MATCH
    // (only the valid edge is created), so the loader is robust to dangling rows.
    let db = Database::in_memory().unwrap();
    let conn = db.connect().unwrap();
    setup(&conn, &["a", "b"]);
    let rows = vec![row("a", "b", "knows", ""), row("a", "missing", "knows", "")];
    create_entity_relations_batched(&conn, &rows).unwrap();
    assert_eq!(rel_count(&conn), 1);
}
