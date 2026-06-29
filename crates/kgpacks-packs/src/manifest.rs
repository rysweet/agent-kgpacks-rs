//! Pack manifest model + validation.
//!
//! Ports `packages/packs/src/manifest.ts`. The on-disk format is the unchanged
//! `manifest.json` with snake_case keys, so packs written by the upstream tooling
//! load byte-for-byte. [`validate_manifest`] is the single schema gate the rest
//! of the crate calls: it errors on any violation and strips prototype-pollution
//! keys when rebuilding the result.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Map, Value};

use crate::errors::{PacksError, Result};
use crate::versioning::is_valid_semver;

/// Canonical manifest filename inside a pack directory.
pub const MANIFEST_FILENAME: &str = "manifest.json";

/// Keys reserved by the typed manifest fields; everything else is preserved as
/// [`PackManifest::extra`].
const KNOWN_KEYS: [&str; 5] = [
    "name",
    "version",
    "description",
    "graph_stats",
    "eval_scores",
];

/// Keys stripped to guard against prototype-pollution from untrusted manifests.
const DANGEROUS_KEYS: [&str; 3] = ["__proto__", "constructor", "prototype"];

/// Pack name pattern: 1–64 chars, alphanumeric lead, then ASCII
/// letters/digits/underscore/hyphen. Anchored + bounded ⇒ ReDoS-safe.
pub fn pack_name_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}$").expect("valid PACK_NAME_RE"))
}

/// Metadata describing an installable knowledge pack.
///
/// Mirrors the TypeScript `PackManifest`. `name` and `version` are required and
/// validated; `description`, `graph_stats` and `eval_scores` are optional; any
/// other keys are preserved verbatim in [`PackManifest::extra`].
#[derive(Debug, Clone, PartialEq)]
pub struct PackManifest {
    /// Pack name (must match [`pack_name_re`]).
    pub name: String,
    /// Semantic version string (must be valid SemVer 2.0).
    pub version: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// Optional graph statistics; every value must be a non-negative finite number.
    pub graph_stats: Option<BTreeMap<String, f64>>,
    /// Optional evaluation scores; every value must be a finite number.
    pub eval_scores: Option<BTreeMap<String, f64>>,
    /// Any additional (unknown) manifest keys, preserved verbatim.
    pub extra: BTreeMap<String, Value>,
}

impl PackManifest {
    /// Construct a minimal manifest from a name and version.
    ///
    /// This does not validate; validation happens at the [`validate_manifest`],
    /// [`save_manifest`] and [`load_manifest`] boundaries (and in
    /// [`crate::build_pack`], which validates before writing).
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            description: None,
            graph_stats: None,
            eval_scores: None,
            extra: BTreeMap::new(),
        }
    }

    /// Canonical `name@version` identifier.
    pub fn id(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }

    /// Serialize to a canonical JSON object (snake_case keys, known fields first).
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("name".into(), Value::String(self.name.clone()));
        map.insert("version".into(), Value::String(self.version.clone()));
        if let Some(description) = &self.description {
            map.insert("description".into(), Value::String(description.clone()));
        }
        if let Some(graph_stats) = &self.graph_stats {
            map.insert("graph_stats".into(), number_map_to_value(graph_stats));
        }
        if let Some(eval_scores) = &self.eval_scores {
            map.insert("eval_scores".into(), number_map_to_value(eval_scores));
        }
        for (key, value) in &self.extra {
            map.insert(key.clone(), value.clone());
        }
        Value::Object(map)
    }
}

fn number_map_to_value(values: &BTreeMap<String, f64>) -> Value {
    let mut map = Map::new();
    for (key, &num) in values {
        // Emit integral counts as integers (e.g. `294`, not `294.0`) to match the
        // reference's on-disk format; non-integral stats (e.g. `size_mb`) stay
        // floats.
        let number = if num.is_finite() && num.fract() == 0.0 && num.abs() < i64::MAX as f64 {
            Some(serde_json::Number::from(num as i64))
        } else {
            serde_json::Number::from_f64(num)
        };
        if let Some(number) = number {
            map.insert(key.clone(), Value::Number(number));
        }
    }
    Value::Object(map)
}

fn validate_number_map(
    field: &str,
    value: &Value,
    allow_negative: bool,
) -> Result<BTreeMap<String, f64>> {
    let object = value
        .as_object()
        .ok_or_else(|| PacksError::ManifestValidation(format!("{field} must be an object")))?;
    let mut out = BTreeMap::new();
    for (key, raw) in object {
        let num = raw.as_f64().filter(|n| n.is_finite()).ok_or_else(|| {
            PacksError::ManifestValidation(if allow_negative {
                format!("{field}.{key} must be a finite number")
            } else {
                format!("{field}.{key} must be a non-negative finite number")
            })
        })?;
        if !allow_negative && num < 0.0 {
            return Err(PacksError::ManifestValidation(format!(
                "{field}.{key} must be a non-negative finite number"
            )));
        }
        out.insert(key.clone(), num);
    }
    Ok(out)
}

/// Validate an arbitrary JSON value as a [`PackManifest`].
///
/// Errors with [`PacksError::ManifestValidation`] on any violation. Optional
/// sections present-but-`null` are treated as absent (real catalog manifests
/// carry `eval_scores: null`). Dangerous keys are stripped from [`PackManifest::extra`].
pub fn validate_manifest(value: &Value) -> Result<PackManifest> {
    let object = value
        .as_object()
        .ok_or_else(|| PacksError::ManifestValidation("manifest must be a JSON object".into()))?;

    let name = match object.get("name") {
        Some(Value::String(s)) if pack_name_re().is_match(s) => s.clone(),
        other => {
            return Err(PacksError::ManifestValidation(format!(
                "invalid pack name {} (must match PACK_NAME_RE)",
                render(other)
            )))
        }
    };

    let version = match object.get("version") {
        Some(Value::String(s)) if is_valid_semver(s) => s.clone(),
        other => {
            return Err(PacksError::ManifestValidation(format!(
                "invalid version {} (must be valid SemVer 2.0)",
                render(other)
            )))
        }
    };

    let description = match object.get("description") {
        None => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => {
            return Err(PacksError::ManifestValidation(
                "description must be a string".into(),
            ))
        }
    };

    let graph_stats = match object.get("graph_stats") {
        None | Some(Value::Null) => None,
        Some(value) => Some(validate_number_map("graph_stats", value, false)?),
    };

    let eval_scores = match object.get("eval_scores") {
        None | Some(Value::Null) => None,
        Some(value) => Some(validate_number_map("eval_scores", value, true)?),
    };

    let mut extra = BTreeMap::new();
    for (key, value) in object {
        if DANGEROUS_KEYS.contains(&key.as_str()) {
            continue;
        }
        if KNOWN_KEYS.contains(&key.as_str()) {
            // Non-null known keys are represented by the typed fields above and
            // re-emitted by `to_value`. Preserve an explicit `null` for an
            // optional section (the reference re-emits `eval_scores: null`).
            if value.is_null() && (key == "graph_stats" || key == "eval_scores") {
                extra.insert(key.clone(), Value::Null);
            }
            continue;
        }
        extra.insert(key.clone(), value.clone());
    }

    Ok(PackManifest {
        name,
        version,
        description,
        graph_stats,
        eval_scores,
        extra,
    })
}

fn render(value: Option<&Value>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "undefined".to_string(),
    }
}

/// Parse and validate a manifest from a JSON string.
pub fn parse_manifest_str(raw: &str) -> Result<PackManifest> {
    let parsed: Value = serde_json::from_str(raw).map_err(|err| {
        PacksError::ManifestValidation(format!("manifest is not valid JSON: {err}"))
    })?;
    validate_manifest(&parsed)
}

/// Read, parse and validate a manifest from a file path.
pub fn load_manifest(manifest_path: impl AsRef<Path>) -> Result<PackManifest> {
    let path = manifest_path.as_ref();
    let raw = std::fs::read_to_string(path).map_err(|err| {
        PacksError::ManifestValidation(format!("cannot read manifest at {}: {err}", path.display()))
    })?;
    parse_manifest_str(&raw).map_err(|err| match err {
        PacksError::ManifestValidation(message) => {
            PacksError::ManifestValidation(format!("manifest at {}: {message}", path.display()))
        }
        other => other,
    })
}

/// Read, parse and validate the `manifest.json` inside a pack directory.
pub fn load_manifest_from_dir(pack_dir: impl AsRef<Path>) -> Result<PackManifest> {
    load_manifest(manifest_path_in(pack_dir))
}

/// Validate `manifest` and write it as pretty JSON (trailing newline) to `path`.
pub fn save_manifest(manifest_path: impl AsRef<Path>, manifest: &PackManifest) -> Result<()> {
    let valid = validate_manifest(&manifest.to_value())?;
    let mut json = serde_json::to_string_pretty(&valid.to_value()).map_err(|err| {
        PacksError::ManifestValidation(format!("cannot serialize manifest: {err}"))
    })?;
    json.push('\n');
    std::fs::write(manifest_path, json)?;
    Ok(())
}

/// The `manifest.json` path inside a pack directory.
pub fn manifest_path_in(pack_dir: impl AsRef<Path>) -> PathBuf {
    pack_dir.as_ref().join(MANIFEST_FILENAME)
}
