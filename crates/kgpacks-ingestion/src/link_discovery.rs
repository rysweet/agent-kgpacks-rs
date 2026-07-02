//! Link discovery for graph expansion.
//!
//! Rust port of `bootstrap/src/expansion/link_discovery.py`. Discovers new
//! articles from the links of an already-processed article, creating `LINKS_TO`
//! relationships and respecting the maximum expansion depth. Operates over a
//! [`kgpacks_db::Connection`] using bound parameters throughout.

use std::collections::{HashMap, HashSet};

use kgpacks_db::{Connection, LogicalType, Value};

use crate::error::Result;
use crate::util::{value_as_i64, value_as_string};

/// Article-expansion states whose existence means "do not re-discover; just
/// link" (parity with the reference membership check).
const EXISTING_STATES: [&str; 4] = ["loaded", "claimed", "discovered", "processed"];

/// Namespace prefixes (lowercased) that are filtered out of expansion.
const INVALID_PREFIXES: [&str; 15] = [
    "wikipedia:",
    "help:",
    "template:",
    "file:",
    "image:",
    "category:",
    "portal:",
    "talk:",
    "user:",
    "mediawiki:",
    "special:",
    "draft:",
    "module:",
    "book:",
    "timedtext:",
];

/// Discovers new articles from links for graph expansion.
pub struct LinkDiscovery<'c, 'db> {
    conn: &'c Connection<'db>,
}

impl<'c, 'db> LinkDiscovery<'c, 'db> {
    /// Bind link discovery to an open connection.
    pub fn new(conn: &'c Connection<'db>) -> Self {
        Self { conn }
    }

    /// Discover new articles from `links` emitted by `source_title`.
    ///
    /// Filters invalid links, then for each valid link: if the target already
    /// exists, ensures a single `LINKS_TO` edge from the source; if it is new,
    /// inserts it as `discovered` at `current_depth + 1` and links it. Returns
    /// the number of newly discovered articles. No expansion happens once
    /// `current_depth >= max_depth`. Parity with `discover_links`.
    pub fn discover_links(
        &self,
        source_title: &str,
        links: &[String],
        current_depth: i64,
        max_depth: i64,
    ) -> Result<usize> {
        if current_depth >= max_depth {
            return Ok(0);
        }
        let next_depth = current_depth + 1;
        let mut new_articles = 0usize;

        let valid_links: Vec<&str> = links
            .iter()
            .filter(|l| Self::is_valid_link(l))
            .map(String::as_str)
            .collect();

        // Batch the existence check and the existing-edge set (avoids N+1).
        let existing_articles = self.batch_article_exists(&valid_links)?;
        let existing_links = self.existing_links(source_title)?;

        for link in valid_links {
            match existing_articles.get(link) {
                Some(state) if EXISTING_STATES.contains(&state.as_str()) => {
                    if !existing_links.contains(link) {
                        // Per-link best-effort, like the reference's
                        // `try: … except: continue` around each link.
                        let _ = self.create_link(source_title, link);
                    }
                }
                Some(_) => {
                    // Exists in some other state — leave it alone.
                }
                None => {
                    // New article: insert as discovered. A primary-key clash means
                    // another worker discovered it first; treat that as benign.
                    if self.insert_discovered_article(link, next_depth).is_ok() {
                        new_articles += 1;
                    }
                    let _ = self.create_link(source_title, link);
                }
            }
        }

        Ok(new_articles)
    }

    /// Whether `title` is a valid article link for expansion.
    ///
    /// Filters out namespace/meta pages (`Wikipedia:`, `Help:`, `File:`, …),
    /// `List of …` pages, `(disambiguation)` pages, and titles shorter than two
    /// characters. Parity with `_is_valid_link`.
    pub fn is_valid_link(title: &str) -> bool {
        if title.chars().count() < 2 {
            return false;
        }
        let lower = title.to_lowercase();
        if INVALID_PREFIXES.iter().any(|p| lower.starts_with(p)) {
            return false;
        }
        if title.starts_with("List of ") {
            return false;
        }
        !title.contains("(disambiguation)")
    }

    /// Check whether an article exists, returning `(exists, state)`.
    /// Parity with `article_exists`.
    pub fn article_exists(&self, title: &str) -> Result<(bool, Option<String>)> {
        let rows = self.conn.run_params(
            "MATCH (a:Article {title: $title}) RETURN a.expansion_state AS state",
            vec![("title", Value::String(title.to_string()))],
        )?;
        match rows.first() {
            Some(row) => Ok((true, value_as_string(row.get("state")))),
            None => Ok((false, None)),
        }
    }

    /// Count articles in the `discovered` state. Parity with `get_discovered_count`.
    pub fn get_discovered_count(&self) -> Result<i64> {
        let rows = self.conn.run(
            "MATCH (a:Article) WHERE a.expansion_state = 'discovered' RETURN COUNT(a) AS count",
        )?;
        Ok(rows
            .first()
            .and_then(|r| value_as_i64(r.get("count")))
            .unwrap_or(0))
    }

    /// Existence + state for many titles in a single query. Titles absent from
    /// the returned map do not exist. Parity with `_batch_article_exists`.
    fn batch_article_exists(&self, titles: &[&str]) -> Result<HashMap<String, String>> {
        if titles.is_empty() {
            return Ok(HashMap::new());
        }
        let list = Value::List(
            LogicalType::String,
            titles
                .iter()
                .map(|t| Value::String((*t).to_string()))
                .collect(),
        );
        let rows = self.conn.run_params(
            "MATCH (a:Article) WHERE a.title IN $titles \
             RETURN a.title AS title, a.expansion_state AS state",
            vec![("titles", list)],
        )?;
        let mut map = HashMap::new();
        for row in rows {
            if let (Some(title), Some(state)) = (
                value_as_string(row.get("title")),
                value_as_string(row.get("state")),
            ) {
                map.insert(title, state);
            }
        }
        Ok(map)
    }

    /// Insert a new article in the `discovered` state at `depth`.
    /// Parity with `_insert_discovered_article`.
    fn insert_discovered_article(&self, title: &str, depth: i64) -> Result<()> {
        self.conn.run_params(
            "CREATE (:Article {\
                title: $title, \
                category: NULL, \
                word_count: 0, \
                expansion_state: 'discovered', \
                expansion_depth: $depth, \
                claimed_at: NULL, \
                processed_at: NULL, \
                retry_count: 0})",
            vec![
                ("title", Value::String(title.to_string())),
                ("depth", Value::Int64(depth)),
            ],
        )?;
        Ok(())
    }

    /// All existing `LINKS_TO` targets from `source_title`, in one query.
    /// Parity with `_get_existing_links`.
    fn existing_links(&self, source_title: &str) -> Result<HashSet<String>> {
        let rows = self.conn.run_params(
            "MATCH (source:Article {title: $source})-[:LINKS_TO]->(target:Article) \
             RETURN target.title AS title",
            vec![("source", Value::String(source_title.to_string()))],
        )?;
        Ok(rows
            .iter()
            .filter_map(|r| value_as_string(r.get("title")))
            .collect())
    }

    /// Create a `LINKS_TO` edge from `source_title` to `target_title`.
    /// Parity with `_create_link`.
    fn create_link(&self, source_title: &str, target_title: &str) -> Result<()> {
        self.conn.run_params(
            "MATCH (source:Article {title: $source}), (target:Article {title: $target}) \
             CREATE (source)-[:LINKS_TO {link_type: 'internal'}]->(target)",
            vec![
                ("source", Value::String(source_title.to_string())),
                ("target", Value::String(target_title.to_string())),
            ],
        )?;
        Ok(())
    }
}
