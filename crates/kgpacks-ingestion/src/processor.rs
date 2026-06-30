//! Single-article processor.
//!
//! Rust port of `bootstrap/src/expansion/processor.py` (with the structural load
//! of `bootstrap/src/database/loader.py`). Fetches an article from a
//! [`ContentSource`], parses sections, generates embeddings, optionally runs LLM
//! extraction, and loads everything into the working store.
//!
//! Embeddings are generated *and stored* (`Section.embedding` / `Chunk.embedding`
//! arrays); the HNSW vector index over those columns is hybrid-retrieval (M4).

use std::sync::OnceLock;

use kgpacks_db::{Connection, LogicalType, Value};
use kgpacks_embeddings::{chunk_sections, Chunk, EmbeddingModel};
use regex::Regex;

use crate::content::{Article, ContentSource, ParsedSection};
use crate::extraction::{ExtractionResult, Extractor};
use crate::orchestrator::ProcessOutcome;
use crate::util::now_ms;

/// Processes a single article end to end into the working store.
///
/// Holds borrowed collaborators (connection, content source, embedder, optional
/// extractor); construct one per unit of work.
pub struct ArticleProcessor<'a, 'db> {
    conn: &'a Connection<'db>,
    content_source: &'a dyn ContentSource,
    embedder: &'a dyn EmbeddingModel,
    extractor: Option<&'a dyn Extractor>,
}

impl<'a, 'db> ArticleProcessor<'a, 'db> {
    /// Build a processor without LLM extraction.
    pub fn new(
        conn: &'a Connection<'db>,
        content_source: &'a dyn ContentSource,
        embedder: &'a dyn EmbeddingModel,
    ) -> Self {
        Self {
            conn,
            content_source,
            embedder,
            extractor: None,
        }
    }

    /// Attach an LLM extractor, enabling entity/fact/relationship extraction.
    pub fn with_extractor(mut self, extractor: &'a dyn Extractor) -> Self {
        self.extractor = Some(extractor);
        self
    }

    /// Process one article: fetch, parse, embed, optionally extract, and load.
    ///
    /// Returns a [`ProcessOutcome`] of `(success, links, error)`. A not-found
    /// article and a stub article (no sections) are reported, mirroring the
    /// reference's early returns; any other failure yields `success = false`
    /// with a secret-redacted error message. Parity with `process_article`.
    pub fn process_article(
        &self,
        title_or_url: &str,
        category: &str,
        expansion_depth: i64,
    ) -> ProcessOutcome {
        match self.process_inner(title_or_url, category, expansion_depth) {
            Ok(outcome) => outcome,
            Err(err) => {
                ProcessOutcome::failure(sanitize_error(&format!("Processing error: {err}")))
            }
        }
    }

    fn process_inner(
        &self,
        title_or_url: &str,
        category: &str,
        expansion_depth: i64,
    ) -> crate::error::Result<ProcessOutcome> {
        // Step 1: fetch.
        let mut article = match self.content_source.fetch_article(title_or_url) {
            Ok(article) => article,
            Err(crate::error::IngestionError::ArticleNotFound(_)) => {
                return Ok(ProcessOutcome::failure(format!(
                    "Article not found: {title_or_url}"
                )));
            }
            Err(other) => return Err(other),
        };

        // Step 2: follow a single Wikipedia redirect, if present.
        if article.source_type == "wikipedia" {
            if let Some(target) = redirect_target(&article.content) {
                match self.content_source.fetch_article(&target) {
                    Ok(redirected) => article = redirected,
                    Err(crate::error::IngestionError::ArticleNotFound(_)) => {
                        // Unfollowable redirect: not an error, just nothing to load.
                        return Ok(ProcessOutcome::success(Vec::new()));
                    }
                    Err(other) => return Err(other),
                }
            }
        }

        // Parse sections; a stub article (no sections) is a benign skip.
        let sections = self.content_source.parse_sections(&article.content);
        if sections.is_empty() {
            return Ok(ProcessOutcome::success(article.links.clone()));
        }

        // Step 3: embed sections.
        let section_texts: Vec<&str> = sections.iter().map(|s| s.content.as_str()).collect();
        let section_embeddings = self.embedder.generate(&section_texts)?;

        // Step 4: optional LLM extraction (failure here must not fail the load).
        let extraction = self.extractor.map(|extractor| {
            let domain = crate::extraction::detect_domain(&article.categories);
            extractor.extract_from_article(&article.title, &sections, 5, domain.as_deref())
        });

        // Step 5: load everything.
        self.insert_article_with_sections(
            &article,
            &sections,
            &section_embeddings,
            category,
            expansion_depth,
            extraction.as_ref(),
        )?;

        Ok(ProcessOutcome::success(article.links.clone()))
    }

    fn insert_article_with_sections(
        &self,
        article: &Article,
        sections: &[ParsedSection],
        section_embeddings: &[Vec<f32>],
        category: &str,
        expansion_depth: i64,
        extraction: Option<&ExtractionResult>,
    ) -> crate::error::Result<()> {
        let now = now_ms();
        let word_count = article.content.split_whitespace().count() as i64;

        self.upsert_article(article, category, word_count, expansion_depth, now)?;
        self.replace_sections(article, sections, section_embeddings)?;
        self.replace_chunks(article, sections)?;
        self.replace_categories(article)?;
        if let Some(extraction) = extraction {
            self.replace_extraction(article, extraction)?;
        }
        Ok(())
    }

    /// Create the article, or update it in place if it already exists (e.g. a
    /// seed stub). Sets `expansion_state = 'loaded'`.
    fn upsert_article(
        &self,
        article: &Article,
        category: &str,
        word_count: i64,
        expansion_depth: i64,
        now: i64,
    ) -> crate::error::Result<()> {
        let exists = !self
            .conn
            .run_params(
                "MATCH (a:Article {title: $title}) RETURN a.title AS title",
                vec![("title", Value::String(article.title.clone()))],
            )?
            .is_empty();

        if exists {
            self.conn.run_params(
                "MATCH (a:Article {title: $title}) \
                 SET a.word_count = $word_count, a.category = $category, \
                     a.expansion_state = 'loaded', a.processed_at = $now",
                vec![
                    ("title", Value::String(article.title.clone())),
                    ("word_count", Value::Int64(word_count)),
                    ("category", Value::String(category.to_string())),
                    ("now", Value::Int64(now)),
                ],
            )?;
        } else {
            self.conn.run_params(
                "CREATE (:Article {\
                    title: $title, category: $category, word_count: $word_count, \
                    expansion_state: 'loaded', expansion_depth: $depth, \
                    claimed_at: NULL, processed_at: $now, retry_count: 0})",
                vec![
                    ("title", Value::String(article.title.clone())),
                    ("category", Value::String(category.to_string())),
                    ("word_count", Value::Int64(word_count)),
                    ("depth", Value::Int64(expansion_depth)),
                    ("now", Value::Int64(now)),
                ],
            )?;
        }
        Ok(())
    }

    /// Replace all `Section`s of the article (delete then re-insert) so retries
    /// never collide on the section primary key.
    fn replace_sections(
        &self,
        article: &Article,
        sections: &[ParsedSection],
        embeddings: &[Vec<f32>],
    ) -> crate::error::Result<()> {
        self.conn.run_params(
            "MATCH (a:Article {title: $title})-[r:HAS_SECTION]->(s:Section) DELETE r, s",
            vec![("title", Value::String(article.title.clone()))],
        )?;

        for (i, (section, embedding)) in sections.iter().zip(embeddings).enumerate() {
            let section_id = format!("{}#{}", article.title, i);
            let section_word_count = section.content.split_whitespace().count() as i64;
            self.conn.run_params(
                "MATCH (a:Article {title: $article_title}) \
                 CREATE (a)-[:HAS_SECTION {section_index: $index}]->(s:Section {\
                    section_id: $section_id, title: $title, content: $content, \
                    embedding: $embedding, level: $level, word_count: $word_count})",
                vec![
                    ("article_title", Value::String(article.title.clone())),
                    ("section_id", Value::String(section_id)),
                    ("title", Value::String(section.title.clone())),
                    ("content", Value::String(section.content.clone())),
                    ("embedding", embedding_value(embedding)),
                    ("level", Value::Int64(section.level)),
                    ("word_count", Value::Int64(section_word_count)),
                    ("index", Value::Int64(i as i64)),
                ],
            )?;
        }
        Ok(())
    }

    /// Replace all `Chunk`s of the article: chunk every section, embed the
    /// chunks, and re-insert with `HAS_CHUNK` edges.
    fn replace_chunks(
        &self,
        article: &Article,
        sections: &[ParsedSection],
    ) -> crate::error::Result<()> {
        let contents: Vec<&str> = sections.iter().map(|s| s.content.as_str()).collect();
        let chunks: Vec<Chunk> = chunk_sections(&contents, &article.title);
        self.conn.run_params(
            "MATCH (a:Article {title: $title})-[r:HAS_CHUNK]->(c:Chunk) DELETE r, c",
            vec![("title", Value::String(article.title.clone()))],
        )?;
        if chunks.is_empty() {
            return Ok(());
        }
        let chunk_texts: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
        let chunk_embeddings = self.embedder.generate(&chunk_texts)?;

        for (chunk, embedding) in chunks.iter().zip(&chunk_embeddings) {
            self.conn.run_params(
                "MATCH (a:Article {title: $article_title}) \
                 CREATE (a)-[:HAS_CHUNK {section_index: $section_index, chunk_index: $chunk_index}]->\
                 (c:Chunk {chunk_id: $chunk_id, content: $content, embedding: $embedding, \
                    article_title: $article_title, section_index: $section_index, \
                    chunk_index: $chunk_index})",
                vec![
                    ("article_title", Value::String(article.title.clone())),
                    ("chunk_id", Value::String(chunk.chunk_id.clone())),
                    ("content", Value::String(chunk.content.clone())),
                    ("embedding", embedding_value(embedding)),
                    ("section_index", Value::Int64(chunk.section_index as i64)),
                    ("chunk_index", Value::Int64(chunk.chunk_index as i64)),
                ],
            )?;
        }
        Ok(())
    }

    /// Replace the article's category edges, merging each of the first three
    /// categories and incrementing its `article_count`.
    fn replace_categories(&self, article: &Article) -> crate::error::Result<()> {
        self.conn.run_params(
            "MATCH (a:Article {title: $title})-[r:IN_CATEGORY]->() DELETE r",
            vec![("title", Value::String(article.title.clone()))],
        )?;
        for cat in article.categories.iter().take(3) {
            self.conn.run_params(
                "MERGE (c:Category {name: $category}) \
                 ON CREATE SET c.article_count = 1 \
                 ON MATCH SET c.article_count = c.article_count + 1",
                vec![("category", Value::String(cat.clone()))],
            )?;
            self.conn.run_params(
                "MATCH (a:Article {title: $title}), (c:Category {name: $category}) \
                 CREATE (a)-[:IN_CATEGORY]->(c)",
                vec![
                    ("title", Value::String(article.title.clone())),
                    ("category", Value::String(cat.clone())),
                ],
            )?;
        }
        Ok(())
    }

    /// Insert extracted entities, facts, and entity relationships, replacing any
    /// previously extracted data for the article.
    fn replace_extraction(
        &self,
        article: &Article,
        extraction: &ExtractionResult,
    ) -> crate::error::Result<()> {
        self.conn.run_params(
            "MATCH (a:Article {title: $title})-[r:HAS_ENTITY]->(:Entity) DELETE r",
            vec![("title", Value::String(article.title.clone()))],
        )?;
        self.conn.run_params(
            "MATCH (a:Article {title: $title})-[r:HAS_FACT]->(:Fact) DELETE r",
            vec![("title", Value::String(article.title.clone()))],
        )?;

        for entity in &extraction.entities {
            let entity_id = format!("{}|{}", article.title, entity.name);
            let description = entity
                .properties
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            self.conn.run_params(
                "MERGE (e:Entity {entity_id: $entity_id}) \
                 ON CREATE SET e.name = $name, e.type = $type, e.description = $description",
                vec![
                    ("entity_id", Value::String(entity_id.clone())),
                    ("name", Value::String(entity.name.clone())),
                    ("type", Value::String(entity.type_.clone())),
                    ("description", Value::String(description.to_string())),
                ],
            )?;
            self.conn.run_params(
                "MATCH (a:Article {title: $title}), (e:Entity {entity_id: $entity_id}) \
                 CREATE (a)-[:HAS_ENTITY]->(e)",
                vec![
                    ("title", Value::String(article.title.clone())),
                    ("entity_id", Value::String(entity_id)),
                ],
            )?;
        }

        for (i, fact) in extraction.key_facts.iter().enumerate() {
            let fact_id = format!("{}|fact{}", article.title, i);
            self.conn.run_params(
                "MERGE (f:Fact {fact_id: $fact_id}) ON CREATE SET f.content = $content",
                vec![
                    ("fact_id", Value::String(fact_id.clone())),
                    ("content", Value::String(fact.clone())),
                ],
            )?;
            self.conn.run_params(
                "MATCH (a:Article {title: $title}), (f:Fact {fact_id: $fact_id}) \
                 CREATE (a)-[:HAS_FACT]->(f)",
                vec![
                    ("title", Value::String(article.title.clone())),
                    ("fact_id", Value::String(fact_id)),
                ],
            )?;
        }

        for rel in &extraction.relationships {
            let source_id = format!("{}|{}", article.title, rel.source);
            let target_id = format!("{}|{}", article.title, rel.target);
            self.conn.run_params(
                "MATCH (e1:Entity {entity_id: $source_id}), (e2:Entity {entity_id: $target_id}) \
                 CREATE (e1)-[:ENTITY_RELATION {relation: $relation, context: $context}]->(e2)",
                vec![
                    ("source_id", Value::String(source_id)),
                    ("target_id", Value::String(target_id)),
                    ("relation", Value::String(rel.relation.clone())),
                    ("context", Value::String(rel.context.clone())),
                ],
            )?;
        }
        Ok(())
    }
}

/// Convert an embedding into a LadybugDB `DOUBLE[]` array bound parameter.
fn embedding_value(embedding: &[f32]) -> Value {
    Value::Array(
        LogicalType::Double,
        embedding
            .iter()
            .map(|x| Value::Double(f64::from(*x)))
            .collect(),
    )
}

/// If `content` is a Wikipedia redirect, return the redirect target.
/// Parity with the `#REDIRECT [[...]]` handling in `process_article`.
fn redirect_target(content: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)^#REDIRECT\s*\[\[(.+?)\]\]").expect("valid redirect regex")
    });
    re.captures(content.trim_start())
        .map(|c| c[1].trim().to_string())
}

/// Redact secrets (API keys, bearer tokens, JWTs, URL credentials) from an
/// error message before it is surfaced or logged. Parity with `_sanitize_error`.
pub fn sanitize_error(error_msg: &str) -> String {
    static RULES: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    let rules = RULES.get_or_init(|| {
        vec![
            (
                Regex::new(
                    r#"(?i)\b(api[_-]?key|token|secret[_-]?key|bearer|authorization)[=:\s]+['"]?([a-zA-Z0-9_-]{20,128})['"]?"#,
                )
                .expect("valid api-key regex"),
                "$1=***REDACTED***",
            ),
            (
                Regex::new(r#"(['"])(sk-[a-zA-Z0-9_-]{20,128}|[a-zA-Z0-9_-]{30,128})(['"])"#)
                    .expect("valid bare-key regex"),
                "$1***REDACTED***$3",
            ),
            (
                Regex::new(r"eyJ[a-zA-Z0-9_-]{10,}\.[a-zA-Z0-9_-]{10,}\.[a-zA-Z0-9_-]*")
                    .expect("valid jwt regex"),
                "***REDACTED_JWT***",
            ),
            (
                Regex::new(
                    r"(?i)([?&](api[_-]?key|token|secret|access[_-]?token|auth)=)[a-zA-Z0-9_%-]{8,128}",
                )
                .expect("valid url-cred regex"),
                "$1***REDACTED***",
            ),
        ]
    });

    let mut sanitized = error_msg.to_string();
    for (re, replacement) in rules {
        sanitized = re.replace_all(&sanitized, *replacement).into_owned();
    }
    sanitized
}
