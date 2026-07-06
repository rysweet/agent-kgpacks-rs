//! Resumable, pipelined CVE pack build.
//!
//! This is the WS6 build path: it materializes a CVE knowledge pack from a
//! [`CorpusSource`] with two properties the plain [`crate::build_pack`] does not
//! have:
//!
//! * **Checkpoint / resume.** After every committed batch the builder writes a
//!   [`BuildCheckpoint`] sidecar next to the graph store (`<graph>.build-checkpoint.json`)
//!   recording the last committed batch, the corpus offset, running counts and a
//!   stable [`BuildParams::params_hash`]. If a run is interrupted, a later run
//!   with `resume` set continues from the checkpoint — reopening the store,
//!   skipping schema creation, rebuilding its dedup set from the store, and
//!   picking up at the next batch. If the recorded params hash does not match the
//!   current run (any output-affecting input changed) the build starts clean.
//!   The sidecar is removed on a clean finish.
//!
//! * **Pipelined `embed || load`.** Embedding (CPU-bound, parallelizable) runs on
//!   a worker thread while the database load/index (serial, single-writer) runs
//!   on the caller's thread. A **bounded** channel between them overlaps the two
//!   stages while capping how many embedded batches are held in memory at once.
//!
//! The corpus is consumed only through the [`CorpusSource`] seam, so the external
//! CVE-corpus fetch (issue #25) can supply a live source without any change here.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::sync_channel;

use kgpacks_db::{Connection, Database, DatabaseOptions, LogicalType, Value};
use kgpacks_embeddings::{EmbeddingModel, DEFAULT_DIM, DETERMINISTIC_MODEL};

use crate::checkpoint::{checkpoint_path_for, BuildCheckpoint, BuildCounts};
use crate::corpus::{CorpusSource, CveRecord};
use crate::errors::{PacksError, Result};
use crate::manifest::{manifest_path_in, save_manifest, validate_manifest, PackManifest};
use crate::pack::GRAPH_STORE_FILENAME;

/// Category assigned to every CVE `Article` node.
pub const CVE_CATEGORY: &str = "cve";

/// Default batch size when [`BuildParams::batch`] is left at its default.
pub const DEFAULT_BATCH_SIZE: usize = 64;

/// Default number of embedded batches the pipeline may buffer ahead of the load
/// stage (the memory bound). `1` overlaps embed-of-N+1 with load-of-N.
pub const DEFAULT_QUEUE_CAPACITY: usize = 2;

/// The output-affecting inputs of a CVE build.
///
/// [`params_hash`](BuildParams::params_hash) folds these into a stable digest
/// used to decide whether a checkpoint may be resumed: any change here yields a
/// different hash, so a resume against changed parameters restarts cleanly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildParams {
    /// Corpus source identifier (e.g. a path or fetch URL).
    pub src: String,
    /// Publication-year filter: when set, only records whose `published_year`
    /// equals it are loaded (records of other/unknown years are skipped).
    pub year: Option<i64>,
    /// Cap on the number of **corpus records considered** (a prefix of the
    /// corpus); the year filter then applies within that prefix.
    pub limit: Option<usize>,
    /// Records loaded (and checkpointed) per batch.
    pub batch: usize,
    /// Embedding model name.
    pub model: String,
    /// Whether entity-to-entity relations are materialized.
    pub with_entity_relations: bool,
}

impl Default for BuildParams {
    fn default() -> Self {
        Self {
            src: String::new(),
            year: None,
            limit: None,
            batch: DEFAULT_BATCH_SIZE,
            model: DETERMINISTIC_MODEL.to_string(),
            with_entity_relations: false,
        }
    }
}

impl BuildParams {
    /// A stable, key-order-independent hash of the output-affecting inputs.
    ///
    /// Built from a canonical JSON serialization over sorted keys (a `BTreeMap`),
    /// then SHA-256'd, so it is stable across Rust versions and platforms and
    /// changes iff any field changes.
    pub fn params_hash(&self) -> String {
        let mut fields: BTreeMap<&'static str, serde_json::Value> = BTreeMap::new();
        fields.insert("batch", serde_json::json!(self.batch));
        fields.insert("limit", serde_json::json!(self.limit));
        fields.insert("model", serde_json::json!(self.model));
        fields.insert("src", serde_json::json!(self.src));
        fields.insert(
            "with_entity_relations",
            serde_json::json!(self.with_entity_relations),
        );
        fields.insert("year", serde_json::json!(self.year));
        let canonical =
            serde_json::to_string(&fields).expect("BTreeMap<&str, Value> always serializes");
        crate::sha256::hex_digest(canonical.as_bytes())
    }
}

/// Tuning for the pipelined build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineOptions {
    /// Resume from a matching checkpoint if one is present (otherwise a stale or
    /// mismatched checkpoint triggers a clean restart).
    pub resume: bool,
    /// Bound on embedded batches buffered between the embed and load stages.
    pub queue_capacity: usize,
    /// Embedding column width of the store (must match the embedder's `dim()`).
    pub embedding_dim: usize,
    /// Stop after this many committed batches, leaving the checkpoint in place.
    ///
    /// A controlled interruption seam: it models a crash after N committed
    /// batches (for resume tests) and is also useful for staged partial builds.
    /// The check runs before loading each received batch, so `Some(0)` commits
    /// nothing and a value `>=` the total number of batches finishes cleanly
    /// (there is no next batch to interrupt before).
    pub interrupt_after_batches: Option<usize>,
}

impl Default for PipelineOptions {
    fn default() -> Self {
        Self {
            resume: false,
            queue_capacity: DEFAULT_QUEUE_CAPACITY,
            embedding_dim: DEFAULT_DIM,
            interrupt_after_batches: None,
        }
    }
}

/// The result of a [`build_cve_pack`] run.
#[derive(Debug, Clone, PartialEq)]
pub struct CveBuildReport {
    /// Pack directory.
    pub path: PathBuf,
    /// Stable hash of the build parameters.
    pub params_hash: String,
    /// Node/relationship totals in the store after this run.
    pub counts: BuildCounts,
    /// Number of batches committed to the store in total (including any resumed).
    pub batches_committed: usize,
    /// Whether this run resumed from an existing checkpoint.
    pub resumed: bool,
    /// The batch index this run resumed from (`0` when starting fresh).
    pub resumed_from_batch: usize,
    /// Whether the run stopped early via `interrupt_after_batches` (in which case
    /// the checkpoint is retained and no manifest is written).
    pub interrupted: bool,
    /// The written manifest — `Some` only on a clean finish.
    pub manifest: Option<PackManifest>,
}

/// The pack graph schema for a CVE pack (nodes then relationships).
///
/// A CVE record becomes an `Article` (with its embedded `description`); its
/// mentioned products/vendors/etc. become `Entity` nodes linked by `HAS_ENTITY`;
/// entity-to-entity `ENTITY_RELATION` edges are added when the build enables
/// them. Unlike the M2 pack schema, the `Article` carries an `embedding` column
/// so the pipeline's embeddings are persisted with the pack.
pub fn cve_schema_ddl(embedding_dim: usize) -> Vec<String> {
    vec![
        format!(
            "CREATE NODE TABLE Article(\
                title STRING, \
                category STRING, \
                description STRING, \
                word_count INT64, \
                published_year INT64, \
                embedding DOUBLE[{embedding_dim}], \
                PRIMARY KEY(title))"
        ),
        "CREATE NODE TABLE Entity(\
            entity_id STRING, \
            name STRING, \
            type STRING, \
            description STRING, \
            PRIMARY KEY(entity_id))"
            .to_string(),
        "CREATE REL TABLE HAS_ENTITY(FROM Article TO Entity)".to_string(),
        "CREATE REL TABLE ENTITY_RELATION(FROM Entity TO Entity, relation STRING, context STRING)"
            .to_string(),
    ]
}

/// Build (or resume building) a CVE pack at `pack_dir` from `corpus`.
///
/// See the [module docs](crate::cve_build) for the checkpoint/resume and
/// pipeline semantics. `embedder.dim()` must equal `options.embedding_dim`.
pub fn build_cve_pack<C, E>(
    pack_dir: impl AsRef<Path>,
    manifest: &PackManifest,
    params: &BuildParams,
    corpus: &C,
    embedder: &E,
    options: &PipelineOptions,
) -> Result<CveBuildReport>
where
    C: CorpusSource + ?Sized,
    E: EmbeddingModel + Sync + ?Sized,
{
    let pack_dir = pack_dir.as_ref();

    // Fail fast on invalid inputs, before any filesystem side effect.
    validate_manifest(&manifest.to_value())?;
    if params.batch == 0 {
        return Err(PacksError::PackInstall("batch size must be >= 1".into()));
    }
    if embedder.dim() != options.embedding_dim {
        return Err(PacksError::PackInstall(format!(
            "embedder dim {} does not match embedding_dim {}",
            embedder.dim(),
            options.embedding_dim
        )));
    }

    let graph_path = pack_dir.join(GRAPH_STORE_FILENAME);
    let cp_path = checkpoint_path_for(&graph_path);
    let params_hash = params.params_hash();

    let total = match params.limit {
        Some(limit) => corpus.len().min(limit),
        None => corpus.len(),
    };

    // ── Decide fresh vs. resume ──────────────────────────────────────────────
    let existing = BuildCheckpoint::load_if_present(&cp_path)?;
    let graph_exists = graph_path.exists();
    // A written manifest is the authoritative marker of a *complete* pack (the
    // checkpoint is cleared on clean finish, but if a crash lands between the
    // manifest write and that clear, a complete pack can still carry a sidecar).
    let finished = graph_exists && manifest_path_in(pack_dir).exists();

    let resume_from = if let Some(cp) = &existing {
        if options.resume && cp.params_hash == params_hash && graph_exists {
            Some(cp.clone())
        } else if finished {
            // Never clobber a completed pack, even if a stale checkpoint survived
            // its clean finish. The caller must remove it to rebuild.
            return Err(PacksError::PackInstall(format!(
                "pack already built at {} (remove it to rebuild)",
                pack_dir.display()
            )));
        } else {
            // Partial build with a stale/mismatched checkpoint: clean restart.
            // Remove the store *before* clearing the sidecar so a failed wipe
            // cannot leave an orphaned store with no checkpoint.
            remove_store(&graph_path)?;
            BuildCheckpoint::clear(&cp_path)?;
            None
        }
    } else {
        if graph_exists {
            return Err(PacksError::PackInstall(format!(
                "graph store already exists at {} (no checkpoint to resume; remove it to rebuild)",
                graph_path.display()
            )));
        }
        None
    };

    // ── Open the store and establish starting state ──────────────────────────
    let mut db;
    let mut loaded: HashSet<String>;
    let mut entities_seen: HashSet<String>;
    let mut counts: BuildCounts;
    let mut offset: usize;
    let mut committed_batches: usize;
    let resumed;
    let resumed_from_batch;

    match resume_from {
        Some(cp) => {
            db = Database::open_with_options(&graph_path, bulk_rw_options())?;
            loaded = existing_ids(&db, "MATCH (a:Article) RETURN a.title AS id")?;
            entities_seen = existing_ids(&db, "MATCH (e:Entity) RETURN e.entity_id AS id")?;
            // Trust the store, not the sidecar, for counts: if the process died
            // after a batch committed but before its checkpoint was written, the
            // store is ahead of `cp.counts`. Reading live counts keeps the
            // resumed run's progress accurate.
            counts = live_counts(&db)?;
            offset = cp.source_offset;
            committed_batches = cp.last_committed_batch;
            resumed = true;
            resumed_from_batch = cp.last_committed_batch;
        }
        None => {
            std::fs::create_dir_all(pack_dir)?;
            db = Database::open_with_options(&graph_path, bulk_rw_options())?;
            // On any failure while initializing a fresh store, remove the partial
            // store so it is never left as an un-resumable orphan (a graph with
            // no checkpoint).
            if let Err(err) = init_fresh_store(&db, options.embedding_dim, &cp_path, &params_hash) {
                db.close();
                let _ = remove_store(&graph_path);
                let _ = BuildCheckpoint::clear(&cp_path);
                return Err(err);
            }
            loaded = HashSet::new();
            entities_seen = HashSet::new();
            counts = BuildCounts::default();
            offset = 0;
            committed_batches = 0;
            resumed = false;
            resumed_from_batch = 0;
        }
    }

    // ── Pipeline: embed (worker thread) || load (this thread) ────────────────
    let start_offset = offset;
    let start_batch = committed_batches;
    let batch_size = params.batch;
    let with_relations = params.with_entity_relations;
    let year_filter = params.year;
    let (tx, rx) = sync_channel::<EmbeddedBatch>(options.queue_capacity);

    let interrupted = std::thread::scope(|scope| -> Result<bool> {
        // Producer: read + embed batches, hand them to the loader.
        let producer = scope.spawn(move || {
            let mut o = start_offset;
            let mut batch_index = start_batch;
            while o < total {
                let end = (o + batch_size).min(total);
                let mut items = Vec::with_capacity(end - o);
                for i in o..end {
                    if let Some(record) = corpus.record(i) {
                        // Publication-year filter: skip records that don't match
                        // (the raw offset still advances, so resume stays exact).
                        if let Some(year) = year_filter {
                            if record.published_year != Some(year) {
                                continue;
                            }
                        }
                        let vector = embedder.embed(&record.description);
                        items.push((record, vector));
                    }
                }
                let batch = EmbeddedBatch {
                    batch_index,
                    end_offset: end,
                    items,
                };
                if tx.send(batch).is_err() {
                    // Loader stopped early (interrupt or error): stop producing.
                    break;
                }
                o = end;
                batch_index += 1;
            }
            // `tx` drops here, closing the channel so the loader's loop ends.
        });

        // Consumer/loader: serial writer on this thread.
        let mut interrupted = false;
        let mut load_err: Option<PacksError> = None;
        {
            let conn = db.connect()?;
            for batch in rx {
                // Interrupt BEFORE loading this batch, so `Some(0)` commits
                // nothing and `Some(n)` commits exactly n batches (the received
                // batch is left for a later resume).
                if let Some(stop_after) = options.interrupt_after_batches {
                    if committed_batches >= stop_after {
                        interrupted = true;
                        break;
                    }
                }
                if let Err(err) = load_batch(
                    &conn,
                    &batch,
                    with_relations,
                    &mut loaded,
                    &mut entities_seen,
                    &mut counts,
                ) {
                    load_err = Some(err);
                    break;
                }
                offset = batch.end_offset;
                committed_batches = batch.batch_index + 1;

                // Checkpoint AFTER the batch's writes are committed, so a resume
                // never repeats a committed batch (and dedup covers the rest).
                BuildCheckpoint {
                    params_hash: params_hash.clone(),
                    last_committed_batch: committed_batches,
                    source_offset: offset,
                    counts,
                }
                .save(&cp_path)?;
            }
        }
        // `rx` is dropped with the block above, unblocking the producer if it is
        // parked on a full channel; join to surface any producer panic.
        producer
            .join()
            .map_err(|_| PacksError::PackInstall("embedding worker panicked".into()))?;

        match load_err {
            Some(err) => Err(err),
            None => Ok(interrupted),
        }
    })?;

    if interrupted {
        // Durably checkpoint the store; keep the sidecar and skip the manifest so
        // a later resume continues from here.
        db.close();
        return Ok(CveBuildReport {
            path: pack_dir.to_path_buf(),
            params_hash,
            counts,
            batches_committed: committed_batches,
            resumed,
            resumed_from_batch,
            interrupted: true,
            manifest: None,
        });
    }

    // ── Clean finish: authoritative stats from the store, manifest, cleanup ──
    let live = live_counts(&db)?;
    let mut written = manifest.clone();
    written.graph_stats = Some(counts_to_stats(&live));
    let written = validate_manifest(&written.to_value())?;
    // On manifest-write failure, leave the store + checkpoint intact so the build
    // can be resumed/retried rather than losing the loaded batches.
    save_manifest(manifest_path_in(pack_dir), &written)?;
    db.close();
    BuildCheckpoint::clear(&cp_path)?;

    Ok(CveBuildReport {
        path: pack_dir.to_path_buf(),
        params_hash,
        counts: live,
        batches_committed: committed_batches,
        resumed,
        resumed_from_batch,
        interrupted: false,
        manifest: Some(written),
    })
}

/// One batch of records with their (already computed) embeddings.
struct EmbeddedBatch {
    batch_index: usize,
    end_offset: usize,
    items: Vec<(CveRecord, Vec<f32>)>,
}

/// Bulk-load knobs: WAL-only appends during the load, one checkpoint at close,
/// so the finished pack is self-contained. Committed batches are recovered from
/// the WAL if a run is interrupted before close.
fn bulk_rw_options() -> DatabaseOptions {
    DatabaseOptions {
        auto_checkpoint: Some(false),
        ..DatabaseOptions::default()
    }
}

/// Apply the CVE schema to a fresh store and write the up-front checkpoint.
///
/// The checkpoint is written *after* the schema so a resume can safely skip
/// schema creation, and *before* the first batch so that even a crash right
/// after the first batch commits leaves a resumable sidecar rather than an
/// orphaned, un-resumable graph.
fn init_fresh_store(
    db: &Database,
    embedding_dim: usize,
    cp_path: &Path,
    params_hash: &str,
) -> Result<()> {
    {
        let conn = db.connect()?;
        for ddl in cve_schema_ddl(embedding_dim) {
            conn.run(&ddl)?;
        }
    }
    BuildCheckpoint {
        params_hash: params_hash.to_string(),
        last_committed_batch: 0,
        source_offset: 0,
        counts: BuildCounts::default(),
    }
    .save(cp_path)
}

/// Load every not-yet-loaded record of `batch` into the store as a single
/// transaction, updating the dedup sets and running counts.
///
/// Wrapping the whole batch in one transaction makes it **atomic**: a crash
/// before `COMMIT` rolls the batch back entirely, so an `Article` exists in the
/// store iff its record was fully loaded. That is what lets a resumed run treat
/// the store as authoritative — it only ever redoes an uncommitted (rolled-back)
/// batch, never a half-written record. Records already present by CVE id are
/// skipped, so a redone batch is idempotent even against duplicate corpus ids.
fn load_batch(
    conn: &Connection<'_>,
    batch: &EmbeddedBatch,
    with_relations: bool,
    loaded: &mut HashSet<String>,
    entities_seen: &mut HashSet<String>,
    counts: &mut BuildCounts,
) -> Result<()> {
    conn.run("BEGIN TRANSACTION")?;
    for (record, vector) in &batch.items {
        if loaded.contains(&record.id) {
            continue;
        }
        if let Err(err) = load_record(conn, record, vector, with_relations, entities_seen, counts) {
            // Leave the connection clean for a later retry/resume.
            let _ = conn.run("ROLLBACK");
            return Err(err);
        }
        loaded.insert(record.id.clone());
    }
    conn.run("COMMIT")?;
    Ok(())
}

/// Insert one CVE record: its `Article` node (with embedding), its entities and
/// `HAS_ENTITY` edges, and — when enabled — its `ENTITY_RELATION` edges.
fn load_record(
    conn: &Connection<'_>,
    record: &CveRecord,
    vector: &[f32],
    with_relations: bool,
    entities_seen: &mut HashSet<String>,
    counts: &mut BuildCounts,
) -> Result<()> {
    let word_count = record.description.split_whitespace().count() as i64;
    conn.run_params(
        "CREATE (:Article {\
            title: $title, category: $category, description: $description, \
            word_count: $word_count, published_year: $published_year, embedding: $embedding})",
        vec![
            ("title", Value::String(record.id.clone())),
            ("category", Value::String(CVE_CATEGORY.to_string())),
            ("description", Value::String(record.description.clone())),
            ("word_count", Value::Int64(word_count)),
            ("published_year", year_value(record.published_year)),
            ("embedding", embedding_value(vector)),
        ],
    )?;
    counts.articles += 1;

    let mut edged: HashSet<&str> = HashSet::new();
    for entity in &record.entities {
        if entities_seen.insert(entity.entity_id.clone()) {
            counts.entities += 1;
        }
        conn.run_params(
            "MERGE (e:Entity {entity_id: $entity_id}) \
             ON CREATE SET e.name = $name, e.type = $type, e.description = $description",
            vec![
                ("entity_id", Value::String(entity.entity_id.clone())),
                ("name", Value::String(entity.name.clone())),
                ("type", Value::String(entity.type_.clone())),
                ("description", Value::String(entity.description.clone())),
            ],
        )?;
        // De-duplicate HAS_ENTITY edges within a record.
        if edged.insert(entity.entity_id.as_str()) {
            conn.run_params(
                "MATCH (a:Article {title: $title}), (e:Entity {entity_id: $entity_id}) \
                 CREATE (a)-[:HAS_ENTITY]->(e)",
                vec![
                    ("title", Value::String(record.id.clone())),
                    ("entity_id", Value::String(entity.entity_id.clone())),
                ],
            )?;
            counts.relationships += 1;
        }
    }

    if with_relations {
        for rel in &record.relations {
            conn.run_params(
                "MATCH (s:Entity {entity_id: $source_id}), (t:Entity {entity_id: $target_id}) \
                 CREATE (s)-[:ENTITY_RELATION {relation: $relation, context: $context}]->(t)",
                vec![
                    ("source_id", Value::String(rel.source_id.clone())),
                    ("target_id", Value::String(rel.target_id.clone())),
                    ("relation", Value::String(rel.relation.clone())),
                    ("context", Value::String(rel.context.clone())),
                ],
            )?;
        }
    }
    Ok(())
}

fn year_value(year: Option<i64>) -> Value {
    match year {
        Some(y) => Value::Int64(y),
        None => Value::Null(LogicalType::Int64),
    }
}

fn embedding_value(embedding: &[f32]) -> Value {
    Value::Array(
        LogicalType::Double,
        embedding
            .iter()
            .map(|x| Value::Double(f64::from(*x)))
            .collect(),
    )
}

/// Collect a set of string ids from a single-column (`id`) query.
fn existing_ids(db: &Database, cypher: &str) -> Result<HashSet<String>> {
    let conn = db.connect()?;
    let rows = conn.run(cypher)?;
    let mut out = HashSet::with_capacity(rows.len());
    for row in rows {
        if let Some(Value::String(id)) = row.get("id") {
            out.insert(id.clone());
        }
    }
    Ok(out)
}

/// Authoritative node/relationship counts read back from the store.
fn live_counts(db: &Database) -> Result<BuildCounts> {
    let conn = db.connect()?;
    Ok(BuildCounts {
        articles: count(&conn, "MATCH (a:Article) RETURN count(a) AS n")?,
        entities: count(&conn, "MATCH (e:Entity) RETURN count(e) AS n")?,
        relationships: count(
            &conn,
            "MATCH (:Article)-[r:HAS_ENTITY]->(:Entity) RETURN count(r) AS n",
        )?,
    })
}

fn count(conn: &Connection<'_>, cypher: &str) -> Result<usize> {
    let rows = conn.run(cypher)?;
    let value = rows
        .first()
        .and_then(|row| row.get("n"))
        .ok_or_else(|| PacksError::PackInstall("count query returned no rows".into()))?;
    let n = match value {
        Value::Int64(n) => *n,
        Value::Int32(n) => i64::from(*n),
        other => {
            return Err(PacksError::PackInstall(format!(
                "count query returned a non-integer value: {other:?}"
            )))
        }
    };
    usize::try_from(n)
        .map_err(|_| PacksError::PackInstall(format!("negative count from store: {n}")))
}

fn counts_to_stats(counts: &BuildCounts) -> BTreeMap<String, f64> {
    let mut stats = BTreeMap::new();
    stats.insert("articles".into(), counts.articles as f64);
    stats.insert("entities".into(), counts.entities as f64);
    stats.insert("relationships".into(), counts.relationships as f64);
    stats
}

/// Remove a graph store and its WAL sidecar. A missing file is fine (nothing to
/// remove); any other I/O error is surfaced so a clean restart never silently
/// proceeds on top of a store it failed to delete.
fn remove_store(graph_path: &Path) -> Result<()> {
    remove_file_if_present(graph_path)?;
    let wal = graph_path.with_file_name(format!("{GRAPH_STORE_FILENAME}.wal"));
    remove_file_if_present(&wal)?;
    Ok(())
}

fn remove_file_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(PacksError::Io(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_hash_is_key_order_independent_and_stable() {
        let a = BuildParams {
            src: "cvelistV5".into(),
            year: Some(2025),
            limit: Some(100),
            batch: 32,
            model: "deterministic-hash-v1".into(),
            with_entity_relations: true,
        };
        let b = a.clone();
        assert_eq!(a.params_hash(), b.params_hash());
        // 64 hex chars (SHA-256).
        assert_eq!(a.params_hash().len(), 64);
    }

    #[test]
    fn params_hash_changes_with_each_output_affecting_field() {
        let base = BuildParams {
            src: "s".into(),
            year: Some(2025),
            limit: Some(10),
            batch: 8,
            model: "m".into(),
            with_entity_relations: false,
        };
        let h = base.params_hash();
        assert_ne!(
            h,
            BuildParams {
                src: "s2".into(),
                ..base.clone()
            }
            .params_hash()
        );
        assert_ne!(
            h,
            BuildParams {
                year: Some(2024),
                ..base.clone()
            }
            .params_hash()
        );
        assert_ne!(
            h,
            BuildParams {
                year: None,
                ..base.clone()
            }
            .params_hash()
        );
        assert_ne!(
            h,
            BuildParams {
                limit: Some(11),
                ..base.clone()
            }
            .params_hash()
        );
        assert_ne!(
            h,
            BuildParams {
                limit: None,
                ..base.clone()
            }
            .params_hash()
        );
        assert_ne!(
            h,
            BuildParams {
                batch: 9,
                ..base.clone()
            }
            .params_hash()
        );
        assert_ne!(
            h,
            BuildParams {
                model: "m2".into(),
                ..base.clone()
            }
            .params_hash()
        );
        assert_ne!(
            h,
            BuildParams {
                with_entity_relations: true,
                ..base.clone()
            }
            .params_hash()
        );
    }

    #[test]
    fn cve_schema_has_four_tables_with_the_embedding_width() {
        let ddl = cve_schema_ddl(768);
        assert_eq!(ddl.len(), 4);
        assert!(ddl[0].contains("embedding DOUBLE[768]"));
        assert!(ddl[2].contains("HAS_ENTITY"));
    }

    #[test]
    fn content_fingerprint_binds_corpus_content() {
        // Same bytes -> same fingerprint; any change -> different fingerprint,
        // so a src that embeds it makes a changed corpus a params-hash mismatch.
        let a = crate::content_fingerprint(b"[{\"id\":\"CVE-2025-0001\"}]");
        let b = crate::content_fingerprint(b"[{\"id\":\"CVE-2025-0001\"}]");
        let c = crate::content_fingerprint(b"[{\"id\":\"CVE-2025-0002\"}]");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64);

        let with = |src: &str| BuildParams {
            src: src.into(),
            ..BuildParams::default()
        };
        assert_ne!(
            with(&format!("corpus.json#sha256:{a}")).params_hash(),
            with(&format!("corpus.json#sha256:{c}")).params_hash(),
        );
    }
}
