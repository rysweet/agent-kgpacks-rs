//! Knowledge-pack build/load over the LadybugDB graph store.
//!
//! A pack is a directory containing a validated [`manifest.json`](crate::MANIFEST_FILENAME)
//! and a single self-contained LadybugDB graph store (`graph.lbug`). This module
//! ports the **schema + build/load** half of `packages/packs`:
//!
//! * [`SCHEMA`] is the M2 graph schema (node + relationship tables), mirroring
//!   the structural tables of `packages/ingestion/src/schema.ts`. The
//!   `embedding FLOAT[768]` columns and `CREATE_VECTOR_INDEX` DDL are
//!   intentionally deferred to M3 (ingestion) / M4 (retrieval).
//! * [`build_pack`] materializes pack content into the graph store and writes the
//!   manifest; [`load_pack`] opens the store and the manifest back.
//!
//! The build → load round-trip over the graph store is the M2 acceptance gate.
//! Tar streaming / registry install is deferred to a later milestone.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use kgpacks_db::{Database, DatabaseOptions, Value};

use crate::errors::{PacksError, Result};
use crate::manifest::{
    load_manifest_from_dir, manifest_path_in, save_manifest, validate_manifest, PackManifest,
};

/// Filename of the LadybugDB graph store inside a pack directory.
pub const GRAPH_STORE_FILENAME: &str = "graph.lbug";

/// Node-table DDL, in dependency order (nodes before any relationship).
///
/// Mirrors the structural node tables of the reference schema. Embedding columns
/// and vector indexes are deferred to M3/M4.
pub const NODE_TABLE_DDL: [&str; 3] = [
    "CREATE NODE TABLE Article(\
        title STRING, \
        category STRING, \
        word_count INT64, \
        expansion_depth INT64, \
        PRIMARY KEY(title))",
    "CREATE NODE TABLE Section(\
        id STRING, \
        title STRING, \
        content STRING, \
        level INT64, \
        word_count INT64, \
        PRIMARY KEY(id))",
    "CREATE NODE TABLE Entity(\
        entity_id STRING, \
        name STRING, \
        type STRING, \
        description STRING, \
        PRIMARY KEY(entity_id))",
];

/// Relationship-table DDL (created after all node tables exist).
pub const REL_TABLE_DDL: [&str; 4] = [
    "CREATE REL TABLE HAS_SECTION(FROM Article TO Section, section_index INT64)",
    "CREATE REL TABLE LINKS_TO(FROM Section TO Section, link_type STRING)",
    "CREATE REL TABLE HAS_ENTITY(FROM Article TO Entity)",
    "CREATE REL TABLE ENTITY_RELATION(FROM Entity TO Entity, relation STRING, context STRING)",
];

/// The full pack graph schema (node tables then relationship tables).
pub const SCHEMA: [&str; 7] = [
    NODE_TABLE_DDL[0],
    NODE_TABLE_DDL[1],
    NODE_TABLE_DDL[2],
    REL_TABLE_DDL[0],
    REL_TABLE_DDL[1],
    REL_TABLE_DDL[2],
    REL_TABLE_DDL[3],
];

/// An `Article` node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Article {
    /// Article title (primary key).
    pub title: String,
    /// Category label.
    pub category: String,
    /// Word count.
    pub word_count: i64,
    /// Link-expansion depth used when the article was ingested.
    pub expansion_depth: i64,
}

/// An `Entity` node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entity {
    /// Stable entity identifier (primary key).
    pub entity_id: String,
    /// Display name.
    pub name: String,
    /// Entity type label.
    pub type_: String,
    /// Free-text description.
    pub description: String,
}

/// The graph content materialized into a pack's store at build time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackContent {
    /// Article nodes.
    pub articles: Vec<Article>,
    /// Entity nodes.
    pub entities: Vec<Entity>,
    /// `HAS_ENTITY` edges as `(article_title, entity_id)` pairs.
    pub article_entities: Vec<(String, String)>,
}

impl PackContent {
    /// Counts mirroring the manifest `graph_stats` shape.
    fn graph_stats(&self) -> BTreeMap<String, f64> {
        let mut stats = BTreeMap::new();
        stats.insert("articles".into(), self.articles.len() as f64);
        stats.insert("entities".into(), self.entities.len() as f64);
        stats.insert("relationships".into(), self.article_entities.len() as f64);
        stats
    }
}

/// A pack written to disk by [`build_pack`].
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltPack {
    /// Pack name.
    pub name: String,
    /// Pack version.
    pub version: String,
    /// Pack directory.
    pub path: PathBuf,
    /// The validated manifest that was written (with `graph_stats` populated).
    pub manifest: PackManifest,
}

/// A single planned load statement: Cypher text plus its bound parameters.
///
/// Separating the *plan* (what statements a load issues) from *execution*
/// (running them against a store) keeps the loader's DB-statement shape and
/// count observable without a live LadybugDB — that is what the WS5
/// linear-scaling guard inspects. The [`cypher`](Self::cypher) is public so a
/// test can assert no edge-creation statement uses the O(N²) comma two-pattern
/// `MATCH (a {..}), (b {..})`; parameters stay internal.
#[derive(Debug, Clone)]
pub struct PlannedStatement {
    /// The Cypher statement text (parameters are bound separately).
    pub cypher: String,
    params: Vec<(&'static str, Value)>,
}

impl PlannedStatement {
    fn ddl(cypher: &str) -> Self {
        Self {
            cypher: cypher.to_string(),
            params: Vec::new(),
        }
    }

    fn with_params(cypher: &str, params: Vec<(&'static str, Value)>) -> Self {
        Self {
            cypher: cypher.to_string(),
            params,
        }
    }

    /// Run this statement on `conn` (no-params branch when there are none, so
    /// schema DDL still goes through the plain [`run`](kgpacks_db::Connection::run)
    /// path exactly as before).
    fn run(self, conn: &kgpacks_db::Connection<'_>) -> Result<()> {
        if self.params.is_empty() {
            conn.run(&self.cypher)?;
        } else {
            conn.run_params(&self.cypher, self.params)?;
        }
        Ok(())
    }
}

/// Cypher that creates one `HAS_ENTITY` edge between an existing `Article` and
/// `Entity`, each located by its **primary key** in its own single `MATCH`.
///
/// Two PK-indexed point lookups (each returns exactly one node) replace the
/// O(N²) comma two-pattern `MATCH (a {..}), (e {..})`, whose cartesian shape
/// forces the streaming loader toward quadratic work as record counts grow.
pub const CREATE_HAS_ENTITY_CYPHER: &str = "MATCH (a:Article {title: $title}) \
     MATCH (e:Entity {entity_id: $entity_id}) \
     CREATE (a)-[:HAS_ENTITY]->(e)";

/// The full, ordered list of statements a [`build_pack`] load issues for
/// `content`: schema DDL, then one `CREATE` per article, per entity, and per
/// `HAS_ENTITY` edge.
///
/// The count is linear in the number of records (a fixed schema prefix plus one
/// statement per record), and every edge statement uses [`CREATE_HAS_ENTITY_CYPHER`]
/// (PK-indexed single-`MATCH`), never the comma two-pattern — the two properties
/// the WS5 guard asserts.
pub fn plan_load_statements(content: &PackContent) -> Vec<PlannedStatement> {
    let mut statements = Vec::with_capacity(
        SCHEMA.len()
            + content.articles.len()
            + content.entities.len()
            + content.article_entities.len(),
    );

    for ddl in SCHEMA {
        statements.push(PlannedStatement::ddl(ddl));
    }

    for article in &content.articles {
        statements.push(PlannedStatement::with_params(
            "CREATE (:Article {title: $title, category: $category, \
             word_count: $word_count, expansion_depth: $expansion_depth})",
            vec![
                ("title", Value::String(article.title.clone())),
                ("category", Value::String(article.category.clone())),
                ("word_count", Value::Int64(article.word_count)),
                ("expansion_depth", Value::Int64(article.expansion_depth)),
            ],
        ));
    }

    for entity in &content.entities {
        statements.push(PlannedStatement::with_params(
            "CREATE (:Entity {entity_id: $entity_id, name: $name, \
             type: $type, description: $description})",
            vec![
                ("entity_id", Value::String(entity.entity_id.clone())),
                ("name", Value::String(entity.name.clone())),
                ("type", Value::String(entity.type_.clone())),
                ("description", Value::String(entity.description.clone())),
            ],
        ));
    }

    for (article_title, entity_id) in &content.article_entities {
        statements.push(PlannedStatement::with_params(
            CREATE_HAS_ENTITY_CYPHER,
            vec![
                ("title", Value::String(article_title.clone())),
                ("entity_id", Value::String(entity_id.clone())),
            ],
        ));
    }

    statements
}

/// Build a pack at `pack_dir`: materialize `content` into a LadybugDB graph store
/// and write the validated manifest.
///
/// `graph_stats` on the manifest is (re)populated from the materialized content
/// so the manifest and the store agree. The (populated) manifest is validated
/// **before** any filesystem side effect, so invalid input fails cleanly; if a
/// later step fails, the partial graph store is removed so the directory can be
/// rebuilt (never leaves partial state). Fails if `pack_dir` already contains a
/// graph store.
pub fn build_pack(
    pack_dir: impl AsRef<Path>,
    manifest: &PackManifest,
    content: &PackContent,
) -> Result<BuiltPack> {
    let pack_dir = pack_dir.as_ref();
    let graph_path = pack_dir.join(GRAPH_STORE_FILENAME);

    // Build the manifest we intend to write (graph_stats from the materialized
    // content) and validate it up front — before touching the filesystem.
    let mut written = manifest.clone();
    written.graph_stats = Some(content.graph_stats());
    let written = validate_manifest(&written.to_value())?;

    if graph_path.exists() {
        return Err(PacksError::PackInstall(format!(
            "graph store already exists at {}",
            graph_path.display()
        )));
    }
    std::fs::create_dir_all(pack_dir)?;

    // Materialize the store; on any failure remove the partial store + WAL so the
    // directory is not wedged for a later retry.
    if let Err(err) = materialize_store(&graph_path, content) {
        remove_partial_store(&graph_path);
        return Err(err);
    }
    if let Err(err) = save_manifest(manifest_path_in(pack_dir), &written) {
        remove_partial_store(&graph_path);
        return Err(err);
    }

    Ok(BuiltPack {
        name: written.name.clone(),
        version: written.version.clone(),
        path: pack_dir.to_path_buf(),
        manifest: written,
    })
}

/// Materialize `content` into a fresh LadybugDB store at `graph_path`.
fn materialize_store(graph_path: &Path, content: &PackContent) -> Result<()> {
    // Bulk-load knob: WAL-only appends during the load, one checkpoint at close,
    // so the resulting pack file is self-contained (no .wal sidecar).
    let options = DatabaseOptions {
        auto_checkpoint: Some(false),
        ..DatabaseOptions::default()
    };
    let mut db = Database::open_with_options(graph_path, options)?;
    {
        let conn = db.connect()?;
        for statement in plan_load_statements(content) {
            statement.run(&conn)?;
        }
    }
    db.close();
    Ok(())
}

/// Best-effort removal of a partially-written graph store and its WAL sidecar.
fn remove_partial_store(graph_path: &Path) {
    let _ = std::fs::remove_file(graph_path);
    let wal = graph_path.with_file_name(format!("{GRAPH_STORE_FILENAME}.wal"));
    let _ = std::fs::remove_file(wal);
}

/// A pack opened from disk by [`load_pack`]: its validated manifest plus the
/// (read-only) LadybugDB graph store.
#[derive(Debug)]
pub struct LoadedPack {
    manifest: PackManifest,
    database: Database,
}

impl LoadedPack {
    /// The pack's validated manifest.
    pub fn manifest(&self) -> &PackManifest {
        &self.manifest
    }

    /// A fresh read connection to the pack's graph store.
    pub fn connect(&self) -> Result<kgpacks_db::Connection<'_>> {
        Ok(self.database.connect()?)
    }

    /// Live counts read back from the graph store, in the manifest
    /// `graph_stats` shape (`articles`, `entities`, `relationships`).
    pub fn graph_stats(&self) -> Result<BTreeMap<String, f64>> {
        let conn = self.connect()?;
        let mut stats = BTreeMap::new();
        stats.insert(
            "articles".into(),
            count(&conn, "MATCH (a:Article) RETURN count(a) AS n")?,
        );
        stats.insert(
            "entities".into(),
            count(&conn, "MATCH (e:Entity) RETURN count(e) AS n")?,
        );
        stats.insert(
            "relationships".into(),
            count(
                &conn,
                "MATCH (:Article)-[r:HAS_ENTITY]->(:Entity) RETURN count(r) AS n",
            )?,
        );
        Ok(stats)
    }
}

fn count(conn: &kgpacks_db::Connection<'_>, cypher: &str) -> Result<f64> {
    let rows = conn.run(cypher)?;
    let value = rows
        .first()
        .and_then(|row| row.get("n"))
        .ok_or_else(|| PacksError::PackInstall("count query returned no rows".into()))?;
    Ok(match value {
        Value::Int64(n) => *n as f64,
        Value::Int32(n) => f64::from(*n),
        other => {
            return Err(PacksError::PackInstall(format!(
                "count query returned a non-integer value: {other:?}"
            )))
        }
    })
}

/// Open a pack from `pack_dir`: validate its manifest and open the graph store
/// read-only.
///
/// Errors with [`PacksError::PackNotFound`] if the directory has no graph store.
pub fn load_pack(pack_dir: impl AsRef<Path>) -> Result<LoadedPack> {
    let pack_dir = pack_dir.as_ref();
    let manifest = load_manifest_from_dir(pack_dir)?;

    let graph_path = pack_dir.join(GRAPH_STORE_FILENAME);
    if !graph_path.exists() {
        return Err(PacksError::PackNotFound(format!(
            "no graph store at {}",
            graph_path.display()
        )));
    }
    let options = DatabaseOptions {
        read_only: Some(true),
        ..DatabaseOptions::default()
    };
    let database = Database::open_with_options(&graph_path, options)?;
    Ok(LoadedPack { manifest, database })
}
