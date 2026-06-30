//! Internal helpers shared across the ingestion pipeline.

use std::time::{SystemTime, UNIX_EPOCH};

use kgpacks_db::Value;

/// Current wall-clock time as epoch-milliseconds.
///
/// The working store records `claimed_at` / `processed_at` as `INT64`
/// epoch-millis (see [`crate::schema`]); this is the single source of "now" for
/// the work queue, so stale-claim reclamation is a plain integer comparison.
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Read a `String` from an optional row value (`None`/`Null`/non-string → `None`).
pub(crate) fn value_as_string(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Read an `i64` from an optional row value, widening `Int32` (`None` otherwise).
pub(crate) fn value_as_i64(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Int64(n)) => Some(*n),
        Some(Value::Int32(n)) => Some(i64::from(*n)),
        _ => None,
    }
}
