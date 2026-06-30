//! Parity tests for `process_one`,
//! mirroring `bootstrap/src/expansion/tests/test_orchestrator_process_one.py`.
//!
//! The Python test guards a keyword-argument bug (`title=` vs `title_or_url=`).
//! In Rust the call is type-checked, so the equivalent contract is verified by
//! recording the arguments forwarded to a mock processor, plus the heartbeat /
//! advance / mark_failed / discover_links control flow.

use std::cell::RefCell;

use kgpacks_ingestion::{
    process_one, ArticleInfo, LinkDiscoverer, ProcessOutcome, Processor, Result, WorkQueue,
};

struct MockProcessor {
    outcome: ProcessOutcome,
    calls: RefCell<Vec<(String, String, i64)>>,
}

impl MockProcessor {
    fn returning(outcome: ProcessOutcome) -> Self {
        Self {
            outcome,
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl Processor for MockProcessor {
    fn process_article(
        &self,
        title_or_url: &str,
        category: &str,
        expansion_depth: i64,
    ) -> ProcessOutcome {
        self.calls.borrow_mut().push((
            title_or_url.to_string(),
            category.to_string(),
            expansion_depth,
        ));
        self.outcome.clone()
    }
}

#[derive(Default)]
struct MockQueue {
    heartbeats: RefCell<Vec<String>>,
    advances: RefCell<Vec<(String, String)>>,
    failures: RefCell<Vec<(String, String)>>,
}

impl WorkQueue for MockQueue {
    fn update_heartbeat(&self, title: &str) -> Result<()> {
        self.heartbeats.borrow_mut().push(title.to_string());
        Ok(())
    }
    fn advance_state(&self, title: &str, new_state: &str) -> Result<()> {
        self.advances
            .borrow_mut()
            .push((title.to_string(), new_state.to_string()));
        Ok(())
    }
    fn mark_failed(&self, title: &str, error: &str) -> Result<()> {
        self.failures
            .borrow_mut()
            .push((title.to_string(), error.to_string()));
        Ok(())
    }
}

/// One recorded `discover_links` call: `(source, links, current_depth, max_depth)`.
type LinkCall = (String, Vec<String>, i64, i64);

#[derive(Default)]
struct MockLinks {
    calls: RefCell<Vec<LinkCall>>,
}

impl LinkDiscoverer for MockLinks {
    fn discover_links(
        &self,
        source_title: &str,
        links: &[String],
        current_depth: i64,
        max_depth: i64,
    ) -> Result<usize> {
        self.calls.borrow_mut().push((
            source_title.to_string(),
            links.to_vec(),
            current_depth,
            max_depth,
        ));
        Ok(0)
    }
}

fn article(title: &str, depth: i64, category: Option<&str>) -> ArticleInfo {
    let info = ArticleInfo::new(title, depth);
    match category {
        Some(c) => info.with_category(c),
        None => info,
    }
}

#[test]
fn forwards_title_category_and_depth_to_the_processor() {
    let processor = MockProcessor::returning(ProcessOutcome::success(vec!["Link A".to_string()]));
    let queue = MockQueue::default();
    let links = MockLinks::default();

    process_one(
        &article("Turing completeness", 1, Some("Science")),
        2,
        &processor,
        &queue,
        &links,
    )
    .unwrap();

    let calls = processor.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0],
        ("Turing completeness".to_string(), "Science".to_string(), 1)
    );
}

#[test]
fn category_defaults_to_general_when_absent() {
    let processor = MockProcessor::returning(ProcessOutcome::success(vec![]));
    let queue = MockQueue::default();
    let links = MockLinks::default();

    process_one(
        &article("Recursion", 0, None),
        2,
        &processor,
        &queue,
        &links,
    )
    .unwrap();

    assert_eq!(processor.calls.borrow()[0].1, "General");
}

#[test]
fn success_path_heartbeats_discovers_and_advances_to_processed() {
    let processor = MockProcessor::returning(ProcessOutcome::success(vec![
        "L1".to_string(),
        "L2".to_string(),
    ]));
    let queue = MockQueue::default();
    let links = MockLinks::default();

    let (title, success, error) = process_one(
        &article("Graph theory", 0, None),
        2,
        &processor,
        &queue,
        &links,
    )
    .unwrap();

    assert_eq!(title, "Graph theory");
    assert!(success);
    assert_eq!(error, None);
    assert_eq!(queue.heartbeats.borrow().as_slice(), ["Graph theory"]);
    assert_eq!(
        queue.advances.borrow().as_slice(),
        [("Graph theory".to_string(), "processed".to_string())]
    );
    assert_eq!(links.calls.borrow().len(), 1);
    assert!(queue.failures.borrow().is_empty());
}

#[test]
fn link_discovery_skipped_at_max_depth() {
    let processor = MockProcessor::returning(ProcessOutcome::success(vec!["X".to_string()]));
    let queue = MockQueue::default();
    let links = MockLinks::default();

    // depth == max_depth: no further discovery, but still advanced to processed.
    process_one(
        &article("Fibonacci", 2, None),
        2,
        &processor,
        &queue,
        &links,
    )
    .unwrap();

    assert!(links.calls.borrow().is_empty());
    assert_eq!(
        queue.advances.borrow().as_slice(),
        [("Fibonacci".to_string(), "processed".to_string())]
    );
}

#[test]
fn failure_path_marks_failed_and_does_not_advance() {
    let processor = MockProcessor::returning(ProcessOutcome::failure("fetch error".to_string()));
    let queue = MockQueue::default();
    let links = MockLinks::default();

    let (title, success, error) = process_one(
        &article("Failing Article", 0, None),
        2,
        &processor,
        &queue,
        &links,
    )
    .unwrap();

    assert_eq!(title, "Failing Article");
    assert!(!success);
    assert_eq!(error.as_deref(), Some("fetch error"));
    assert_eq!(
        queue.failures.borrow().as_slice(),
        [("Failing Article".to_string(), "fetch error".to_string())]
    );
    assert!(queue.advances.borrow().is_empty());
    assert!(links.calls.borrow().is_empty());
}

#[test]
fn failure_without_message_marks_failed_with_unknown_error() {
    let processor = MockProcessor::returning(ProcessOutcome {
        success: false,
        links: vec![],
        error: None,
    });
    let queue = MockQueue::default();
    let links = MockLinks::default();

    process_one(&article("Mystery", 0, None), 2, &processor, &queue, &links).unwrap();

    assert_eq!(
        queue.failures.borrow().as_slice(),
        [("Mystery".to_string(), "Unknown error".to_string())]
    );
}
