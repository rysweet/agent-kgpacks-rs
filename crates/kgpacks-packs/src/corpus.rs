//! The CVE corpus seam consumed by the resumable pack builder.
//!
//! The builder in [`crate::cve_build`] embeds and loads CVE records in stable
//! order. It only needs two things from its input: a **total count** and
//! **random access by offset** (so a build interrupted after batch *N* can
//! resume from the checkpointed source offset without re-reading earlier
//! records). Those two operations are the [`CorpusSource`] trait.
//!
//! This is the integration seam for the external CVE-corpus fetch (issue #25):
//! that work lands a live [`CorpusSource`] (streaming from `CVEProject/cvelistV5`)
//! and the builder consumes it unchanged. Until then — and for hermetic unit
//! tests — [`FixtureCorpus`] provides an in-memory, `Vec`-backed corpus that can
//! also be loaded from a JSON file, so the whole resumable/pipelined build path
//! is exercisable today.

use std::path::Path;

use serde_json::Value;

use crate::errors::{PacksError, Result};

/// An entity mentioned by a CVE record (e.g. an affected product or vendor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CveEntity {
    /// Stable entity identifier (primary key within the pack).
    pub entity_id: String,
    /// Display name.
    pub name: String,
    /// Entity type label (e.g. `product`, `vendor`, `weakness`).
    pub type_: String,
    /// Free-text description.
    pub description: String,
}

/// A directed relation between two of a record's entities.
///
/// Only materialized into the pack when the build enables
/// `with_entity_relations` (see [`crate::cve_build::BuildParams`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CveRelation {
    /// `entity_id` of the source entity.
    pub source_id: String,
    /// `entity_id` of the target entity.
    pub target_id: String,
    /// Relation label.
    pub relation: String,
    /// Free-text context for the relation.
    pub context: String,
}

/// A single CVE record — the unit the builder embeds (its description) and
/// loads (as an `Article` node plus its entities/relations).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CveRecord {
    /// CVE identifier, e.g. `"CVE-2025-0074"` (the `Article` primary key).
    pub id: String,
    /// The record's descriptive text; this is what gets embedded.
    pub description: String,
    /// Publication year, when known.
    pub published_year: Option<i64>,
    /// Entities mentioned by the record.
    pub entities: Vec<CveEntity>,
    /// Entity-to-entity relations for the record.
    pub relations: Vec<CveRelation>,
}

impl CveRecord {
    /// A minimal record with just an id and description (no entities/relations).
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            published_year: None,
            entities: Vec::new(),
            relations: Vec::new(),
        }
    }
}

/// A corpus of CVE records in a stable, offset-addressable order.
///
/// Implementations must return the **same record for the same offset** across
/// calls within a build, and must be safe to share across threads (`Sync`) so
/// the builder's embedding stage can read records from a worker thread while the
/// load stage writes to the database on the main thread.
pub trait CorpusSource: Sync {
    /// Total number of records available.
    fn len(&self) -> usize;

    /// Whether the corpus is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The record at `offset`, or `None` if `offset >= len()`.
    fn record(&self, offset: usize) -> Option<CveRecord>;
}

/// An in-memory, `Vec`-backed [`CorpusSource`] for tests and for the CLI's
/// file-driven build path (until the live #25 fetch lands).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FixtureCorpus {
    records: Vec<CveRecord>,
}

impl FixtureCorpus {
    /// Build a fixture corpus from an ordered list of records.
    pub fn new(records: Vec<CveRecord>) -> Self {
        Self { records }
    }

    /// The records, in order.
    pub fn records(&self) -> &[CveRecord] {
        &self.records
    }

    /// Parse a corpus from a JSON array of record objects.
    ///
    /// Each element must be an object with a string `id` and string
    /// `description`; `published_year` (integer), `entities` and `relations`
    /// (arrays of objects) are optional. Unknown keys are ignored, so the
    /// on-disk format can grow without breaking older builders.
    pub fn from_json_str(raw: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(raw)
            .map_err(|err| PacksError::Corpus(format!("corpus is not valid JSON: {err}")))?;
        let array = value.as_array().ok_or_else(|| {
            PacksError::Corpus("corpus must be a JSON array of records".to_string())
        })?;
        let mut records = Vec::with_capacity(array.len());
        for (i, item) in array.iter().enumerate() {
            records.push(parse_record(item, i)?);
        }
        Ok(Self { records })
    }

    /// Read and parse a corpus from a JSON file.
    pub fn from_json_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path).map_err(|err| {
            PacksError::Corpus(format!("cannot read corpus at {}: {err}", path.display()))
        })?;
        Self::from_json_str(&raw).map_err(|err| match err {
            PacksError::Corpus(message) => {
                PacksError::Corpus(format!("corpus at {}: {message}", path.display()))
            }
            other => other,
        })
    }
}

impl CorpusSource for FixtureCorpus {
    fn len(&self) -> usize {
        self.records.len()
    }

    fn record(&self, offset: usize) -> Option<CveRecord> {
        self.records.get(offset).cloned()
    }
}

fn parse_record(item: &Value, index: usize) -> Result<CveRecord> {
    let object = item
        .as_object()
        .ok_or_else(|| PacksError::Corpus(format!("record {index} must be a JSON object")))?;
    let id = required_str(object.get("id"), index, "id")?;
    let description = required_str(object.get("description"), index, "description")?;
    let published_year = match object.get("published_year") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_i64().ok_or_else(|| {
            PacksError::Corpus(format!(
                "record {index} `published_year` must be an integer"
            ))
        })?),
    };
    let entities = parse_entities(object.get("entities"), index)?;
    let relations = parse_relations(object.get("relations"), index)?;
    Ok(CveRecord {
        id,
        description,
        published_year,
        entities,
        relations,
    })
}

fn parse_entities(value: Option<&Value>, index: usize) -> Result<Vec<CveEntity>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let array = value
        .as_array()
        .ok_or_else(|| PacksError::Corpus(format!("record {index} `entities` must be an array")))?;
    let mut out = Vec::with_capacity(array.len());
    for entity in array {
        let object = entity.as_object().ok_or_else(|| {
            PacksError::Corpus(format!("record {index} entity must be an object"))
        })?;
        out.push(CveEntity {
            entity_id: required_str(object.get("entity_id"), index, "entity.entity_id")?,
            name: optional_str(object.get("name")),
            type_: optional_str(object.get("type")),
            description: optional_str(object.get("description")),
        });
    }
    Ok(out)
}

fn parse_relations(value: Option<&Value>, index: usize) -> Result<Vec<CveRelation>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let array = value.as_array().ok_or_else(|| {
        PacksError::Corpus(format!("record {index} `relations` must be an array"))
    })?;
    let mut out = Vec::with_capacity(array.len());
    for relation in array {
        let object = relation.as_object().ok_or_else(|| {
            PacksError::Corpus(format!("record {index} relation must be an object"))
        })?;
        out.push(CveRelation {
            source_id: required_str(object.get("source_id"), index, "relation.source_id")?,
            target_id: required_str(object.get("target_id"), index, "relation.target_id")?,
            relation: optional_str(object.get("relation")),
            context: optional_str(object.get("context")),
        });
    }
    Ok(out)
}

fn required_str(value: Option<&Value>, index: usize, field: &str) -> Result<String> {
    match value {
        Some(Value::String(s)) if !s.is_empty() => Ok(s.clone()),
        _ => Err(PacksError::Corpus(format!(
            "record {index} is missing required string `{field}`"
        ))),
    }
}

fn optional_str(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_minimal_corpus() {
        let corpus =
            FixtureCorpus::from_json_str(r#"[{"id":"CVE-2025-0001","description":"a flaw"}]"#)
                .expect("parse");
        assert_eq!(corpus.len(), 1);
        let record = corpus.record(0).expect("record");
        assert_eq!(record.id, "CVE-2025-0001");
        assert_eq!(record.description, "a flaw");
        assert!(record.entities.is_empty());
        assert!(record.relations.is_empty());
        assert_eq!(record.published_year, None);
    }

    #[test]
    fn parses_entities_and_relations() {
        let corpus = FixtureCorpus::from_json_str(
            r#"[{
                "id":"CVE-2025-0002",
                "description":"rce in acme web",
                "published_year": 2025,
                "entities":[
                    {"entity_id":"prod:acme-web","name":"Acme Web","type":"product","description":"the server"},
                    {"entity_id":"vendor:acme","name":"Acme","type":"vendor"}
                ],
                "relations":[
                    {"source_id":"prod:acme-web","target_id":"vendor:acme","relation":"made_by","context":"vendor"}
                ]
            }]"#,
        )
        .expect("parse");
        let record = corpus.record(0).expect("record");
        assert_eq!(record.published_year, Some(2025));
        assert_eq!(record.entities.len(), 2);
        assert_eq!(record.entities[1].name, "Acme");
        assert_eq!(record.relations.len(), 1);
        assert_eq!(record.relations[0].relation, "made_by");
    }

    #[test]
    fn record_out_of_range_is_none() {
        let corpus = FixtureCorpus::new(vec![CveRecord::new("CVE-2025-0003", "x")]);
        assert!(corpus.record(0).is_some());
        assert!(corpus.record(1).is_none());
    }

    #[test]
    fn rejects_a_non_array_corpus() {
        assert!(FixtureCorpus::from_json_str(r#"{"id":"x"}"#).is_err());
    }

    #[test]
    fn rejects_a_record_missing_its_id() {
        assert!(FixtureCorpus::from_json_str(r#"[{"description":"no id"}]"#).is_err());
    }
}
