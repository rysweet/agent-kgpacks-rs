//! `kgpacks-backend` — entity-graph API surface. `GET /api/v1/graph/entities`.
//!
//! Rust port of `@kgpacks/backend`'s `routes/graph-entities.ts` +
//! `services/graph-entities.ts` + the `graphEntitiesQuerySchema` half of
//! `schemas.ts`. Exposes the entity neighborhood over `Entity` / `HAS_ENTITY` /
//! `ENTITY_RELATION`.
//!
//! The reference runs on Fastify, whose JSON schema validates + coerces the query
//! string before the handler runs. This crate has no HTTP server yet (that lands
//! with the rest of the transport in M5), so the schema is expressed as the
//! explicit [`validate_query`] step: it takes the raw (string) query parameters a
//! querystring would carry and enforces the same contract —
//!
//!   * `entity` — required, non-empty, `<= 500` chars;
//!   * `depth` — integer `1..=3`, default `1`;
//!   * `limit` — integer `1..=200`, default `50`;
//!   * `type` — optional, `<= 200` chars;
//!   * `mode` — enum `auto | co-occurrence | relation`, default `auto`.
//!
//! A missing `entity` renders as a `MISSING_PARAMETER` 400 envelope; any other
//! validation failure as `INVALID_PARAMETER` 400; an unknown seed entity as a
//! `NOT_FOUND` 404 from the service.

use kgpacks_db::Connection;
use kgpacks_query::{
    entity_graph, EntityGraphMode, EntityGraphOptions, EntityGraphResult, QueryError,
};

use crate::errors::ApiError;

/// Upper bound on the `entity` seed length (parity with the reference schema's
/// `maxLength: 500`).
const MAX_ENTITY_LEN: usize = 500;
/// Upper bound on the `type` filter length (parity with `maxLength: 200`).
const MAX_TYPE_LEN: usize = 200;
/// Inclusive `depth` bounds (parity with `minimum: 1, maximum: 3`).
const MIN_DEPTH: i64 = 1;
const MAX_DEPTH: i64 = 3;
/// Default `depth` when omitted (parity with the schema `default: 1`).
const DEFAULT_DEPTH: i64 = 1;
/// Inclusive `limit` bounds (parity with `minimum: 1, maximum: 200`).
const MIN_LIMIT: i64 = 1;
const MAX_LIMIT: i64 = 200;
/// Default `limit` when omitted (parity with the schema `default: 50`).
const DEFAULT_LIMIT: i64 = 50;

/// The raw `GET /api/v1/graph/entities` query parameters, as a querystring would
/// carry them (every value a string). [`validate_query`] parses and bounds these.
#[derive(Debug, Clone, Default)]
pub struct GraphEntitiesQuery {
    /// The `entity` seed id.
    pub entity: Option<String>,
    /// The `depth` radius (parsed as an integer).
    pub depth: Option<String>,
    /// The `limit` node cap (parsed as an integer).
    pub limit: Option<String>,
    /// The optional `type` filter.
    pub type_filter: Option<String>,
    /// The `mode` enum.
    pub mode: Option<String>,
}

impl GraphEntitiesQuery {
    /// Build a query from decoded `(key, value)` querystring pairs, keeping the
    /// last value for a repeated key (mirroring typical querystring parsing).
    /// Unknown keys are ignored.
    pub fn from_pairs<'a, I>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        let mut query = GraphEntitiesQuery::default();
        for (key, value) in pairs {
            let slot = match key {
                "entity" => &mut query.entity,
                "depth" => &mut query.depth,
                "limit" => &mut query.limit,
                "type" => &mut query.type_filter,
                "mode" => &mut query.mode,
                _ => continue,
            };
            *slot = Some(value.to_string());
        }
        query
    }
}

/// The validated, bounded parameters produced from a [`GraphEntitiesQuery`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedQuery {
    /// The (non-empty) seed entity id.
    pub entity: String,
    /// The bounded depth (`1..=3`).
    pub depth: i64,
    /// The bounded node limit (`1..=200`).
    pub limit: usize,
    /// The optional type filter.
    pub type_filter: Option<String>,
    /// The parsed traversal mode.
    pub mode: EntityGraphMode,
}

impl ValidatedQuery {
    /// Map the validated params onto the query crate's [`EntityGraphOptions`].
    fn to_options(&self) -> EntityGraphOptions {
        EntityGraphOptions {
            entity: self.entity.clone(),
            depth: Some(self.depth),
            type_filter: self.type_filter.clone(),
            mode: self.mode,
            limit: Some(self.limit),
        }
    }
}

/// Validate + bound a raw [`GraphEntitiesQuery`], enforcing the request contract.
///
/// Returns a [`ValidatedQuery`] on success, or an [`ApiError`] rendering the
/// standard 400 envelope: `MISSING_PARAMETER` for an absent `entity`,
/// `INVALID_PARAMETER` for any other violation.
pub fn validate_query(query: &GraphEntitiesQuery) -> Result<ValidatedQuery, ApiError> {
    // `entity` — required, non-empty, bounded length.
    let entity = match query.entity.as_deref() {
        None => {
            return Err(ApiError::missing_parameter(
                "Missing required parameter: entity",
            ))
        }
        Some("") => return Err(ApiError::invalid_parameter("entity must not be empty")),
        Some(value) if value.chars().count() > MAX_ENTITY_LEN => {
            return Err(ApiError::invalid_parameter(format!(
                "entity must be at most {MAX_ENTITY_LEN} characters"
            )))
        }
        Some(value) => value.to_string(),
    };

    // `depth` — integer 1..=3, default 1.
    let depth = parse_bounded_int(
        query.depth.as_deref(),
        "depth",
        MIN_DEPTH,
        MAX_DEPTH,
        DEFAULT_DEPTH,
    )?;

    // `limit` — integer 1..=200, default 50.
    let limit = parse_bounded_int(
        query.limit.as_deref(),
        "limit",
        MIN_LIMIT,
        MAX_LIMIT,
        DEFAULT_LIMIT,
    )?;

    // `type` — optional, bounded length.
    let type_filter = match query.type_filter.as_deref() {
        Some(value) if value.chars().count() > MAX_TYPE_LEN => {
            return Err(ApiError::invalid_parameter(format!(
                "type must be at most {MAX_TYPE_LEN} characters"
            )))
        }
        Some(value) => Some(value.to_string()),
        None => None,
    };

    // `mode` — enum, default auto.
    let mode = match query.mode.as_deref() {
        None => EntityGraphMode::Auto,
        Some(value) => EntityGraphMode::parse(value).ok_or_else(|| {
            ApiError::invalid_parameter(format!(
                "mode must be one of auto, co-occurrence, relation, got {value}"
            ))
        })?,
    };

    Ok(ValidatedQuery {
        entity,
        depth,
        limit: limit as usize,
        type_filter,
        mode,
    })
}

/// Parse an optional string integer parameter, applying `default` when absent and
/// enforcing the inclusive `[min, max]` bound. A non-integer or out-of-range value
/// yields an `INVALID_PARAMETER` [`ApiError`].
fn parse_bounded_int(
    raw: Option<&str>,
    name: &str,
    min: i64,
    max: i64,
    default: i64,
) -> Result<i64, ApiError> {
    let value = match raw {
        None => return Ok(default),
        Some(text) => text.trim().parse::<i64>().map_err(|_| {
            ApiError::invalid_parameter(format!("{name} must be an integer, got {text}"))
        })?,
    };
    if value < min || value > max {
        return Err(ApiError::invalid_parameter(format!(
            "{name} must be between {min} and {max}, got {value}"
        )));
    }
    Ok(value)
}

/// Build the entity neighborhood for already-validated params, mapping the query
/// crate's typed failures onto the API error envelope: an unknown seed → `404`
/// `NOT_FOUND`; an out-of-range depth → `400` `INVALID_PARAMETER` (defensive —
/// depth is already bounded by [`validate_query`]); any other failure (a driver
/// error) → `500` `INTERNAL_ERROR`.
///
/// Mirrors the reference `getEntityGraph`.
pub fn get_entity_graph(
    conn: &Connection<'_>,
    query: &ValidatedQuery,
) -> Result<EntityGraphResult, ApiError> {
    entity_graph(conn, &query.to_options()).map_err(|err| match err {
        QueryError::EntityNotFound(entity) => {
            ApiError::not_found(format!("Entity not found: {entity}"))
        }
        QueryError::InvalidArgument(message) => ApiError::invalid_parameter(message),
        other => ApiError::internal_error(other.to_string()),
    })
}

/// The `GET /api/v1/graph/entities` handler: validate the raw query, then build
/// the neighborhood. Combines [`validate_query`] and [`get_entity_graph`] into the
/// single call a route makes.
pub fn graph_entities(
    conn: &Connection<'_>,
    query: &GraphEntitiesQuery,
) -> Result<EntityGraphResult, ApiError> {
    let validated = validate_query(query)?;
    get_entity_graph(conn, &validated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::ErrorCode;

    #[test]
    fn missing_entity_is_a_missing_parameter() {
        let err = validate_query(&GraphEntitiesQuery::default()).unwrap_err();
        assert_eq!(err.code, ErrorCode::MissingParameter);
        assert_eq!(err.status_code, 400);
    }

    #[test]
    fn defaults_are_applied() {
        let query = GraphEntitiesQuery {
            entity: Some("Rust|Cargo".to_string()),
            ..Default::default()
        };
        let validated = validate_query(&query).unwrap();
        assert_eq!(validated.depth, 1);
        assert_eq!(validated.limit, 50);
        assert_eq!(validated.mode, EntityGraphMode::Auto);
        assert!(validated.type_filter.is_none());
    }

    #[test]
    fn out_of_range_depth_is_invalid() {
        let query = GraphEntitiesQuery {
            entity: Some("e".to_string()),
            depth: Some("4".to_string()),
            ..Default::default()
        };
        let err = validate_query(&query).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    #[test]
    fn non_integer_limit_is_invalid() {
        let query = GraphEntitiesQuery {
            entity: Some("e".to_string()),
            limit: Some("abc".to_string()),
            ..Default::default()
        };
        let err = validate_query(&query).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    #[test]
    fn out_of_range_limit_is_invalid() {
        let query = GraphEntitiesQuery {
            entity: Some("e".to_string()),
            limit: Some("201".to_string()),
            ..Default::default()
        };
        assert_eq!(
            validate_query(&query).unwrap_err().code,
            ErrorCode::InvalidParameter
        );
    }

    #[test]
    fn unknown_mode_is_invalid() {
        let query = GraphEntitiesQuery {
            entity: Some("e".to_string()),
            mode: Some("sideways".to_string()),
            ..Default::default()
        };
        assert_eq!(
            validate_query(&query).unwrap_err().code,
            ErrorCode::InvalidParameter
        );
    }

    #[test]
    fn valid_mode_and_type_pass() {
        let query = GraphEntitiesQuery::from_pairs([
            ("entity", "Rust|Cargo"),
            ("depth", "2"),
            ("limit", "10"),
            ("type", "tool"),
            ("mode", "relation"),
        ]);
        let validated = validate_query(&query).unwrap();
        assert_eq!(validated.entity, "Rust|Cargo");
        assert_eq!(validated.depth, 2);
        assert_eq!(validated.limit, 10);
        assert_eq!(validated.type_filter.as_deref(), Some("tool"));
        assert_eq!(validated.mode, EntityGraphMode::Relation);
    }
}
