//! Integration tests for the `GET /api/v1/graph/entities` handler
//! ([`graph_entities`]): a valid request builds the neighborhood; an unknown seed
//! surfaces as a `404 NOT_FOUND`; validation failures surface as `400` envelopes.
//! Exercised over a real in-memory LadybugDB.

use kgpacks_backend::{graph_entities, ErrorCode, GraphEntitiesQuery};
use kgpacks_db::{Connection, Database, Value};

fn seed_db() -> Database {
    let db = Database::in_memory().unwrap();
    {
        let conn = db.connect().unwrap();
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
        for (id, name) in [("e1", "Alpha"), ("e2", "Bravo")] {
            conn.run_params(
                "CREATE (:Entity {entity_id: $id, name: $name, type: 'person', description: ''})",
                vec![
                    ("id", Value::String(id.into())),
                    ("name", Value::String(name.into())),
                ],
            )
            .unwrap();
        }
        conn.run("CREATE (:Article {title: 'A1'})").unwrap();
        for id in ["e1", "e2"] {
            conn.run_params(
                "MATCH (a:Article {title: 'A1'}), (e:Entity {entity_id: $id}) \
                 CREATE (a)-[:HAS_ENTITY]->(e)",
                vec![("id", Value::String(id.into()))],
            )
            .unwrap();
        }
    }
    db
}

fn conn_of(db: &Database) -> Connection<'_> {
    db.connect().unwrap()
}

#[test]
fn valid_request_returns_the_neighborhood() {
    let db = seed_db();
    let conn = conn_of(&db);
    let query = GraphEntitiesQuery::from_pairs([("entity", "e1")]);
    let result = graph_entities(&conn, &query).unwrap();

    assert_eq!(result.seed, "e1");
    let ids: Vec<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, vec!["e1", "e2"]);
    assert_eq!(result.total_edges, 1);
}

#[test]
fn unknown_seed_is_a_404() {
    let db = seed_db();
    let conn = conn_of(&db);
    let query = GraphEntitiesQuery::from_pairs([("entity", "nope")]);
    let err = graph_entities(&conn, &query).unwrap_err();
    assert_eq!(err.status_code, 404);
    assert_eq!(err.code, ErrorCode::NotFound);
    // The error renders the standard envelope.
    let envelope = err.to_envelope().to_json();
    let parsed: serde_json::Value = serde_json::from_str(&envelope).unwrap();
    assert_eq!(parsed["error"]["code"], "NOT_FOUND");
}

#[test]
fn missing_entity_is_a_400_missing_parameter() {
    let db = seed_db();
    let conn = conn_of(&db);
    let err = graph_entities(&conn, &GraphEntitiesQuery::default()).unwrap_err();
    assert_eq!(err.status_code, 400);
    assert_eq!(err.code, ErrorCode::MissingParameter);
}

#[test]
fn out_of_range_depth_is_a_400_invalid_parameter() {
    let db = seed_db();
    let conn = conn_of(&db);
    let query = GraphEntitiesQuery::from_pairs([("entity", "e1"), ("depth", "9")]);
    let err = graph_entities(&conn, &query).unwrap_err();
    assert_eq!(err.status_code, 400);
    assert_eq!(err.code, ErrorCode::InvalidParameter);
}

#[test]
fn depth_2_and_limit_are_honored() {
    let db = seed_db();
    let conn = conn_of(&db);
    let query = GraphEntitiesQuery::from_pairs([("entity", "e1"), ("depth", "2"), ("limit", "1")]);
    let result = graph_entities(&conn, &query).unwrap();
    // limit caps the node set to just the seed.
    assert_eq!(result.total_nodes, 1);
    assert_eq!(result.nodes[0].id, "e1");
}
