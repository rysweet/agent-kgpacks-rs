//! WS6 acceptance gate: **resumable + pipelined CVE pack build**.
//!
//! Exercises the real LadybugDB-backed build path end to end:
//! * a full build materializes the CVE graph and clears its checkpoint;
//! * an interrupted build leaves a resumable checkpoint whose `params_hash`
//!   matches the run, and resuming completes it without duplicating records;
//! * a resume against changed parameters cleanly restarts;
//! * the pipeline produces identical output across queue capacities; and
//! * embedding runs off the load thread (proving `embed || load` overlap).

use std::collections::HashSet;
use std::sync::Mutex;
use std::thread::ThreadId;

use kgpacks_db::{Database, DatabaseOptions, Value};
use kgpacks_embeddings::{Embedder, EmbeddingModel, DEFAULT_DIM};
use kgpacks_packs::{
    build_cve_pack, checkpoint_path_for, BuildCheckpoint, BuildCounts, BuildParams, CveEntity,
    CveRecord, CveRelation, FixtureCorpus, PackManifest, PipelineOptions, GRAPH_STORE_FILENAME,
};

/// A corpus of `n` synthetic CVE records that deliberately **share** entities
/// across records (so entity de-duplication via `MERGE` is exercised) and carry
/// one intra-record relation each.
fn sample_corpus(n: usize) -> FixtureCorpus {
    let mut records = Vec::with_capacity(n);
    for i in 0..n {
        let vendor = format!("vendor:v{}", i % 3); // shared across records
        let product = format!("product:p{i}");
        records.push(CveRecord {
            id: format!("CVE-2025-{i:04}"),
            description: format!(
                "A vulnerability number {i} affecting product p{i} from a vendor."
            ),
            published_year: Some(2025),
            entities: vec![
                CveEntity {
                    entity_id: product.clone(),
                    name: format!("Product {i}"),
                    type_: "product".into(),
                    description: "an affected product".into(),
                },
                CveEntity {
                    entity_id: vendor.clone(),
                    name: format!("Vendor {}", i % 3),
                    type_: "vendor".into(),
                    description: "the vendor".into(),
                },
            ],
            relations: vec![CveRelation {
                source_id: product,
                target_id: vendor,
                relation: "made_by".into(),
                context: "vendor of the affected product".into(),
            }],
        });
    }
    FixtureCorpus::new(records)
}

fn params(src: &str, batch: usize, with_relations: bool) -> BuildParams {
    BuildParams {
        src: src.into(),
        year: Some(2025),
        limit: None,
        batch,
        model: kgpacks_embeddings::DETERMINISTIC_MODEL.to_string(),
        with_entity_relations: with_relations,
    }
}

fn opts(resume: bool) -> PipelineOptions {
    PipelineOptions {
        resume,
        embedding_dim: DEFAULT_DIM,
        ..PipelineOptions::default()
    }
}

/// Open the built store read-only and summarize what it contains.
struct StoreSummary {
    articles: Vec<String>,
    entities: Vec<String>,
    has_entity: i64,
    entity_relation: i64,
    all_have_embeddings: bool,
}

fn summarize(pack_dir: &std::path::Path) -> StoreSummary {
    let graph = pack_dir.join(GRAPH_STORE_FILENAME);
    let db = Database::open_with_options(
        &graph,
        DatabaseOptions {
            read_only: Some(true),
            ..DatabaseOptions::default()
        },
    )
    .expect("open store");
    let conn = db.connect().expect("connect");

    let articles = string_col(
        &conn,
        "MATCH (a:Article) RETURN a.title AS v ORDER BY a.title",
    );
    let entities = string_col(
        &conn,
        "MATCH (e:Entity) RETURN e.entity_id AS v ORDER BY e.entity_id",
    );
    let has_entity = scalar(
        &conn,
        "MATCH (:Article)-[r:HAS_ENTITY]->(:Entity) RETURN count(r) AS n",
    );
    let entity_relation = scalar(
        &conn,
        "MATCH (:Entity)-[r:ENTITY_RELATION]->(:Entity) RETURN count(r) AS n",
    );
    // Every article must carry a full-width embedding vector.
    let embed_rows = conn
        .run("MATCH (a:Article) RETURN a.embedding AS v")
        .expect("query embeddings");
    let all_have_embeddings = !embed_rows.is_empty()
        && embed_rows.iter().all(|row| match &row["v"] {
            Value::Array(_, items) | Value::List(_, items) => items.len() == DEFAULT_DIM,
            _ => false,
        });
    StoreSummary {
        articles,
        entities,
        has_entity,
        entity_relation,
        all_have_embeddings,
    }
}

fn string_col(conn: &kgpacks_db::Connection<'_>, cypher: &str) -> Vec<String> {
    conn.run(cypher)
        .expect("query")
        .iter()
        .map(|row| match &row["v"] {
            Value::String(s) => s.clone(),
            other => panic!("expected string, got {other:?}"),
        })
        .collect()
}

fn scalar(conn: &kgpacks_db::Connection<'_>, cypher: &str) -> i64 {
    match &conn.run(cypher).expect("query")[0]["n"] {
        Value::Int64(n) => *n,
        Value::Int32(n) => i64::from(*n),
        other => panic!("expected integer, got {other:?}"),
    }
}

#[test]
fn builds_a_cve_pack_end_to_end_and_clears_the_checkpoint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pack_dir = dir.path().join("cve");
    let corpus = sample_corpus(7);

    let report = build_cve_pack(
        &pack_dir,
        &PackManifest::new("cve", "1.0.0"),
        &params("fixture", 3, true),
        &corpus,
        &Embedder::new(DEFAULT_DIM),
        &opts(false),
    )
    .expect("build");

    assert!(!report.interrupted);
    assert!(!report.resumed);
    assert_eq!(report.counts.articles, 7);
    // 3 shared vendors + 7 distinct products = 10 entities.
    assert_eq!(report.counts.entities, 10);
    assert_eq!(report.counts.relationships, 14); // 2 HAS_ENTITY per record
    assert!(report.manifest.is_some());

    // Checkpoint sidecar is gone after a clean finish; no WAL sidecar remains.
    let cp = checkpoint_path_for(pack_dir.join(GRAPH_STORE_FILENAME));
    assert!(!cp.exists(), "checkpoint must be cleared on clean finish");
    assert!(!pack_dir
        .join(format!("{GRAPH_STORE_FILENAME}.wal"))
        .exists());

    let s = summarize(&pack_dir);
    assert_eq!(s.articles.len(), 7);
    assert_eq!(s.entities.len(), 10);
    assert_eq!(s.has_entity, 14);
    assert_eq!(s.entity_relation, 7); // one relation per record
    assert!(
        s.all_have_embeddings,
        "every article must carry an embedding"
    );
}

#[test]
fn resumes_after_interruption_without_duplicating_records() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pack_dir = dir.path().join("cve");
    let corpus = sample_corpus(10);
    let p = params("fixture", 3, true);

    // Interrupt after 2 committed batches (6 of 10 records loaded).
    let interrupted = build_cve_pack(
        &pack_dir,
        &PackManifest::new("cve", "1.0.0"),
        &p,
        &corpus,
        &Embedder::new(DEFAULT_DIM),
        &PipelineOptions {
            interrupt_after_batches: Some(2),
            ..opts(false)
        },
    )
    .expect("partial build");

    assert!(interrupted.interrupted);
    assert_eq!(interrupted.batches_committed, 2);
    assert_eq!(interrupted.counts.articles, 6);

    // The checkpoint exists and records the run's params hash + progress.
    let cp_path = checkpoint_path_for(pack_dir.join(GRAPH_STORE_FILENAME));
    let cp = BuildCheckpoint::load(&cp_path).expect("checkpoint present");
    assert_eq!(cp.params_hash, p.params_hash());
    assert_eq!(cp.last_committed_batch, 2);
    assert_eq!(cp.source_offset, 6);
    assert_eq!(summarize(&pack_dir).articles.len(), 6);

    // Resume: same params, no interrupt -> completes to 10 with no duplication.
    let resumed = build_cve_pack(
        &pack_dir,
        &PackManifest::new("cve", "1.0.0"),
        &p,
        &corpus,
        &Embedder::new(DEFAULT_DIM),
        &opts(true),
    )
    .expect("resume build");

    assert!(resumed.resumed);
    assert_eq!(resumed.resumed_from_batch, 2);
    assert!(!resumed.interrupted);
    assert!(!cp_path.exists(), "checkpoint cleared on clean finish");

    let s = summarize(&pack_dir);
    assert_eq!(s.articles.len(), 10, "no duplicated or missing records");
    let unique: HashSet<&String> = s.articles.iter().collect();
    assert_eq!(unique.len(), 10, "article ids are unique after resume");
    assert_eq!(s.has_entity, 20); // 2 per record, no duplicate edges
}

#[test]
fn resume_result_matches_an_uninterrupted_build() {
    let corpus = sample_corpus(11);
    let p = params("fixture", 4, true);

    // Reference: a single clean build.
    let ref_dir = tempfile::tempdir().expect("tempdir");
    let ref_pack = ref_dir.path().join("cve");
    build_cve_pack(
        &ref_pack,
        &PackManifest::new("cve", "1.0.0"),
        &p,
        &corpus,
        &Embedder::new(DEFAULT_DIM),
        &opts(false),
    )
    .expect("reference build");
    let reference = summarize(&ref_pack);

    // Interrupted then resumed build.
    let dir = tempfile::tempdir().expect("tempdir");
    let pack_dir = dir.path().join("cve");
    build_cve_pack(
        &pack_dir,
        &PackManifest::new("cve", "1.0.0"),
        &p,
        &corpus,
        &Embedder::new(DEFAULT_DIM),
        &PipelineOptions {
            interrupt_after_batches: Some(1),
            ..opts(false)
        },
    )
    .expect("partial");
    build_cve_pack(
        &pack_dir,
        &PackManifest::new("cve", "1.0.0"),
        &p,
        &corpus,
        &Embedder::new(DEFAULT_DIM),
        &opts(true),
    )
    .expect("resume");
    let resumed = summarize(&pack_dir);

    assert_eq!(resumed.articles, reference.articles);
    assert_eq!(resumed.entities, reference.entities);
    assert_eq!(resumed.has_entity, reference.has_entity);
    assert_eq!(resumed.entity_relation, reference.entity_relation);
}

#[test]
fn resume_with_a_changed_params_hash_restarts_clean() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pack_dir = dir.path().join("cve");
    let corpus = sample_corpus(9);

    // Partial build with params A.
    build_cve_pack(
        &pack_dir,
        &PackManifest::new("cve", "1.0.0"),
        &params("srcA", 3, false),
        &corpus,
        &Embedder::new(DEFAULT_DIM),
        &PipelineOptions {
            interrupt_after_batches: Some(1),
            ..opts(false)
        },
    )
    .expect("partial A");

    // Resume with params B (different src AND relations enabled): must refuse the
    // stale checkpoint and rebuild cleanly to the full corpus under B.
    let params_b = params("srcB", 3, true);
    let report = build_cve_pack(
        &pack_dir,
        &PackManifest::new("cve", "1.0.0"),
        &params_b,
        &corpus,
        &Embedder::new(DEFAULT_DIM),
        &opts(true),
    )
    .expect("clean restart under B");

    assert!(!report.resumed, "a params mismatch must not resume");
    assert_eq!(report.params_hash, params_b.params_hash());
    let s = summarize(&pack_dir);
    assert_eq!(s.articles.len(), 9);
    assert_eq!(s.entity_relation, 9, "params B enabled entity relations");
}

#[test]
fn year_filter_loads_only_matching_records() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pack_dir = dir.path().join("cve");
    // A mixed-year corpus: 2 records in 2024, 3 in 2025, 1 with unknown year.
    let mut records = Vec::new();
    for (i, year) in [
        Some(2024),
        Some(2025),
        Some(2024),
        Some(2025),
        None,
        Some(2025),
    ]
    .into_iter()
    .enumerate()
    {
        let mut r = CveRecord::new(format!("CVE-2020-{i:04}"), format!("flaw {i}"));
        r.published_year = year;
        records.push(r);
    }
    let corpus = FixtureCorpus::new(records);

    let report = build_cve_pack(
        &pack_dir,
        &PackManifest::new("cve", "1.0.0"),
        &BuildParams {
            src: "fixture".into(),
            year: Some(2025),
            limit: None,
            batch: 2,
            model: kgpacks_embeddings::DETERMINISTIC_MODEL.to_string(),
            with_entity_relations: false,
        },
        &corpus,
        &Embedder::new(DEFAULT_DIM),
        &opts(false),
    )
    .expect("build");

    // Only the three 2025 records are loaded; 2024 and unknown-year are skipped.
    assert_eq!(report.counts.articles, 3);
    let s = summarize(&pack_dir);
    assert_eq!(
        s.articles,
        vec!["CVE-2020-0001", "CVE-2020-0003", "CVE-2020-0005"]
    );
}

#[test]
fn a_finished_pack_is_not_clobbered_even_with_a_stray_checkpoint() {
    // A crash between the manifest write and the checkpoint clear can leave a
    // complete pack that still carries a sidecar. A later non-resume (or
    // mismatched-resume) build must refuse rather than wipe the finished pack.
    let dir = tempfile::tempdir().expect("tempdir");
    let pack_dir = dir.path().join("cve");
    let corpus = sample_corpus(4);
    let p = params("fixture", 2, false);

    build_cve_pack(
        &pack_dir,
        &PackManifest::new("cve", "1.0.0"),
        &p,
        &corpus,
        &Embedder::new(DEFAULT_DIM),
        &opts(false),
    )
    .expect("clean build");

    // Simulate the surviving sidecar.
    let cp_path = checkpoint_path_for(pack_dir.join(GRAPH_STORE_FILENAME));
    BuildCheckpoint {
        params_hash: p.params_hash(),
        last_committed_batch: 2,
        source_offset: 4,
        counts: BuildCounts {
            articles: 4,
            entities: 5,
            relationships: 8,
        },
    }
    .save(&cp_path)
    .expect("write stray checkpoint");

    // A non-resume rebuild must refuse (not wipe) the finished pack.
    let err = build_cve_pack(
        &pack_dir,
        &PackManifest::new("cve", "1.0.0"),
        &p,
        &corpus,
        &Embedder::new(DEFAULT_DIM),
        &opts(false),
    );
    assert!(err.is_err(), "must refuse to clobber a finished pack");
    // The finished pack is intact.
    assert!(pack_dir.join(GRAPH_STORE_FILENAME).is_file());
    assert!(pack_dir.join("manifest.json").is_file());
    assert_eq!(summarize(&pack_dir).articles.len(), 4);
}

#[test]
fn a_fresh_build_refuses_to_clobber_a_finished_pack() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pack_dir = dir.path().join("cve");
    let corpus = sample_corpus(3);
    let p = params("fixture", 2, false);

    build_cve_pack(
        &pack_dir,
        &PackManifest::new("cve", "1.0.0"),
        &p,
        &corpus,
        &Embedder::new(DEFAULT_DIM),
        &opts(false),
    )
    .expect("first build");

    // A finished pack has no checkpoint; a non-resume rebuild must refuse.
    let err = build_cve_pack(
        &pack_dir,
        &PackManifest::new("cve", "1.0.0"),
        &p,
        &corpus,
        &Embedder::new(DEFAULT_DIM),
        &opts(false),
    );
    assert!(err.is_err(), "must not clobber an existing store");
}

#[test]
fn pipeline_output_is_identical_across_queue_capacities() {
    let corpus = sample_corpus(13);
    let p = params("fixture", 3, true);

    let mut reference: Option<(Vec<String>, Vec<String>, i64, i64)> = None;
    for capacity in [0usize, 1, 4, 32] {
        let dir = tempfile::tempdir().expect("tempdir");
        let pack_dir = dir.path().join("cve");
        build_cve_pack(
            &pack_dir,
            &PackManifest::new("cve", "1.0.0"),
            &p,
            &corpus,
            &Embedder::new(DEFAULT_DIM),
            &PipelineOptions {
                queue_capacity: capacity,
                ..opts(false)
            },
        )
        .unwrap_or_else(|e| panic!("build with capacity {capacity} failed: {e}"));
        let s = summarize(&pack_dir);
        let snapshot = (s.articles, s.entities, s.has_entity, s.entity_relation);
        match &reference {
            None => reference = Some(snapshot),
            Some(r) => assert_eq!(&snapshot, r, "capacity {capacity} diverged"),
        }
    }
}

/// An embedder that records the id of the thread each `embed` call runs on.
struct ThreadRecordingEmbedder {
    inner: Embedder,
    threads: Mutex<HashSet<ThreadId>>,
}

impl EmbeddingModel for ThreadRecordingEmbedder {
    fn dim(&self) -> usize {
        self.inner.dim()
    }
    fn embed(&self, text: &str) -> Vec<f32> {
        self.threads
            .lock()
            .expect("lock")
            .insert(std::thread::current().id());
        self.inner.embed(text)
    }
}

#[test]
fn embedding_runs_off_the_load_thread() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pack_dir = dir.path().join("cve");
    let corpus = sample_corpus(6);
    let embedder = ThreadRecordingEmbedder {
        inner: Embedder::new(DEFAULT_DIM),
        threads: Mutex::new(HashSet::new()),
    };

    build_cve_pack(
        &pack_dir,
        &PackManifest::new("cve", "1.0.0"),
        &params("fixture", 2, false),
        &corpus,
        &embedder,
        &opts(false),
    )
    .expect("build");

    let embed_threads = embedder.threads.into_inner().expect("lock");
    assert!(!embed_threads.is_empty(), "embedding must have run");
    assert!(
        !embed_threads.contains(&std::thread::current().id()),
        "embedding must run on the worker thread, not the load thread"
    );
}

#[test]
fn resume_with_no_prior_state_builds_fresh() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pack_dir = dir.path().join("cve");
    let corpus = sample_corpus(4);

    // resume=true with nothing on disk yet is a normal fresh build.
    let report = build_cve_pack(
        &pack_dir,
        &PackManifest::new("cve", "1.0.0"),
        &params("fixture", 2, false),
        &corpus,
        &Embedder::new(DEFAULT_DIM),
        &opts(true),
    )
    .expect("fresh build under resume");
    assert!(!report.resumed);
    assert_eq!(summarize(&pack_dir).articles.len(), 4);
}

#[test]
fn without_entity_relations_no_relation_edges_are_created() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pack_dir = dir.path().join("cve");
    let corpus = sample_corpus(5);

    build_cve_pack(
        &pack_dir,
        &PackManifest::new("cve", "1.0.0"),
        &params("fixture", 2, false),
        &corpus,
        &Embedder::new(DEFAULT_DIM),
        &opts(false),
    )
    .expect("build");

    let s = summarize(&pack_dir);
    assert_eq!(s.entity_relation, 0, "relations disabled -> no edges");
    assert_eq!(s.has_entity, 10, "HAS_ENTITY edges are still created");
}

#[test]
fn duplicate_corpus_ids_are_loaded_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pack_dir = dir.path().join("cve");
    // Two records share the same CVE id (e.g. a corpus with a stray duplicate),
    // one in the first batch and one in a later batch.
    let corpus = FixtureCorpus::new(vec![
        CveRecord::new("CVE-2025-0000", "first"),
        CveRecord::new("CVE-2025-0001", "second"),
        CveRecord::new("CVE-2025-0000", "duplicate of the first"),
    ]);

    let report = build_cve_pack(
        &pack_dir,
        &PackManifest::new("cve", "1.0.0"),
        &BuildParams {
            src: "fixture".into(),
            year: None,
            limit: None,
            batch: 2,
            model: kgpacks_embeddings::DETERMINISTIC_MODEL.to_string(),
            with_entity_relations: false,
        },
        &corpus,
        &Embedder::new(DEFAULT_DIM),
        &opts(false),
    )
    .expect("build must not fail on a duplicate id");

    // The duplicate is skipped: two distinct articles, not three.
    assert_eq!(report.counts.articles, 2);
    let s = summarize(&pack_dir);
    assert_eq!(s.articles, vec!["CVE-2025-0000", "CVE-2025-0001"]);
}

#[test]
fn resume_recovers_when_the_store_is_ahead_of_the_checkpoint() {
    // Simulates a crash where a batch committed to the store but the process
    // died before its checkpoint write landed: the on-disk checkpoint lags the
    // store. Resume must still finish correctly (no double-load, right counts).
    let dir = tempfile::tempdir().expect("tempdir");
    let pack_dir = dir.path().join("cve");
    let corpus = sample_corpus(10);
    let p = params("fixture", 3, true);

    build_cve_pack(
        &pack_dir,
        &PackManifest::new("cve", "1.0.0"),
        &p,
        &corpus,
        &Embedder::new(DEFAULT_DIM),
        &PipelineOptions {
            interrupt_after_batches: Some(2), // store has 6 records committed
            ..opts(false)
        },
    )
    .expect("partial build");

    // Rewind the checkpoint to lag the store by one batch (store: 6 records at
    // offset 6; checkpoint now claims only 3 at offset 3, with stale counts).
    let cp_path = checkpoint_path_for(pack_dir.join(GRAPH_STORE_FILENAME));
    let mut cp = BuildCheckpoint::load(&cp_path).expect("checkpoint");
    cp.last_committed_batch = 1;
    cp.source_offset = 3;
    cp.counts = BuildCounts {
        articles: 3,
        entities: 0,
        relationships: 0,
    };
    cp.save(&cp_path).expect("rewind checkpoint");

    // Resume: the already-committed records at offsets 3..6 are skipped via the
    // DB-rebuilt dedup set, the rest load once, and the pack completes.
    build_cve_pack(
        &pack_dir,
        &PackManifest::new("cve", "1.0.0"),
        &p,
        &corpus,
        &Embedder::new(DEFAULT_DIM),
        &opts(true),
    )
    .expect("resume");

    let s = summarize(&pack_dir);
    assert_eq!(s.articles.len(), 10);
    let unique: HashSet<&String> = s.articles.iter().collect();
    assert_eq!(
        unique.len(),
        10,
        "no duplicated records after a lagging resume"
    );
    assert_eq!(s.has_entity, 20);
}

#[test]
fn a_store_is_never_left_without_a_checkpoint_until_clean_finish() {
    // interrupt_after_batches = Some(0) commits nothing, yet the store and its
    // checkpoint both exist (the checkpoint is written up front), so the build
    // is resumable rather than an orphaned, un-resumable store.
    let dir = tempfile::tempdir().expect("tempdir");
    let pack_dir = dir.path().join("cve");
    let corpus = sample_corpus(6);
    let p = params("fixture", 2, false);

    let report = build_cve_pack(
        &pack_dir,
        &PackManifest::new("cve", "1.0.0"),
        &p,
        &corpus,
        &Embedder::new(DEFAULT_DIM),
        &PipelineOptions {
            interrupt_after_batches: Some(0),
            ..opts(false)
        },
    )
    .expect("zero-batch interrupt");

    assert!(report.interrupted);
    assert_eq!(report.batches_committed, 0);
    let graph = pack_dir.join(GRAPH_STORE_FILENAME);
    let cp_path = checkpoint_path_for(&graph);
    assert!(graph.is_file(), "store exists");
    assert!(cp_path.exists(), "a resumable checkpoint exists");
    assert_eq!(summarize(&pack_dir).articles.len(), 0);

    // And it resumes to a complete pack.
    build_cve_pack(
        &pack_dir,
        &PackManifest::new("cve", "1.0.0"),
        &p,
        &corpus,
        &Embedder::new(DEFAULT_DIM),
        &opts(true),
    )
    .expect("resume from zero");
    assert_eq!(summarize(&pack_dir).articles.len(), 6);
    assert!(!cp_path.exists(), "checkpoint cleared on clean finish");
}
