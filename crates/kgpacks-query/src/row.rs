//! `kgpacks-query` — result-row coercion helpers.
//!
//! Rust port of `@kgpacks/query`'s `row.ts`. LadybugDB returns primary keys as
//! integer or string [`Value`]s and column values as loosely-typed [`Value`]s.
//! These helpers normalize them into the strict public shapes
//! ([`crate::RetrieverResult`]) without scattering matches across the retrieval
//! modules, and underpin the score arithmetic (`clamp01(1 - distance)`) and id
//! stringification every retrieval mode relies on.

use kgpacks_db::Value;

/// Stringifies a node primary key.
///
/// Integer keys (the published-pack `Section.id` may be `INT64`; the TS
/// reference fixtures use `INT64`) are rendered in full precision — an `i64`
/// round-trips exactly, with none of the `Number.MAX_SAFE_INTEGER` loss the
/// TypeScript `bigint` branch guards against. String keys (the RS pack schema's
/// `Section.id STRING`) pass through unchanged. Any other value falls back to
/// its `Display` form.
pub fn to_id_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Int64(n) => n.to_string(),
        Value::Int32(n) => n.to_string(),
        Value::Int16(n) => n.to_string(),
        Value::Int8(n) => n.to_string(),
        Value::UInt64(n) => n.to_string(),
        Value::UInt32(n) => n.to_string(),
        Value::UInt16(n) => n.to_string(),
        Value::UInt8(n) => n.to_string(),
        Value::Int128(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// Coerces a column value to a string, mapping `NULL` to `''`.
///
/// Mirrors the TypeScript `coerceContent`: strings pass through, `null`/`NULL`
/// becomes the empty string, and anything else is rendered via `Display`.
pub fn coerce_content(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null(_) => String::new(),
        other => other.to_string(),
    }
}

/// Clamps a value into the closed unit interval `[0, 1]`.
///
/// Mirrors the TypeScript `clamp01`, used to bound a cosine similarity
/// `1 - distance` (a tiny negative distance can push it above 1; a distance
/// above 1 can push it below 0). Only ever called with finite values — the score
/// path guards `is_finite` before clamping — so the `NaN` edge of
/// [`f64::clamp`] is unreachable here.
pub fn clamp01(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}
