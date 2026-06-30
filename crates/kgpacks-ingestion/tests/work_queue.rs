//! Parity tests for the work-queue state machine
//! (`bootstrap/src/expansion/work_queue.py`). The reference ships no standalone
//! `test_work_queue.py`; these prove the claim/heartbeat/reclaim and state-
//! transition contract the orchestrator relies on.

use std::time::{SystemTime, UNIX_EPOCH};

use kgpacks_db::{Connection, Database, Value};
use kgpacks_ingestion::{apply_ingestion_schema, IngestionError, WorkQueueManager};

fn store() -> Database {
    let db = Database::in_memory().expect("open in-memory db");
    {
        let conn = db.connect().expect("connect");
        apply_ingestion_schema(&conn).expect("apply schema");
    }
    db
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// Insert an article with explicit state/depth/claimed_at/retry_count.
fn insert(
    conn: &Connection<'_>,
    title: &str,
    state: &str,
    depth: i64,
    claimed_at: Option<i64>,
    retry: i64,
) {
    let claimed = match claimed_at {
        Some(ms) => Value::Int64(ms),
        None => Value::Null(kgpacks_db::LogicalType::Int64),
    };
    conn.run_params(
        "CREATE (:Article {title: $title, category: NULL, word_count: 0, \
         expansion_state: $state, expansion_depth: $depth, claimed_at: $claimed, \
         processed_at: NULL, retry_count: $retry})",
        vec![
            ("title", Value::String(title.to_string())),
            ("state", Value::String(state.to_string())),
            ("depth", Value::Int64(depth)),
            ("claimed", claimed),
            ("retry", Value::Int64(retry)),
        ],
    )
    .expect("insert article");
}

fn state_of(conn: &Connection<'_>, title: &str) -> String {
    let rows = conn
        .run_params(
            "MATCH (a:Article {title: $title}) RETURN a.expansion_state AS s",
            vec![("title", Value::String(title.to_string()))],
        )
        .unwrap();
    match rows[0].get("s") {
        Some(Value::String(s)) => s.clone(),
        other => panic!("unexpected state: {other:?}"),
    }
}

fn retry_of(conn: &Connection<'_>, title: &str) -> i64 {
    let rows = conn
        .run_params(
            "MATCH (a:Article {title: $title}) RETURN a.retry_count AS r",
            vec![("title", Value::String(title.to_string()))],
        )
        .unwrap();
    match rows[0].get("r") {
        Some(Value::Int64(n)) => *n,
        Some(Value::Int32(n)) => i64::from(*n),
        other => panic!("unexpected retry: {other:?}"),
    }
}

#[test]
fn claim_work_takes_lowest_depth_first_and_marks_claimed() {
    let db = store();
    let conn = db.connect().unwrap();
    insert(&conn, "Deep", "discovered", 2, None, 0);
    insert(&conn, "Seed", "discovered", 0, None, 0);
    insert(&conn, "Mid", "discovered", 1, None, 0);

    let queue = WorkQueueManager::new(&conn);
    let claimed = queue.claim_work(2).unwrap();
    assert_eq!(claimed.len(), 2);
    assert_eq!(claimed[0].title, "Seed");
    assert_eq!(claimed[1].title, "Mid");
    assert_eq!(state_of(&conn, "Seed"), "claimed");
    assert_eq!(state_of(&conn, "Mid"), "claimed");
    assert_eq!(state_of(&conn, "Deep"), "discovered");
}

#[test]
fn claim_work_returns_empty_when_no_work() {
    let db = store();
    let conn = db.connect().unwrap();
    let queue = WorkQueueManager::new(&conn);
    assert!(queue.claim_work(5).unwrap().is_empty());
}

#[test]
fn advance_state_follows_the_success_path_and_rejects_invalid_states() {
    let db = store();
    let conn = db.connect().unwrap();
    insert(&conn, "A", "discovered", 0, None, 0);
    let queue = WorkQueueManager::new(&conn);

    queue.advance_state("A", "claimed").unwrap();
    assert_eq!(state_of(&conn, "A"), "claimed");
    queue.advance_state("A", "loaded").unwrap();
    assert_eq!(state_of(&conn, "A"), "loaded");
    queue.advance_state("A", "processed").unwrap();
    assert_eq!(state_of(&conn, "A"), "processed");

    let err = queue.advance_state("A", "bogus").unwrap_err();
    assert!(matches!(err, IngestionError::InvalidState(s) if s == "bogus"));
}

#[test]
fn advance_state_guards_against_illegal_predecessors() {
    let db = store();
    let conn = db.connect().unwrap();
    insert(&conn, "A", "discovered", 0, None, 0);
    let queue = WorkQueueManager::new(&conn);

    // loaded's only legal predecessor is claimed, so a discovered article is
    // left untouched.
    queue.advance_state("A", "loaded").unwrap();
    assert_eq!(state_of(&conn, "A"), "discovered");
}

#[test]
fn mark_failed_retries_then_fails_after_budget() {
    let db = store();
    let conn = db.connect().unwrap();
    insert(&conn, "A", "claimed", 0, Some(now_ms()), 0);
    let queue = WorkQueueManager::with_max_retries(&conn, 3);

    queue.mark_failed("A", "boom").unwrap();
    assert_eq!(retry_of(&conn, "A"), 1);
    assert_eq!(state_of(&conn, "A"), "discovered");

    insert_claimed_again(&conn, "A");
    queue.mark_failed("A", "boom").unwrap();
    assert_eq!(retry_of(&conn, "A"), 2);
    assert_eq!(state_of(&conn, "A"), "discovered");

    insert_claimed_again(&conn, "A");
    queue.mark_failed("A", "boom").unwrap();
    assert_eq!(retry_of(&conn, "A"), 3);
    assert_eq!(state_of(&conn, "A"), "failed");
}

/// Reset an article to `claimed` (mark_failed's retry path moves it back to
/// discovered; the next attempt would have re-claimed it).
fn insert_claimed_again(conn: &Connection<'_>, title: &str) {
    conn.run_params(
        "MATCH (a:Article {title: $title}) SET a.expansion_state = 'claimed'",
        vec![("title", Value::String(title.to_string()))],
    )
    .unwrap();
}

#[test]
fn mark_failed_on_missing_article_is_noop() {
    let db = store();
    let conn = db.connect().unwrap();
    let queue = WorkQueueManager::new(&conn);
    queue.mark_failed("Ghost", "boom").unwrap(); // does not error
}

#[test]
fn reclaim_stale_resets_old_claims_only() {
    let db = store();
    let conn = db.connect().unwrap();
    insert(&conn, "Stale", "claimed", 0, Some(0), 0); // claimed at epoch 0
    insert(&conn, "Fresh", "claimed", 0, Some(now_ms()), 0);
    let queue = WorkQueueManager::new(&conn);

    let reclaimed = queue.reclaim_stale(300).unwrap();
    assert_eq!(reclaimed, 1);
    assert_eq!(state_of(&conn, "Stale"), "discovered");
    assert_eq!(state_of(&conn, "Fresh"), "claimed");
}

#[test]
fn update_heartbeat_refreshes_claimed_at() {
    let db = store();
    let conn = db.connect().unwrap();
    insert(&conn, "A", "claimed", 0, Some(0), 0);
    let queue = WorkQueueManager::new(&conn);
    queue.update_heartbeat("A").unwrap();

    // A claimed_at of 0 would be stale under any positive timeout; after the
    // heartbeat it is fresh and no longer reclaimed.
    assert_eq!(queue.reclaim_stale(300).unwrap(), 0);
    assert_eq!(state_of(&conn, "A"), "claimed");
}

#[test]
fn get_queue_stats_counts_by_state() {
    let db = store();
    let conn = db.connect().unwrap();
    insert(&conn, "d1", "discovered", 0, None, 0);
    insert(&conn, "d2", "discovered", 0, None, 0);
    insert(&conn, "c1", "claimed", 0, Some(now_ms()), 0);
    insert(&conn, "l1", "loaded", 0, None, 0);
    insert(&conn, "p1", "processed", 0, None, 0);
    insert(&conn, "f1", "failed", 0, None, 0);

    let stats = WorkQueueManager::new(&conn).get_queue_stats().unwrap();
    assert_eq!(stats.discovered, 2);
    assert_eq!(stats.claimed, 1);
    assert_eq!(stats.loaded, 1);
    assert_eq!(stats.processed, 1);
    assert_eq!(stats.failed, 1);
    assert_eq!(stats.total, 6);
}
