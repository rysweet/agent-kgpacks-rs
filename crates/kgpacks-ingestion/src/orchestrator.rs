//! Expansion orchestrator.
//!
//! Rust port of `bootstrap/src/expansion/orchestrator.py`. The heart of the
//! module is [`process_one`], the per-article step that drives one claimed
//! article through processing, link discovery, and the work-queue state
//! machine. It is written against the [`Processor`], [`WorkQueue`], and
//! [`LinkDiscoverer`] traits so it can be exercised with mocks and no database
//! — the subject of the `orchestrator process-one` parity test. The concrete
//! pipeline components ([`crate::ArticleProcessor`], [`crate::WorkQueueManager`],
//! [`crate::LinkDiscovery`]) implement those traits.

use kgpacks_db::{Connection, Value};
use kgpacks_embeddings::EmbeddingModel;

use crate::content::ContentSource;
use crate::error::Result;
use crate::extraction::Extractor;
use crate::link_discovery::LinkDiscovery;
use crate::processor::ArticleProcessor;
use crate::schema::apply_ingestion_schema_with_dim;
use crate::util::now_ms;
use crate::work_queue::{QueueStats, WorkQueueManager};

/// Outcome of processing a single article (parity with the
/// `(success, links, error)` tuple returned by `process_article`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutcome {
    /// Whether the article was loaded successfully.
    pub success: bool,
    /// Outgoing link targets (for expansion).
    pub links: Vec<String>,
    /// Error message when `success == false`.
    pub error: Option<String>,
}

impl ProcessOutcome {
    /// A successful outcome carrying the article's outgoing `links`.
    pub fn success(links: Vec<String>) -> Self {
        Self {
            success: true,
            links,
            error: None,
        }
    }

    /// A failed outcome carrying an `error` message and no links.
    pub fn failure(error: String) -> Self {
        Self {
            success: false,
            links: Vec::new(),
            error: Some(error),
        }
    }
}

/// Processes a single article (the [`crate::ArticleProcessor`] abstraction).
pub trait Processor {
    /// Fetch, parse, embed, and load one article.
    fn process_article(
        &self,
        title_or_url: &str,
        category: &str,
        expansion_depth: i64,
    ) -> ProcessOutcome;
}

/// The work-queue operations [`process_one`] needs (the
/// [`crate::WorkQueueManager`] abstraction).
pub trait WorkQueue {
    /// Refresh the heartbeat for a claimed article.
    fn update_heartbeat(&self, title: &str) -> Result<()>;
    /// Advance an article to a new state.
    fn advance_state(&self, title: &str, new_state: &str) -> Result<()>;
    /// Record a processing failure (retry or fail).
    fn mark_failed(&self, title: &str, error: &str) -> Result<()>;
}

/// Link discovery as [`process_one`] needs it (the [`crate::LinkDiscovery`]
/// abstraction).
pub trait LinkDiscoverer {
    /// Discover and link new articles from a source's outgoing links.
    fn discover_links(
        &self,
        source_title: &str,
        links: &[String],
        current_depth: i64,
        max_depth: i64,
    ) -> Result<usize>;
}

/// A unit of work claimed from the queue (parity with the `article_info` dict).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleInfo {
    /// Article title.
    pub title: String,
    /// Expansion depth.
    pub expansion_depth: i64,
    /// Optional category; defaults to `"General"` when absent.
    pub category: Option<String>,
}

impl ArticleInfo {
    /// Build an `ArticleInfo` with no explicit category.
    pub fn new(title: impl Into<String>, expansion_depth: i64) -> Self {
        Self {
            title: title.into(),
            expansion_depth,
            category: None,
        }
    }

    /// Set the category.
    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }
}

/// Process one claimed article: heartbeat, process, discover links (when below
/// `max_depth`), then advance to `processed` — or `mark_failed` on failure.
///
/// Returns `(title, success, error)`. The category defaults to `"General"` when
/// the [`ArticleInfo`] has none, and link discovery is skipped at or above
/// `max_depth`. Errors from link discovery and the state-machine writes
/// (`advance_state` / `mark_failed`) propagate, mirroring how those exceptions
/// escape `RyuGraphOrchestrator._process_one`; the heartbeat is best-effort
/// (the reference's `update_heartbeat` logs and swallows internally).
pub fn process_one(
    article: &ArticleInfo,
    max_depth: i64,
    processor: &dyn Processor,
    queue: &dyn WorkQueue,
    link_discovery: &dyn LinkDiscoverer,
) -> Result<(String, bool, Option<String>)> {
    let title = article.title.clone();
    let depth = article.expansion_depth;
    let category = article
        .category
        .clone()
        .unwrap_or_else(|| "General".to_string());

    // Heartbeat before doing the (potentially slow) work; best-effort.
    let _ = queue.update_heartbeat(&title);

    let outcome = processor.process_article(&title, &category, depth);

    if outcome.success {
        if depth < max_depth {
            link_discovery.discover_links(&title, &outcome.links, depth, max_depth)?;
        }
        // The processor already set `loaded`; advance to `processed`.
        queue.advance_state(&title, "processed")?;
    } else {
        let error = outcome
            .error
            .clone()
            .unwrap_or_else(|| "Unknown error".to_string());
        queue.mark_failed(&title, &error)?;
    }

    Ok((title, outcome.success, outcome.error))
}

// ── Concrete trait impls wiring the pipeline components into `process_one` ───

impl Processor for ArticleProcessor<'_, '_> {
    fn process_article(
        &self,
        title_or_url: &str,
        category: &str,
        expansion_depth: i64,
    ) -> ProcessOutcome {
        ArticleProcessor::process_article(self, title_or_url, category, expansion_depth)
    }
}

impl WorkQueue for WorkQueueManager<'_, '_> {
    fn update_heartbeat(&self, title: &str) -> Result<()> {
        WorkQueueManager::update_heartbeat(self, title)
    }
    fn advance_state(&self, title: &str, new_state: &str) -> Result<()> {
        WorkQueueManager::advance_state(self, title, new_state)
    }
    fn mark_failed(&self, title: &str, error: &str) -> Result<()> {
        WorkQueueManager::mark_failed(self, title, error)
    }
}

impl LinkDiscoverer for LinkDiscovery<'_, '_> {
    fn discover_links(
        &self,
        source_title: &str,
        links: &[String],
        current_depth: i64,
        max_depth: i64,
    ) -> Result<usize> {
        LinkDiscovery::discover_links(self, source_title, links, current_depth, max_depth)
    }
}

/// Tuning for an expansion run (parity with the `RyuGraphOrchestrator` ctor args).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpansionConfig {
    /// Maximum expansion depth from the seeds.
    pub max_depth: i64,
    /// Articles claimed per batch.
    pub batch_size: i64,
    /// Stale-claim reclaim timeout, in seconds.
    pub claim_timeout_seconds: i64,
    /// Embedding column width of the working store.
    pub embedding_dim: usize,
}

impl Default for ExpansionConfig {
    fn default() -> Self {
        Self {
            max_depth: 2,
            batch_size: 10,
            claim_timeout_seconds: 300,
            embedding_dim: crate::schema::DEFAULT_EMBEDDING_DIM,
        }
    }
}

/// Coordinates knowledge-graph expansion over a working store.
///
/// Owns the [`kgpacks_db::Database`], applies the working-store schema on
/// [`open`](Orchestrator::open), and exposes seed initialization, status, and
/// the per-article step. The fetch/embed/extract collaborators are supplied per
/// call so the orchestrator stays agnostic to the content source and model.
pub struct Orchestrator {
    db: kgpacks_db::Database,
    config: ExpansionConfig,
}

impl Orchestrator {
    /// Open (creating if absent) the working store at `path` and apply the
    /// ingestion schema.
    pub fn open(path: impl AsRef<std::path::Path>, config: ExpansionConfig) -> Result<Self> {
        let db = kgpacks_db::Database::open(path)?;
        {
            let conn = db.connect()?;
            apply_ingestion_schema_with_dim(&conn, config.embedding_dim)?;
        }
        Ok(Self { db, config })
    }

    /// Open an in-memory working store (useful for tests) and apply the schema.
    pub fn in_memory(config: ExpansionConfig) -> Result<Self> {
        let db = kgpacks_db::Database::in_memory()?;
        {
            let conn = db.connect()?;
            apply_ingestion_schema_with_dim(&conn, config.embedding_dim)?;
        }
        Ok(Self { db, config })
    }

    /// The expansion configuration.
    pub fn config(&self) -> &ExpansionConfig {
        &self.config
    }

    /// A fresh connection to the working store.
    pub fn connect(&self) -> Result<Connection<'_>> {
        Ok(self.db.connect()?)
    }

    /// Insert seed articles at depth 0 in the `discovered` state, skipping any
    /// that already exist. Parity with `initialize_seeds`.
    pub fn initialize_seeds(&self, seed_titles: &[String], category: &str) -> Result<()> {
        let conn = self.connect()?;
        for title in seed_titles {
            let exists = !conn
                .run_params(
                    "MATCH (a:Article {title: $title}) RETURN a.title AS title",
                    vec![("title", Value::String(title.clone()))],
                )?
                .is_empty();
            if exists {
                continue;
            }
            conn.run_params(
                "CREATE (:Article {\
                    title: $title, category: $category, word_count: 0, \
                    expansion_state: 'discovered', expansion_depth: 0, \
                    claimed_at: NULL, processed_at: NULL, retry_count: 0})",
                vec![
                    ("title", Value::String(title.clone())),
                    ("category", Value::String(category.to_string())),
                ],
            )?;
        }
        Ok(())
    }

    /// Current queue statistics. Parity with `get_status`.
    pub fn get_status(&self) -> Result<QueueStats> {
        let conn = self.connect()?;
        WorkQueueManager::new(&conn).get_queue_stats()
    }

    /// Process one claimed article using freshly-bound pipeline components on
    /// `conn`, mirroring how `_process_one` constructs per-worker components.
    /// Returns `(title, success, error)`.
    pub fn process_claimed(
        &self,
        conn: &Connection<'_>,
        article: &ArticleInfo,
        content_source: &dyn ContentSource,
        embedder: &dyn EmbeddingModel,
        extractor: Option<&dyn Extractor>,
    ) -> Result<(String, bool, Option<String>)> {
        let processor = match extractor {
            Some(extractor) => {
                ArticleProcessor::new(conn, content_source, embedder).with_extractor(extractor)
            }
            None => ArticleProcessor::new(conn, content_source, embedder),
        };
        let queue = WorkQueueManager::new(conn);
        let link_discovery = LinkDiscovery::new(conn);
        process_one(
            article,
            self.config.max_depth,
            &processor,
            &queue,
            &link_discovery,
        )
    }

    /// Reclaim stale claims older than the configured timeout. Returns the count.
    pub fn reclaim_stale(&self) -> Result<usize> {
        let conn = self.connect()?;
        WorkQueueManager::new(&conn).reclaim_stale(self.config.claim_timeout_seconds)
    }
}

/// A monotonically-increasing session identifier derived from the wall clock,
/// used to tag an expansion run (parity with `initialize_seeds`' session id).
pub fn new_session_id() -> String {
    format!("{:08x}", now_ms() as u64 & 0xffff_ffff)
}
