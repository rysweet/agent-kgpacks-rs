//! The build-checkpoint sidecar (`<graph>.build-checkpoint.json`).
//!
//! A resumable build persists just enough state after each committed batch that
//! an interrupted run can pick up where it left off:
//!
//! * `params_hash` — identifies the run's output-affecting inputs (see
//!   [`crate::cve_build::BuildParams::params_hash`]). On resume the recorded hash
//!   must match the current run's, or the build starts clean.
//! * `last_committed_batch` — number of batches fully loaded and committed.
//! * `source_offset` — how many corpus records have been consumed (the resume
//!   cursor into the [`crate::CorpusSource`]).
//! * `counts` — running node/relationship totals, so the resumed run reports
//!   cumulative stats without re-counting.
//!
//! The sidecar is written **atomically** (temp file + rename) after each batch,
//! so a crash mid-write never leaves a torn checkpoint, and is removed on a clean
//! finish so a completed pack carries no resume state.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::errors::{PacksError, Result};

/// Filename suffix appended to the graph-store path for its checkpoint sidecar.
pub const CHECKPOINT_SUFFIX: &str = ".build-checkpoint.json";

/// The checkpoint sidecar path for a graph store at `graph_path`
/// (`<graph_path>.build-checkpoint.json`).
pub fn checkpoint_path_for(graph_path: impl AsRef<Path>) -> PathBuf {
    let mut os = graph_path.as_ref().as_os_str().to_owned();
    os.push(CHECKPOINT_SUFFIX);
    PathBuf::from(os)
}

/// Running node/relationship totals materialized into a pack so far.
///
/// Mirrors the manifest `graph_stats` shape (`articles`, `entities`,
/// `relationships`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BuildCounts {
    /// Number of `Article` (CVE) nodes loaded.
    pub articles: usize,
    /// Number of distinct `Entity` nodes loaded.
    pub entities: usize,
    /// Number of `HAS_ENTITY` relationships loaded.
    pub relationships: usize,
}

/// The persisted state of an in-progress build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildCheckpoint {
    /// Stable hash of the run's output-affecting parameters.
    pub params_hash: String,
    /// Number of batches fully loaded and committed to the store.
    pub last_committed_batch: usize,
    /// Number of corpus records consumed (the resume cursor).
    pub source_offset: usize,
    /// Running node/relationship totals.
    pub counts: BuildCounts,
}

impl BuildCheckpoint {
    /// Serialize to the on-disk JSON object.
    pub fn to_value(&self) -> Value {
        let mut counts = Map::new();
        counts.insert("articles".into(), Value::from(self.counts.articles));
        counts.insert("entities".into(), Value::from(self.counts.entities));
        counts.insert(
            "relationships".into(),
            Value::from(self.counts.relationships),
        );

        let mut map = Map::new();
        map.insert(
            "params_hash".into(),
            Value::String(self.params_hash.clone()),
        );
        map.insert(
            "last_committed_batch".into(),
            Value::from(self.last_committed_batch),
        );
        map.insert("source_offset".into(), Value::from(self.source_offset));
        map.insert("counts".into(), Value::Object(counts));
        Value::Object(map)
    }

    /// Parse a checkpoint from its on-disk JSON.
    pub fn from_value(value: &Value) -> Result<Self> {
        let object = value
            .as_object()
            .ok_or_else(|| PacksError::Checkpoint("checkpoint must be a JSON object".into()))?;
        let params_hash = match object.get("params_hash") {
            Some(Value::String(s)) if !s.is_empty() => s.clone(),
            _ => {
                return Err(PacksError::Checkpoint(
                    "checkpoint is missing a `params_hash` string".into(),
                ))
            }
        };
        let last_committed_batch =
            required_usize(object.get("last_committed_batch"), "last_committed_batch")?;
        let source_offset = required_usize(object.get("source_offset"), "source_offset")?;
        let counts_value = object
            .get("counts")
            .and_then(Value::as_object)
            .ok_or_else(|| PacksError::Checkpoint("checkpoint is missing `counts`".into()))?;
        let counts = BuildCounts {
            articles: required_usize(counts_value.get("articles"), "counts.articles")?,
            entities: required_usize(counts_value.get("entities"), "counts.entities")?,
            relationships: required_usize(
                counts_value.get("relationships"),
                "counts.relationships",
            )?,
        };
        Ok(Self {
            params_hash,
            last_committed_batch,
            source_offset,
            counts,
        })
    }

    /// Write the checkpoint to `path` atomically (temp file + rename), so a crash
    /// during the write can never leave a torn sidecar.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let mut json = serde_json::to_string_pretty(&self.to_value())
            .map_err(|err| PacksError::Checkpoint(format!("cannot serialize checkpoint: {err}")))?;
        json.push('\n');

        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|err| {
            let _ = std::fs::remove_file(&tmp);
            PacksError::Io(err)
        })?;
        std::fs::rename(&tmp, path).map_err(|err| {
            // Don't leave a stray temp file behind if the rename fails.
            let _ = std::fs::remove_file(&tmp);
            PacksError::Io(err)
        })?;
        Ok(())
    }

    /// Read and parse a checkpoint from `path`.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path)?;
        let value: Value = serde_json::from_str(&raw).map_err(|err| {
            PacksError::Checkpoint(format!(
                "checkpoint at {} is not valid JSON: {err}",
                path.display()
            ))
        })?;
        Self::from_value(&value)
    }

    /// Read a checkpoint if `path` exists; `Ok(None)` when it does not.
    pub fn load_if_present(path: impl AsRef<Path>) -> Result<Option<Self>> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(None);
        }
        Self::load(path).map(Some)
    }

    /// Remove the checkpoint sidecar; a no-op if it is already absent.
    pub fn clear(path: impl AsRef<Path>) -> Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(PacksError::Io(err)),
        }
    }
}

fn required_usize(value: Option<&Value>, field: &str) -> Result<usize> {
    value
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| {
            PacksError::Checkpoint(format!(
                "checkpoint field `{field}` must be a non-negative integer"
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> BuildCheckpoint {
        BuildCheckpoint {
            params_hash: "deadbeef".into(),
            last_committed_batch: 3,
            source_offset: 30,
            counts: BuildCounts {
                articles: 30,
                entities: 12,
                relationships: 40,
            },
        }
    }

    #[test]
    fn round_trips_through_value() {
        let cp = sample();
        let parsed = BuildCheckpoint::from_value(&cp.to_value()).expect("parse");
        assert_eq!(parsed, cp);
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = checkpoint_path_for(dir.path().join("graph.lbug"));
        let cp = sample();
        cp.save(&path).expect("save");
        assert!(path.exists());
        let loaded = BuildCheckpoint::load(&path).expect("load");
        assert_eq!(loaded, cp);
    }

    #[test]
    fn checkpoint_path_appends_the_suffix() {
        let path = checkpoint_path_for(Path::new("/tmp/packs/cve/graph.lbug"));
        assert_eq!(
            path,
            Path::new("/tmp/packs/cve/graph.lbug.build-checkpoint.json")
        );
    }

    #[test]
    fn load_if_present_returns_none_when_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = checkpoint_path_for(dir.path().join("graph.lbug"));
        assert_eq!(BuildCheckpoint::load_if_present(&path).expect("load"), None);
    }

    #[test]
    fn clear_is_a_noop_when_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = checkpoint_path_for(dir.path().join("graph.lbug"));
        BuildCheckpoint::clear(&path).expect("clear");
        // And it removes the file when present.
        sample().save(&path).expect("save");
        assert!(path.exists());
        BuildCheckpoint::clear(&path).expect("clear");
        assert!(!path.exists());
    }

    #[test]
    fn rejects_a_torn_checkpoint() {
        assert!(BuildCheckpoint::from_value(&serde_json::json!({
            "params_hash": "x",
            "source_offset": 1
        }))
        .is_err());
    }
}
