//! Contract tests for the `kgpacks-db` wrapper (`Database` / `Connection`).
//!
//! Ports `packages/db/test/database.test.ts` from the TypeScript reference and
//! proves the M2 parity surface: open/close, fresh connections, bound-parameter
//! Cypher (never interpolated), empty results, error propagation, on-disk
//! persistence, and the `auto_checkpoint = false` durability property.
//!
//! These run fully OFFLINE — only the core graph engine is exercised (no
//! VECTOR/FTS extension, which is M4). Extension-loader wiring is covered by its
//! own error-path test.

use kgpacks_db::{Database, DatabaseOptions, Value};

fn i64_of(value: &Value) -> i64 {
    match value {
        Value::Int64(n) => *n,
        Value::Int32(n) => i64::from(*n),
        other => panic!("expected an integer value, got {other:?}"),
    }
}

fn string_of(value: &Value) -> &str {
    match value {
        Value::String(s) => s.as_str(),
        other => panic!("expected a string value, got {other:?}"),
    }
}

// ── Database ────────────────────────────────────────────────────────────────

#[test]
fn opens_in_memory_by_default_and_yields_a_usable_connection() {
    let db = Database::in_memory().expect("open in-memory database");
    let conn = db.connect().expect("connect");
    assert!(conn.is_open());
    conn.run("RETURN 1 AS one").expect("trivial query runs");
}

#[test]
fn connect_returns_a_fresh_connection_on_each_call() {
    let db = Database::in_memory().expect("open in-memory database");
    let a = db.connect().expect("first connect");
    let b = db.connect().expect("second connect");
    // Two independent, usable connections.
    a.run("RETURN 1 AS one").expect("a runs");
    b.run("RETURN 1 AS one").expect("b runs");
}

#[test]
fn database_close_is_idempotent() {
    let mut db = Database::in_memory().expect("open in-memory database");
    db.close();
    db.close();
    assert!(!db.is_open());
}

#[test]
fn connect_after_close_errors() {
    let mut db = Database::in_memory().expect("open in-memory database");
    db.close();
    assert!(db.connect().is_err());
}

// ── Connection.run ──────────────────────────────────────────────────────────

#[test]
fn round_trips_nodes_and_returns_rows_keyed_by_return_aliases() {
    let db = Database::in_memory().expect("open in-memory database");
    let conn = db.connect().expect("connect");

    conn.run("CREATE NODE TABLE Doc(id INT64, title STRING, PRIMARY KEY(id))")
        .expect("create table");
    conn.run("CREATE (:Doc {id: 1, title: 'alpha'})")
        .expect("insert alpha");
    conn.run("CREATE (:Doc {id: 2, title: 'beta'})")
        .expect("insert beta");

    let rows = conn
        .run("MATCH (d:Doc) RETURN d.id AS id, d.title AS title ORDER BY d.id")
        .expect("match query");

    assert_eq!(rows.len(), 2);
    assert_eq!(string_of(&rows[0]["title"]), "alpha");
    assert_eq!(string_of(&rows[1]["title"]), "beta");
    assert_eq!(i64_of(&rows[0]["id"]), 1);
    assert_eq!(i64_of(&rows[1]["id"]), 2);
}

#[test]
fn binds_named_params_instead_of_interpolating_them() {
    let db = Database::in_memory().expect("open in-memory database");
    let conn = db.connect().expect("connect");

    conn.run("CREATE NODE TABLE Doc(id INT64, title STRING, PRIMARY KEY(id))")
        .expect("create table");
    conn.run_params(
        "CREATE (:Doc {id: $id, title: $title})",
        vec![
            ("id", Value::Int64(7)),
            ("title", Value::String("gamma".into())),
        ],
    )
    .expect("parameterized insert");

    let rows = conn
        .run_params(
            "MATCH (d:Doc) WHERE d.id = $id RETURN d.title AS title",
            vec![("id", Value::Int64(7))],
        )
        .expect("parameterized match");

    assert_eq!(rows.len(), 1);
    assert_eq!(string_of(&rows[0]["title"]), "gamma");
}

#[test]
fn bound_params_are_not_interpreted_as_query_text() {
    // A param value that would be a SQL/Cypher injection if interpolated must be
    // treated as an opaque string literal when bound.
    let db = Database::in_memory().expect("open in-memory database");
    let conn = db.connect().expect("connect");

    conn.run("CREATE NODE TABLE Doc(id INT64, title STRING, PRIMARY KEY(id))")
        .expect("create table");
    let evil = "'); MATCH (n) DETACH DELETE n; //";
    conn.run_params(
        "CREATE (:Doc {id: $id, title: $title})",
        vec![
            ("id", Value::Int64(1)),
            ("title", Value::String(evil.into())),
        ],
    )
    .expect("insert with hostile title");

    let rows = conn
        .run("MATCH (d:Doc) RETURN d.title AS title")
        .expect("read back");
    assert_eq!(rows.len(), 1);
    assert_eq!(string_of(&rows[0]["title"]), evil);
}

#[test]
fn returns_an_empty_vec_when_a_match_finds_nothing() {
    let db = Database::in_memory().expect("open in-memory database");
    let conn = db.connect().expect("connect");

    conn.run("CREATE NODE TABLE Doc(id INT64, PRIMARY KEY(id))")
        .expect("create table");
    let rows = conn
        .run_params(
            "MATCH (d:Doc) WHERE d.id = $id RETURN d.id AS id",
            vec![("id", Value::Int64(999))],
        )
        .expect("match query");
    assert!(rows.is_empty());
}

#[test]
fn errors_on_invalid_cypher() {
    let db = Database::in_memory().expect("open in-memory database");
    let conn = db.connect().expect("connect");
    assert!(conn.run("THIS IS NOT VALID CYPHER").is_err());
}

// ── Connection lifecycle ────────────────────────────────────────────────────

#[test]
fn connection_close_is_idempotent() {
    let db = Database::in_memory().expect("open in-memory database");
    let mut conn = db.connect().expect("connect");
    conn.close();
    conn.close();
    assert!(!conn.is_open());
}

#[test]
fn run_after_connection_close_errors() {
    let db = Database::in_memory().expect("open in-memory database");
    let mut conn = db.connect().expect("connect");
    conn.close();
    assert!(conn.run("RETURN 1 AS one").is_err());
}

// ── Extension loader ────────────────────────────────────────────────────────

#[test]
fn load_extension_propagates_errors_for_an_unknown_extension() {
    // The loader issues the real `INSTALL` + `LOAD EXTENSION` sequence. Loading a
    // clearly invalid extension name must surface an error rather than silently
    // succeed — proving the wiring without depending on a successful (network +
    // M4 vector/FTS) extension fetch.
    let db = Database::in_memory().expect("open in-memory database");
    let conn = db.connect().expect("connect");
    assert!(conn
        .load_extension("definitely_not_a_real_extension_xyz")
        .is_err());
}

// ── On-disk database ────────────────────────────────────────────────────────

#[test]
fn persists_to_a_path_and_reads_back_from_a_reopened_database() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("pack.lbug");

    {
        let db = Database::open(&path).expect("open on-disk database");
        let writer = db.connect().expect("connect writer");
        writer
            .run("CREATE NODE TABLE Doc(id INT64, PRIMARY KEY(id))")
            .expect("create table");
        writer.run("CREATE (:Doc {id: 42})").expect("insert");
        // `db`/`writer` drop here, flushing and closing the database.
    }

    let db = Database::open(&path).expect("reopen on-disk database");
    let reader = db.connect().expect("connect reader");
    let rows = reader
        .run("MATCH (d:Doc) RETURN d.id AS id")
        .expect("read back");
    assert_eq!(rows.len(), 1);
    assert_eq!(i64_of(&rows[0]["id"]), 42);
}

#[test]
fn auto_checkpoint_false_is_durable_and_leaves_no_wal_after_close() {
    // The bulk builder opens with `auto_checkpoint = Some(false)` to keep a large
    // streaming load linear (WAL-only appends, one checkpoint at close). Verify
    // the durability property that path depends on: close() still checkpoints, so
    // the data reads back AND the pack file is self-contained for distribution.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("nocheckpoint.lbug");

    {
        let options = DatabaseOptions {
            auto_checkpoint: Some(false),
            ..DatabaseOptions::default()
        };
        let mut db = Database::open_with_options(&path, options).expect("open on-disk database");
        {
            let mut writer = db.connect().expect("connect writer");
            writer
                .run("CREATE NODE TABLE Doc(id INT64, PRIMARY KEY(id))")
                .expect("create table");
            writer
                .run("UNWIND range(1, 50) AS i CREATE (:Doc {id: i})")
                .expect("bulk insert");
            writer.close();
        }
        db.close();
    }

    let wal = path.with_extension("lbug.wal");
    assert!(
        !wal.exists(),
        "expected no WAL sidecar at {} after close",
        wal.display()
    );

    let db = Database::open(&path).expect("reopen on-disk database");
    let reader = db.connect().expect("connect reader");
    let rows = reader
        .run("MATCH (d:Doc) RETURN count(d) AS n")
        .expect("count rows");
    assert_eq!(i64_of(&rows[0]["n"]), 50);
}
