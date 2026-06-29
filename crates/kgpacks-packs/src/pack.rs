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
use crate::manifest::{load_manifest_from_dir, manifest_path_in, save_manifest, PackManifest};

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

/// Apply the pack graph [`SCHEMA`] to a connection (node tables, then rels).
fn apply_schema(conn: &kgpacks_db::Connection<'_>) -> Result<()> {
    for ddl in SCHEMA {
        conn.run(ddl)?;
    }
    Ok(())
}

/// Build a pack at `pack_dir`: materialize `content` into a LadybugDB graph store
/// and write the validated manifest.
///
/// `graph_stats` on the manifest is (re)populated from the materialized content
/// so the manifest and the store agree. Fails if `pack_dir` already contains a
/// graph store.
pub fn build_pack(
    pack_dir: impl AsRef<Path>,
    manifest: &PackManifest,
    content: &PackContent,
) -> Result<BuiltPack> {
    let pack_dir = pack_dir.as_ref();
    let graph_path = pack_dir.join(GRAPH_STORE_FILENAME);
    if graph_path.exists() {
        return Err(PacksError::PackInstall(format!(
            "graph store already exists at {}",
            graph_path.display()
        )));
    }
    std::fs::create_dir_all(pack_dir)?;

    // Bulk-load knob: WAL-only appends during the load, one checkpoint at close,
    // so the resulting pack file is self-contained (no .wal sidecar).
    let options = DatabaseOptions {
        auto_checkpoint: Some(false),
        ..DatabaseOptions::default()
    };
    let mut db = Database::open_with_options(&graph_path, options)?;
    {
        let conn = db.connect()?;
        apply_schema(&conn)?;

        for article in &content.articles {
            conn.run_params(
                "CREATE (:Article {title: $title, category: $category, \
                 word_count: $word_count, expansion_depth: $expansion_depth})",
                vec![
                    ("title", Value::String(article.title.clone())),
                    ("category", Value::String(article.category.clone())),
                    ("word_count", Value::Int64(article.word_count)),
                    ("expansion_depth", Value::Int64(article.expansion_depth)),
                ],
            )?;
        }

        for entity in &content.entities {
            conn.run_params(
                "CREATE (:Entity {entity_id: $entity_id, name: $name, \
                 type: $type, description: $description})",
                vec![
                    ("entity_id", Value::String(entity.entity_id.clone())),
                    ("name", Value::String(entity.name.clone())),
                    ("type", Value::String(entity.type_.clone())),
                    ("description", Value::String(entity.description.clone())),
                ],
            )?;
        }

        for (article_title, entity_id) in &content.article_entities {
            conn.run_params(
                "MATCH (a:Article {title: $title}), (e:Entity {entity_id: $entity_id}) \
                 CREATE (a)-[:HAS_ENTITY]->(e)",
                vec![
                    ("title", Value::String(article_title.clone())),
                    ("entity_id", Value::String(entity_id.clone())),
                ],
            )?;
        }
    }
    db.close();

    // Populate graph_stats from the materialized content, then write the manifest.
    let mut written = manifest.clone();
    written.graph_stats = Some(content.graph_stats());
    save_manifest(manifest_path_in(pack_dir), &written)?;

    Ok(BuiltPack {
        name: written.name.clone(),
        version: written.version.clone(),
        path: pack_dir.to_path_buf(),
        manifest: written,
    })
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
