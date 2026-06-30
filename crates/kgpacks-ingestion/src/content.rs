//! Pluggable content sources.
//!
//! Rust port of `bootstrap/src/sources/base.py`. A [`ContentSource`] fetches a
//! source-agnostic [`Article`], parses it into [`ParsedSection`]s, and extracts
//! outgoing links. Production sources (Wikipedia, web) land later; this module
//! ships the trait plus an in-memory [`MapContentSource`] so the pipeline can be
//! exercised hermetically (no network) in tests and examples.

use std::collections::HashMap;

use regex::Regex;

use crate::error::IngestionError;

/// A source-agnostic article (parity with the reference `Article` dataclass).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Article {
    /// Article title.
    pub title: String,
    /// Raw text content (wikitext for Wikipedia, markdown for web, …).
    pub content: String,
    /// Outgoing link targets (titles for Wikipedia, URLs for web).
    pub links: Vec<String>,
    /// Category labels.
    pub categories: Vec<String>,
    /// Canonical source URL, if any.
    pub source_url: String,
    /// Source kind: `"wikipedia"`, `"web"`, `"memory"`, …
    pub source_type: String,
}

/// A parsed article section (parity with the reference section dict
/// `{title, content, level}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSection {
    /// Section heading (empty for the lead/intro section).
    pub title: String,
    /// Section body text.
    pub content: String,
    /// Heading level (1 for the intro and top-level headings).
    pub level: i64,
}

/// Pluggable content source (parity with the reference `ContentSource` protocol).
pub trait ContentSource {
    /// Fetch an article by title (named sources) or URL (web).
    ///
    /// Returns [`IngestionError::ArticleNotFound`] if the article cannot be found.
    fn fetch_article(&self, title_or_url: &str) -> Result<Article, IngestionError>;

    /// Parse raw article `content` into sections (`title`, `content`, `level`).
    fn parse_sections(&self, content: &str) -> Vec<ParsedSection>;

    /// Extract outgoing link targets from raw article `content`.
    fn get_links(&self, content: &str) -> Vec<String>;
}

/// An in-memory [`ContentSource`] backed by a fixed map of articles.
///
/// Useful for hermetic tests and examples: register articles up front, then run
/// the pipeline against them with no network access. Sections are parsed from
/// markdown-style `#`/`##` headings; links are extracted from `[[Wiki Link]]`
/// markup (in addition to any [`Article::links`] set when the article was
/// registered).
#[derive(Debug, Clone, Default)]
pub struct MapContentSource {
    articles: HashMap<String, Article>,
}

impl MapContentSource {
    /// Create an empty source.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `article`, keyed by its title, returning `self` for chaining.
    pub fn with_article(mut self, article: Article) -> Self {
        self.articles.insert(article.title.clone(), article);
        self
    }

    /// Register `article`, keyed by its title.
    pub fn insert(&mut self, article: Article) {
        self.articles.insert(article.title.clone(), article);
    }
}

impl ContentSource for MapContentSource {
    fn fetch_article(&self, title_or_url: &str) -> Result<Article, IngestionError> {
        self.articles
            .get(title_or_url)
            .cloned()
            .ok_or_else(|| IngestionError::ArticleNotFound(title_or_url.to_string()))
    }

    fn parse_sections(&self, content: &str) -> Vec<ParsedSection> {
        parse_markdown_sections(content)
    }

    fn get_links(&self, content: &str) -> Vec<String> {
        extract_wiki_links(content)
    }
}

/// Parse markdown-ish `content` into sections.
///
/// Lines before the first heading form an intro section (empty title, level 1).
/// A line beginning with one or more `#` followed by a space starts a new
/// section whose level is the number of `#`. Sections with no body text are
/// dropped, mirroring the reference's "skip empty sections" behavior.
fn parse_markdown_sections(content: &str) -> Vec<ParsedSection> {
    let mut sections: Vec<ParsedSection> = Vec::new();
    let mut current_title = String::new();
    let mut current_level: i64 = 1;
    let mut buffer: Vec<&str> = Vec::new();

    let flush = |title: &str, level: i64, buffer: &[&str], out: &mut Vec<ParsedSection>| {
        let body = buffer.join("\n");
        let trimmed = body.trim();
        if !trimmed.is_empty() {
            out.push(ParsedSection {
                title: title.to_string(),
                content: trimmed.to_string(),
                level,
            });
        }
    };

    for line in content.lines() {
        if let Some((level, heading)) = parse_heading(line) {
            flush(&current_title, current_level, &buffer, &mut sections);
            buffer.clear();
            current_title = heading;
            current_level = level;
        } else {
            buffer.push(line);
        }
    }
    flush(&current_title, current_level, &buffer, &mut sections);
    sections
}

/// If `line` is a markdown heading (`#`+ then a space), return `(level, text)`.
fn parse_heading(line: &str) -> Option<(i64, String)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    let rest = &trimmed[hashes..];
    if !rest.starts_with(' ') {
        return None;
    }
    Some((hashes as i64, rest.trim().to_string()))
}

/// Extract `[[Wiki Link]]` targets from `content`, de-duplicated, in order.
fn extract_wiki_links(content: &str) -> Vec<String> {
    // `[[Target]]` or `[[Target|Display]]` -> capture `Target`.
    let re = Regex::new(r"\[\[([^\]|]+)(?:\|[^\]]*)?\]\]").expect("valid wiki-link regex");
    let mut seen = Vec::new();
    for caps in re.captures_iter(content) {
        let target = caps[1].trim().to_string();
        if !target.is_empty() && !seen.contains(&target) {
            seen.push(target);
        }
    }
    seen
}
