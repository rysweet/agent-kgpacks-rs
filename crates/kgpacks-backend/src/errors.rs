//! `kgpacks-backend` — error model.
//!
//! Rust port of `@kgpacks/backend`'s `errors.ts`. Every failure the API surfaces
//! uses one envelope shape:
//!
//! ```json
//! { "error": { "code": "…", "message": "…", "details": null }, "timestamp": "…" }
//! ```
//!
//! [`ApiError`] carries the HTTP status + a stable machine [`ErrorCode`]; a caller
//! renders it into the [`ErrorEnvelope`] (JSON via [`ApiError::to_envelope`] /
//! [`ErrorEnvelope::to_json`]). Ported from the reference backend's JSON error
//! bodies.

use std::time::{SystemTime, UNIX_EPOCH};

/// Stable machine-readable error codes returned in the envelope.
///
/// Mirrors the TypeScript `ErrorCode` union (the subset the current routes use).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// A required parameter was absent (`400`).
    MissingParameter,
    /// A parameter was present but failed validation (`400`).
    InvalidParameter,
    /// The requested resource (e.g. a seed entity) does not exist (`404`).
    NotFound,
    /// An unexpected server-side failure (`500`).
    InternalError,
}

impl ErrorCode {
    /// The stable wire string for this code (e.g. `"MISSING_PARAMETER"`).
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::MissingParameter => "MISSING_PARAMETER",
            ErrorCode::InvalidParameter => "INVALID_PARAMETER",
            ErrorCode::NotFound => "NOT_FOUND",
            ErrorCode::InternalError => "INTERNAL_ERROR",
        }
    }
}

/// An error carrying the HTTP status + envelope `code`/`message` to return.
///
/// Services return these (e.g. [`ApiError::not_found`]); a route renders them
/// into the [`ErrorEnvelope`]. Mirrors the TypeScript `ApiError` class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    /// HTTP status code (`400`, `404`, `500`).
    pub status_code: u16,
    /// Stable machine-readable code.
    pub code: ErrorCode,
    /// Human-readable message.
    pub message: String,
}

impl ApiError {
    /// Construct an error with an explicit status, code and message.
    pub fn new(status_code: u16, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status_code,
            code,
            message: message.into(),
        }
    }

    /// `400 MISSING_PARAMETER` — a required parameter is absent.
    pub fn missing_parameter(message: impl Into<String>) -> Self {
        Self::new(400, ErrorCode::MissingParameter, message)
    }

    /// `400 INVALID_PARAMETER` — a parameter failed validation.
    pub fn invalid_parameter(message: impl Into<String>) -> Self {
        Self::new(400, ErrorCode::InvalidParameter, message)
    }

    /// `404 NOT_FOUND` — the requested resource does not exist.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(404, ErrorCode::NotFound, message)
    }

    /// `500 INTERNAL_ERROR` — an unexpected server-side failure.
    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::new(500, ErrorCode::InternalError, message)
    }

    /// Render this error as the standard [`ErrorEnvelope`].
    pub fn to_envelope(&self) -> ErrorEnvelope {
        ErrorEnvelope {
            code: self.code,
            message: self.message.clone(),
            timestamp: now_iso(),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code.as_str())
    }
}

impl std::error::Error for ApiError {}

/// The standard error envelope serialized to clients.
///
/// Mirrors the TypeScript `ErrorEnvelope` (`details` is always `null` for the
/// current routes, so it is fixed to `null` in [`ErrorEnvelope::to_json`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorEnvelope {
    /// Stable machine-readable code.
    pub code: ErrorCode,
    /// Human-readable message.
    pub message: String,
    /// ISO-8601 UTC timestamp (trailing `Z`).
    pub timestamp: String,
}

impl ErrorEnvelope {
    /// Serialize to the wire JSON shape
    /// `{ "error": { "code", "message", "details" }, "timestamp" }`.
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "error": {
                "code": self.code.as_str(),
                "message": self.message,
                "details": serde_json::Value::Null,
            },
            "timestamp": self.timestamp,
        })
        .to_string()
    }
}

/// Current time as an ISO-8601 string with a trailing `Z` (UTC), e.g.
/// `2026-07-06T10:05:10.812Z`.
///
/// Mirrors the reference `new Date().toISOString()`. Implemented from
/// [`SystemTime`] via the civil-from-days algorithm so the crate needs no date
/// dependency. A pre-epoch clock (which should not occur on a server) renders the
/// epoch itself.
pub fn now_iso() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    iso_from_epoch_millis(millis)
}

/// Format epoch-milliseconds as an ISO-8601 UTC timestamp with millisecond
/// precision and a trailing `Z`.
fn iso_from_epoch_millis(millis: i64) -> String {
    let millis = millis.max(0);
    let secs = millis / 1000;
    let ms = millis % 1000;
    let days = secs / 86_400;
    let secs_of_day = secs % 86_400;
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{ms:03}Z")
}

/// Convert a count of days since the Unix epoch (1970-01-01) into a
/// `(year, month, day)` proleptic-Gregorian civil date. Howard Hinnant's
/// `civil_from_days` algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_formats_known_epoch_millis() {
        // 2026-07-06T10:05:10.812Z corresponds to this epoch-millis value.
        assert_eq!(
            iso_from_epoch_millis(1_783_332_310_812),
            "2026-07-06T10:05:10.812Z"
        );
        // The epoch itself.
        assert_eq!(iso_from_epoch_millis(0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn envelope_json_has_the_standard_shape() {
        let err = ApiError::invalid_parameter("depth must be 1..3");
        let json = err.to_envelope().to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["error"]["code"], "INVALID_PARAMETER");
        assert_eq!(parsed["error"]["message"], "depth must be 1..3");
        assert!(parsed["error"]["details"].is_null());
        assert!(parsed["timestamp"].as_str().unwrap().ends_with('Z'));
    }
}
