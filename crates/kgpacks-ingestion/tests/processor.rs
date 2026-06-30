//! Integration tests for the article processor
//! (`bootstrap/src/expansion/processor.py` + `database/loader.py`): a hermetic
//! `MapContentSource` is loaded end to end into the working store, and secret
//! redaction is checked (parity with `test_processor_security.py`).

use kgpacks_db::{Connection, Database, Value};
use kgpacks_embeddings::Embedder;
use kgpacks_ingestion::extraction::{Entity, ExtractionResult, Relationship};
use kgpacks_ingestion::{
    apply_ingestion_schema, sanitize_error, Article, ArticleProcessor, MapContentSource,
    MockExtractor,
};
use std::collections::BTreeMap;

fn store() -> Database {
    let db = Database::in_memory().expect("open in-memory db");
    {
        let conn = db.connect().expect("connect");
        apply_ingestion_schema(&conn).expect("apply schema");
    }
    db
}

fn rust_article() -> Article {
    Article {
        title: "Rust".to_string(),
        content: "Rust is a systems programming language.\n\n\
                  ## History\nRust began as a personal project in 2006.\n\n\
                  ## Features\nRust enforces memory safety via ownership and borrowing."
            .to_string(),
        links: vec!["Cargo".to_string(), "LLVM".to_string()],
        categories: vec![
            "Programming languages".to_string(),
            "Systems software".to_string(),
        ],
        source_url: String::new(),
        source_type: "memory".to_string(),
    }
}

fn count(conn: &Connection<'_>, cypher: &str) -> i64 {
    let rows = conn.run(cypher).unwrap();
    match rows[0].get("n") {
        Some(Value::Int64(n)) => *n,
        Some(Value::Int32(n)) => i64::from(*n),
        other => panic!("unexpected count: {other:?}"),
    }
}

#[test]
fn loads_article_sections_and_chunks_with_embeddings() {
    let db = store();
    let conn = db.connect().unwrap();
    let source = MapContentSource::new().with_article(rust_article());
    let embedder = Embedder::bge();
    let processor = ArticleProcessor::new(&conn, &source, &embedder);

    let outcome = processor.process_article("Rust", "Programming", 0);
    assert!(outcome.success);
    assert_eq!(outcome.links, vec!["Cargo".to_string(), "LLVM".to_string()]);
    assert_eq!(outcome.error, None);

    // Article is loaded with a positive word count.
    let rows = conn
        .run("MATCH (a:Article {title: 'Rust'}) RETURN a.expansion_state AS s, a.word_count AS w")
        .unwrap();
    assert!(matches!(rows[0].get("s"), Some(Value::String(s)) if s == "loaded"));
    assert!(matches!(rows[0].get("w"), Some(Value::Int64(w)) if *w > 0));

    // Three sections (intro + History + Features), at least one chunk, and the
    // category edges.
    assert_eq!(count(&conn, "MATCH (s:Section) RETURN COUNT(s) AS n"), 3);
    assert!(count(&conn, "MATCH (c:Chunk) RETURN COUNT(c) AS n") >= 1);
    assert_eq!(
        count(
            &conn,
            "MATCH (:Article)-[r:IN_CATEGORY]->(:Category) RETURN COUNT(r) AS n"
        ),
        2
    );

    // Embeddings are persisted as 768-wide arrays.
    let emb = conn
        .run("MATCH (s:Section) RETURN s.embedding AS e LIMIT 1")
        .unwrap();
    match emb[0].get("e") {
        Some(Value::Array(_, items)) | Some(Value::List(_, items)) => {
            assert_eq!(items.len(), 768)
        }
        other => panic!("expected an embedding array, got {other:?}"),
    }
}

#[test]
fn reprocessing_is_idempotent() {
    let db = store();
    let conn = db.connect().unwrap();
    let source = MapContentSource::new().with_article(rust_article());
    let embedder = Embedder::bge();
    let processor = ArticleProcessor::new(&conn, &source, &embedder);

    processor.process_article("Rust", "Programming", 0);
    processor.process_article("Rust", "Programming", 0);

    // Sections are replaced, not duplicated, on a second pass.
    assert_eq!(count(&conn, "MATCH (a:Article) RETURN COUNT(a) AS n"), 1);
    assert_eq!(count(&conn, "MATCH (s:Section) RETURN COUNT(s) AS n"), 3);
}

#[test]
fn loads_llm_extraction_when_an_extractor_is_attached() {
    let db = store();
    let conn = db.connect().unwrap();
    let source = MapContentSource::new().with_article(rust_article());
    let embedder = Embedder::bge();

    let mut props = BTreeMap::new();
    props.insert(
        "description".to_string(),
        serde_json::json!("the cargo tool"),
    );
    let extraction = ExtractionResult {
        entities: vec![
            Entity {
                name: "Rust".to_string(),
                type_: "concept".to_string(),
                properties: BTreeMap::new(),
            },
            Entity {
                name: "Cargo".to_string(),
                type_: "tool".to_string(),
                properties: props,
            },
        ],
        relationships: vec![Relationship {
            source: "Rust".to_string(),
            relation: "uses".to_string(),
            target: "Cargo".to_string(),
            context: "Rust uses Cargo".to_string(),
        }],
        key_facts: vec!["Rust is memory safe".to_string()],
    };
    let extractor = MockExtractor::new(extraction);
    let processor = ArticleProcessor::new(&conn, &source, &embedder).with_extractor(&extractor);

    assert!(processor.process_article("Rust", "Programming", 0).success);

    assert_eq!(count(&conn, "MATCH (e:Entity) RETURN COUNT(e) AS n"), 2);
    assert_eq!(count(&conn, "MATCH (f:Fact) RETURN COUNT(f) AS n"), 1);
    assert_eq!(
        count(
            &conn,
            "MATCH (:Article)-[r:HAS_ENTITY]->(:Entity) RETURN COUNT(r) AS n"
        ),
        2
    );
    assert_eq!(
        count(
            &conn,
            "MATCH (:Entity)-[r:ENTITY_RELATION]->(:Entity) RETURN COUNT(r) AS n"
        ),
        1
    );
}

#[test]
fn not_found_article_reports_failure() {
    let db = store();
    let conn = db.connect().unwrap();
    let source = MapContentSource::new(); // empty
    let embedder = Embedder::bge();
    let processor = ArticleProcessor::new(&conn, &source, &embedder);

    let outcome = processor.process_article("Nonexistent", "General", 0);
    assert!(!outcome.success);
    assert_eq!(
        outcome.error.as_deref(),
        Some("Article not found: Nonexistent")
    );
}

#[test]
fn stub_article_without_sections_is_a_benign_skip() {
    let db = store();
    let conn = db.connect().unwrap();
    let stub = Article {
        title: "Stub".to_string(),
        content: "   ".to_string(), // no headings, no body -> no sections
        links: vec!["Somewhere".to_string()],
        categories: vec![],
        source_url: String::new(),
        source_type: "memory".to_string(),
    };
    let source = MapContentSource::new().with_article(stub);
    let embedder = Embedder::bge();
    let processor = ArticleProcessor::new(&conn, &source, &embedder);

    let outcome = processor.process_article("Stub", "General", 0);
    assert!(outcome.success);
    assert_eq!(outcome.links, vec!["Somewhere".to_string()]);
    // Nothing was loaded.
    assert_eq!(count(&conn, "MATCH (a:Article) RETURN COUNT(a) AS n"), 0);
}

#[test]
fn sanitize_error_redacts_secrets() {
    let sanitized = sanitize_error("auth failed with api_key=sk-abcdefghijklmnopqrstuvwxyz012345");
    assert!(!sanitized.contains("sk-abcdefghijklmnopqrstuvwxyz012345"));
    assert!(sanitized.contains("REDACTED"));

    let jwt = sanitize_error(
        "token eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abc123",
    );
    assert!(jwt.contains("REDACTED"));
}

#[test]
fn sanitize_error_redacts_dict_style_and_authorization_headers() {
    // A 20–29-char dict-style value (too short for the bare-key rule) must still
    // be redacted by the dict-style rule.
    let dict = sanitize_error("config {\"api_key\": \"abcdefghij0123456789\"}");
    assert!(!dict.contains("abcdefghij0123456789"), "got: {dict}");
    assert!(dict.contains("REDACTED"));

    let header = sanitize_error("Authorization: Bearer abcdef123456");
    assert!(!header.contains("abcdef123456"), "got: {header}");
    assert!(header.contains("REDACTED"));
}
