//! Work-queue manager for article expansion.
//!
//! Rust port of `bootstrap/src/expansion/work_queue.py`. Coordinates
//! claim/heartbeat/reclaim and the expansion state machine over a
//! [`kgpacks_db::Connection`]:
//!
//! ```text
//! discovered -> claimed -> loaded -> processed   (success path)
//! claimed -> discovered                          (retry / stale reclaim)
//! claimed -> failed                              (after max retries)
//! ```
//!
//! Timestamps (`claimed_at`) are `INT64` epoch-millis (see [`crate::schema`]),
//! so stale-claim detection is a plain integer comparison.

use kgpacks_db::{Connection, LogicalType, Value};

use crate::error::{IngestionError, Result};
use crate::util::{now_ms, value_as_i64, value_as_string};

/// Every legal expansion state (parity with `VALID_STATES`).
pub const VALID_STATES: [&str; 5] = ["discovered", "claimed", "loaded", "processed", "failed"];

/// A batch entry returned by [`WorkQueueManager::claim_work`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedArticle {
    /// Article title.
    pub title: String,
    /// Expansion depth of the article.
    pub expansion_depth: i64,
    /// Epoch-millis timestamp the claim was taken.
    pub claimed_at: i64,
}

/// Counts of articles by state (parity with the `get_queue_stats` dict).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueueStats {
    /// Articles awaiting a claim.
    pub discovered: i64,
    /// Articles currently claimed by a worker.
    pub claimed: i64,
    /// Articles with content loaded.
    pub loaded: i64,
    /// Articles fully processed (loaded + links discovered).
    pub processed: i64,
    /// Articles that exhausted their retries.
    pub failed: i64,
    /// Total non-null-state articles.
    pub total: i64,
}

/// Legal predecessor states for each target state (parity with
/// `_VALID_PREDECESSORS`). A target absent from this table may be set from any
/// state.
fn valid_predecessors(new_state: &str) -> &'static [&'static str] {
    match new_state {
        "claimed" => &["discovered"],
        "loaded" => &["claimed"],
        "processed" => &["loaded", "claimed"],
        "failed" => &["claimed", "discovered"],
        "discovered" => &["claimed", "failed"],
        _ => &[],
    }
}

/// Manages the work queue for distributed article processing.
pub struct WorkQueueManager<'c, 'db> {
    conn: &'c Connection<'db>,
    max_retries: i64,
}

impl<'c, 'db> WorkQueueManager<'c, 'db> {
    /// Bind a work-queue manager to a connection with the default retry budget (3).
    pub fn new(conn: &'c Connection<'db>) -> Self {
        Self::with_max_retries(conn, 3)
    }

    /// Bind a work-queue manager with an explicit `max_retries` budget.
    pub fn with_max_retries(conn: &'c Connection<'db>, max_retries: i64) -> Self {
        Self { conn, max_retries }
    }

    /// Claim up to `batch_size` `discovered` articles (lowest depth first),
    /// transitioning each to `claimed` under a `WHERE` guard so a lost race
    /// simply yields nothing for that article. Parity with `claim_work`.
    pub fn claim_work(&self, batch_size: i64) -> Result<Vec<ClaimedArticle>> {
        let candidates = self.conn.run_params(
            "MATCH (a:Article) WHERE a.expansion_state = 'discovered' \
             RETURN a.title AS title, a.expansion_depth AS expansion_depth \
             ORDER BY a.expansion_depth ASC LIMIT $batch_size",
            vec![("batch_size", Value::Int64(batch_size))],
        )?;

        let mut claimed = Vec::new();
        for row in candidates {
            let Some(title) = value_as_string(row.get("title")) else {
                continue;
            };
            let depth = value_as_i64(row.get("expansion_depth")).unwrap_or(0);
            let now = now_ms();
            let updated = self.conn.run_params(
                "MATCH (a:Article {title: $title}) WHERE a.expansion_state = 'discovered' \
                 SET a.expansion_state = 'claimed', a.claimed_at = $now \
                 RETURN a.title AS title",
                vec![
                    ("title", Value::String(title.clone())),
                    ("now", Value::Int64(now)),
                ],
            )?;
            if updated.is_empty() {
                continue; // lost the race
            }
            claimed.push(ClaimedArticle {
                title,
                expansion_depth: depth,
                claimed_at: now,
            });
        }
        Ok(claimed)
    }

    /// Refresh the heartbeat (`claimed_at`) of a claimed article so it is not
    /// reclaimed while being processed. Parity with `update_heartbeat`.
    pub fn update_heartbeat(&self, article_title: &str) -> Result<()> {
        self.conn.run_params(
            "MATCH (a:Article {title: $title}) WHERE a.expansion_state = 'claimed' \
             SET a.claimed_at = $now",
            vec![
                ("title", Value::String(article_title.to_string())),
                ("now", Value::Int64(now_ms())),
            ],
        )?;
        Ok(())
    }

    /// Reclaim claimed articles whose `claimed_at` is older than
    /// `timeout_seconds`, resetting them to `discovered`. Returns the count
    /// reclaimed. Parity with `reclaim_stale`.
    pub fn reclaim_stale(&self, timeout_seconds: i64) -> Result<usize> {
        let cutoff = now_ms() - timeout_seconds * 1000;
        let stale = self.conn.run_params(
            "MATCH (a:Article) \
             WHERE a.expansion_state = 'claimed' AND a.claimed_at < $cutoff \
             RETURN a.title AS title",
            vec![("cutoff", Value::Int64(cutoff))],
        )?;

        let mut reclaimed = 0usize;
        for row in stale {
            let Some(title) = value_as_string(row.get("title")) else {
                continue;
            };
            self.conn.run_params(
                "MATCH (a:Article {title: $title}) WHERE a.expansion_state = 'claimed' \
                 SET a.expansion_state = 'discovered', a.claimed_at = NULL",
                vec![("title", Value::String(title))],
            )?;
            reclaimed += 1;
        }
        Ok(reclaimed)
    }

    /// Advance an article to `new_state`, guarded by the legal predecessor set.
    ///
    /// Returns [`IngestionError::InvalidState`] if `new_state` is not one of
    /// [`VALID_STATES`]. Parity with `advance_state`.
    pub fn advance_state(&self, article_title: &str, new_state: &str) -> Result<()> {
        if !VALID_STATES.contains(&new_state) {
            return Err(IngestionError::InvalidState(new_state.to_string()));
        }
        let predecessors = valid_predecessors(new_state);
        let now = now_ms();
        if predecessors.is_empty() {
            self.conn.run_params(
                "MATCH (a:Article {title: $title}) \
                 SET a.expansion_state = $new_state, a.processed_at = $now",
                vec![
                    ("title", Value::String(article_title.to_string())),
                    ("new_state", Value::String(new_state.to_string())),
                    ("now", Value::Int64(now)),
                ],
            )?;
        } else {
            let list = Value::List(
                LogicalType::String,
                predecessors
                    .iter()
                    .map(|s| Value::String((*s).to_string()))
                    .collect(),
            );
            self.conn.run_params(
                "MATCH (a:Article {title: $title}) WHERE a.expansion_state IN $predecessors \
                 SET a.expansion_state = $new_state, a.processed_at = $now",
                vec![
                    ("title", Value::String(article_title.to_string())),
                    ("new_state", Value::String(new_state.to_string())),
                    ("now", Value::Int64(now)),
                    ("predecessors", list),
                ],
            )?;
        }
        Ok(())
    }

    /// Record a processing failure: increment `retry_count`, then mark `failed`
    /// once the retry budget is exhausted or reset to `discovered` to retry.
    /// A missing article is a no-op. Parity with `mark_failed`.
    pub fn mark_failed(&self, article_title: &str, _error: &str) -> Result<()> {
        let rows = self.conn.run_params(
            "MATCH (a:Article {title: $title}) RETURN a.retry_count AS retry_count",
            vec![("title", Value::String(article_title.to_string()))],
        )?;
        let Some(current) = rows
            .first()
            .and_then(|r| value_as_i64(r.get("retry_count")))
        else {
            return Ok(()); // article not found
        };
        let new_retry = current + 1;
        if new_retry >= self.max_retries {
            self.conn.run_params(
                "MATCH (a:Article {title: $title}) \
                 SET a.retry_count = $retry, a.expansion_state = 'failed', a.processed_at = $now",
                vec![
                    ("title", Value::String(article_title.to_string())),
                    ("retry", Value::Int64(new_retry)),
                    ("now", Value::Int64(now_ms())),
                ],
            )?;
        } else {
            self.conn.run_params(
                "MATCH (a:Article {title: $title}) \
                 SET a.retry_count = $retry, a.expansion_state = 'discovered', a.claimed_at = NULL",
                vec![
                    ("title", Value::String(article_title.to_string())),
                    ("retry", Value::Int64(new_retry)),
                ],
            )?;
        }
        Ok(())
    }

    /// Count articles by state. Parity with `get_queue_stats`.
    pub fn get_queue_stats(&self) -> Result<QueueStats> {
        let rows = self.conn.run(
            "MATCH (a:Article) WHERE a.expansion_state IS NOT NULL \
             RETURN a.expansion_state AS state, COUNT(a) AS count",
        )?;
        let mut stats = QueueStats::default();
        for row in rows {
            let count = value_as_i64(row.get("count")).unwrap_or(0);
            match value_as_string(row.get("state")).as_deref() {
                Some("discovered") => stats.discovered = count,
                Some("claimed") => stats.claimed = count,
                Some("loaded") => stats.loaded = count,
                Some("processed") => stats.processed = count,
                Some("failed") => stats.failed = count,
                _ => {}
            }
            stats.total += count;
        }
        Ok(stats)
    }
}
