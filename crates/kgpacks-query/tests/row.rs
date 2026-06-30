//! Edge-case tests for the result-row coercion helpers (`row.ts` parity).
//!
//! These normalize the loosely-typed [`Value`]s LadybugDB returns (integer or
//! string primary keys, arbitrary column values) into the strict public shapes,
//! and underpin the score arithmetic (`clamp01(1 - distance)`) and id
//! stringification every retrieval mode relies on. Getting them right is a
//! precondition for correct ranking.

use kgpacks_db::{LogicalType, Value};
use kgpacks_query::row::{clamp01, coerce_content, to_id_string};

// ── to_id_string ────────────────────────────────────────────────────────────

#[test]
fn to_id_string_stringifies_integer_keys() {
    assert_eq!(to_id_string(&Value::Int64(42)), "42");
    assert_eq!(to_id_string(&Value::Int64(0)), "0");
    assert_eq!(to_id_string(&Value::Int32(7)), "7");
}

#[test]
fn to_id_string_preserves_full_precision_for_large_int64() {
    // 2^53 + 1 — not representable exactly as an f64; an i64 round-trips it
    // exactly, with none of the `Number.MAX_SAFE_INTEGER` loss the reference's
    // bigint branch had to guard against.
    assert_eq!(
        to_id_string(&Value::Int64(9_007_199_254_740_993)),
        "9007199254740993"
    );
}

#[test]
fn to_id_string_passes_through_string_keys() {
    // The RS pack schema keys `Section` by a STRING `id`.
    assert_eq!(
        to_id_string(&Value::String("section-abc".into())),
        "section-abc"
    );
    assert_eq!(to_id_string(&Value::String(String::new())), "");
}

// ── coerce_content ──────────────────────────────────────────────────────────

#[test]
fn coerce_content_returns_strings_unchanged() {
    assert_eq!(
        coerce_content(&Value::String("hello world".into())),
        "hello world"
    );
    assert_eq!(coerce_content(&Value::String(String::new())), "");
}

#[test]
fn coerce_content_maps_null_to_empty_string() {
    assert_eq!(coerce_content(&Value::Null(LogicalType::String)), "");
}

#[test]
fn coerce_content_stringifies_other_values() {
    assert_eq!(coerce_content(&Value::Int64(123)), "123");
    assert_eq!(coerce_content(&Value::Bool(true)), "True");
}

// ── clamp01 ─────────────────────────────────────────────────────────────────

#[test]
fn clamp01_passes_through_values_in_range() {
    assert_eq!(clamp01(0.0), 0.0);
    assert_eq!(clamp01(0.5), 0.5);
    assert_eq!(clamp01(1.0), 1.0);
}

#[test]
fn clamp01_clamps_out_of_range_values() {
    assert_eq!(clamp01(-0.3), 0.0); // e.g. cosine distance > 1
    assert_eq!(clamp01(1.2), 1.0); // e.g. tiny negative distance
}
