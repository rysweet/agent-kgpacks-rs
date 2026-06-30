//! Parity tests for link discovery,
//! mirroring `bootstrap/src/expansion/tests/test_link_discovery.py`.

use kgpacks_db::{Connection, Database, Value};
use kgpacks_ingestion::{apply_ingestion_schema, LinkDiscovery};

/// Open an in-memory working store with the ingestion schema applied.
fn store() -> Database {
    let db = Database::in_memory().expect("open in-memory db");
    {
        let conn = db.connect().expect("connect");
        apply_ingestion_schema(&conn).expect("apply schema");
    }
    db
}

/// Insert an article in a given state at a given depth.
fn insert_article(conn: &Connection<'_>, title: &str, state: &str, depth: i64) {
    conn.run_params(
        "CREATE (:Article {title: $title, category: NULL, word_count: 0, \
         expansion_state: $state, expansion_depth: $depth, claimed_at: NULL, \
         processed_at: NULL, retry_count: 0})",
        vec![
            ("title", Value::String(title.to_string())),
            ("state", Value::String(state.to_string())),
            ("depth", Value::Int64(depth)),
        ],
    )
    .expect("insert article");
}

fn count_links(conn: &Connection<'_>, source: &str, target: &str) -> i64 {
    let rows = conn
        .run_params(
            "MATCH (s:Article {title: $s})-[r:LINKS_TO]->(t:Article {title: $t}) \
             RETURN COUNT(r) AS n",
            vec![
                ("s", Value::String(source.to_string())),
                ("t", Value::String(target.to_string())),
            ],
        )
        .expect("count links");
    match rows[0].get("n") {
        Some(Value::Int64(n)) => *n,
        Some(Value::Int32(n)) => i64::from(*n),
        other => panic!("unexpected count value: {other:?}"),
    }
}

const SEED: &str = "Python (programming language)";

fn seeded() -> Database {
    let db = store();
    {
        let conn = db.connect().unwrap();
        insert_article(&conn, SEED, "loaded", 0);
    }
    db
}

// ── _is_valid_link ──────────────────────────────────────────────────────────

#[test]
fn valid_link_filtering() {
    assert!(LinkDiscovery::is_valid_link("Python"));
    assert!(LinkDiscovery::is_valid_link("Machine Learning"));
    assert!(LinkDiscovery::is_valid_link(
        "Python (programming language)"
    ));
    assert!(LinkDiscovery::is_valid_link("Mercury (planet)"));

    for invalid in [
        "Wikipedia:About",
        "Help:Contents",
        "Template:Infobox",
        "File:Python_logo.svg",
        "Image:Example.jpg",
        "Category:Programming languages",
        "Portal:Technology",
        "User:Example",
        "List of programming languages",
        "Python (disambiguation)",
        "",
        "A",
    ] {
        assert!(
            !LinkDiscovery::is_valid_link(invalid),
            "{invalid} should be invalid"
        );
    }
}

// ── article_exists ──────────────────────────────────────────────────────────

#[test]
fn article_exists_reports_state() {
    let db = seeded();
    let conn = db.connect().unwrap();
    let disco = LinkDiscovery::new(&conn);

    let (exists, state) = disco.article_exists("Non-existent Article").unwrap();
    assert!(!exists);
    assert_eq!(state, None);

    let (exists, state) = disco.article_exists(SEED).unwrap();
    assert!(exists);
    assert_eq!(state.as_deref(), Some("loaded"));

    insert_article(&conn, "Discovered Article", "discovered", 1);
    let (exists, state) = disco.article_exists("Discovered Article").unwrap();
    assert!(exists);
    assert_eq!(state.as_deref(), Some("discovered"));
}

// ── discover_links ──────────────────────────────────────────────────────────

#[test]
fn discovers_new_articles() {
    let db = seeded();
    let conn = db.connect().unwrap();
    let disco = LinkDiscovery::new(&conn);
    let links = vec![
        "Machine Learning".to_string(),
        "Artificial Intelligence".to_string(),
        "Data Science".to_string(),
    ];
    assert_eq!(disco.discover_links(SEED, &links, 0, 2).unwrap(), 3);
}

#[test]
fn filters_invalid_links() {
    let db = seeded();
    let conn = db.connect().unwrap();
    let disco = LinkDiscovery::new(&conn);
    let links = vec![
        "Valid Article".to_string(),
        "Wikipedia:About".to_string(),
        "List of examples".to_string(),
        "Example (disambiguation)".to_string(),
    ];
    assert_eq!(disco.discover_links(SEED, &links, 0, 2).unwrap(), 1);
}

#[test]
fn links_to_existing_articles_without_rediscovering() {
    let db = seeded();
    let conn = db.connect().unwrap();
    insert_article(&conn, "Existing Article", "loaded", 1);
    let disco = LinkDiscovery::new(&conn);

    let new = disco
        .discover_links(SEED, &["Existing Article".to_string()], 0, 2)
        .unwrap();
    assert_eq!(new, 0);
    assert_eq!(count_links(&conn, SEED, "Existing Article"), 1);
}

#[test]
fn respects_max_depth() {
    let db = seeded();
    let conn = db.connect().unwrap();
    let disco = LinkDiscovery::new(&conn);
    let new = disco
        .discover_links(SEED, &["Should Not Discover".to_string()], 2, 2)
        .unwrap();
    assert_eq!(new, 0);
}

#[test]
fn sets_correct_depth_and_link_type() {
    let db = seeded();
    let conn = db.connect().unwrap();
    let disco = LinkDiscovery::new(&conn);
    disco
        .discover_links(SEED, &["New Article".to_string()], 0, 2)
        .unwrap();

    let rows = conn
        .run("MATCH (a:Article {title: 'New Article'}) RETURN a.expansion_depth AS depth")
        .unwrap();
    assert!(matches!(rows[0].get("depth"), Some(Value::Int64(1))));

    let link_rows = conn
        .run_params(
            "MATCH (:Article {title: $s})-[r:LINKS_TO]->(:Article {title: 'New Article'}) \
             RETURN r.link_type AS t",
            vec![("s", Value::String(SEED.to_string()))],
        )
        .unwrap();
    assert!(matches!(link_rows[0].get("t"), Some(Value::String(s)) if s == "internal"));
}

#[test]
fn handles_duplicate_links_to_one_article() {
    let db = seeded();
    let conn = db.connect().unwrap();
    let disco = LinkDiscovery::new(&conn);
    let links = vec![
        "Article A".to_string(),
        "Article A".to_string(),
        "Article A".to_string(),
    ];
    disco.discover_links(SEED, &links, 0, 2).unwrap();

    let rows = conn
        .run("MATCH (a:Article {title: 'Article A'}) RETURN COUNT(a) AS n")
        .unwrap();
    assert!(matches!(rows[0].get("n"), Some(Value::Int64(1))));
}

#[test]
fn empty_links_list_discovers_nothing() {
    let db = seeded();
    let conn = db.connect().unwrap();
    let disco = LinkDiscovery::new(&conn);
    assert_eq!(disco.discover_links(SEED, &[], 0, 2).unwrap(), 0);
}

#[test]
fn does_not_create_duplicate_relationships() {
    let db = seeded();
    let conn = db.connect().unwrap();
    let disco = LinkDiscovery::new(&conn);
    let links = vec!["Target".to_string()];
    disco.discover_links(SEED, &links, 0, 2).unwrap();
    disco.discover_links(SEED, &links, 0, 2).unwrap();
    assert_eq!(count_links(&conn, SEED, "Target"), 1);
}

// ── get_discovered_count ────────────────────────────────────────────────────

#[test]
fn discovered_count_only_counts_discovered_state() {
    let db = store();
    let conn = db.connect().unwrap();
    let disco = LinkDiscovery::new(&conn);
    assert_eq!(disco.get_discovered_count().unwrap(), 0);

    for i in 0..5 {
        insert_article(&conn, &format!("Discovered {i}"), "discovered", 1);
    }
    for (i, state) in ["loaded", "claimed", "failed"].iter().enumerate() {
        insert_article(&conn, &format!("Other {i}"), state, 0);
    }
    assert_eq!(disco.get_discovered_count().unwrap(), 5);
}

// ── integration ─────────────────────────────────────────────────────────────

#[test]
fn full_discovery_workflow() {
    let db = seeded();
    let conn = db.connect().unwrap();
    let disco = LinkDiscovery::new(&conn);
    let links = vec![
        "Machine Learning".to_string(),
        "Artificial Intelligence".to_string(),
        "Python Syntax".to_string(),
        "Wikipedia:About".to_string(),
        "List of languages".to_string(),
    ];
    assert_eq!(disco.discover_links(SEED, &links, 0, 2).unwrap(), 3);
    assert_eq!(disco.get_discovered_count().unwrap(), 3);
    for title in [
        "Machine Learning",
        "Artificial Intelligence",
        "Python Syntax",
    ] {
        let (exists, state) = disco.article_exists(title).unwrap();
        assert!(exists);
        assert_eq!(state.as_deref(), Some("discovered"));
    }
}
