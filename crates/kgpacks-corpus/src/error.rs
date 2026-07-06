//! Error type for the CVE-corpus integration.
//!
//! Ports the reference `CorpusError` (`scripts/cve-corpus.mjs`): a message plus the
//! optional URL that provoked the failure, so the CLI can print a useful diagnostic.
//! Infallible-in-tests I/O and JSON failures fold in through `From` so the orchestrator
//! can use `?`; the real network path adds a feature-gated `reqwest` variant.

use thiserror::Error;

/// Errors raised while acquiring the CVE corpus.
#[derive(Debug, Error)]
pub enum CorpusError {
    /// A domain failure (bad release payload, SSRF violation, missing asset, …).
    /// Carries the offending URL when one is relevant (parity with the reference
    /// `CorpusError { message, url }`).
    #[error("{message}")]
    Corpus {
        /// Human-readable description of what went wrong.
        message: String,
        /// The URL that provoked the failure, if any.
        url: Option<String>,
    },

    /// A filesystem error while creating directories or writing provenance.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// A JSON (de)serialization error.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// A transport error from the real `net` downloader/resolver.
    #[cfg(feature = "net")]
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
}

impl CorpusError {
    /// A domain error with no associated URL.
    pub fn msg(message: impl Into<String>) -> Self {
        Self::Corpus {
            message: message.into(),
            url: None,
        }
    }

    /// A domain error tagged with the URL that provoked it.
    pub fn with_url(message: impl Into<String>, url: impl Into<String>) -> Self {
        Self::Corpus {
            message: message.into(),
            url: Some(url.into()),
        }
    }

    /// The URL associated with this error, if any (used by the CLI to enrich output).
    pub fn url(&self) -> Option<&str> {
        match self {
            Self::Corpus { url, .. } => url.as_deref(),
            _ => None,
        }
    }
}

/// Convenience result alias for fallible corpus operations.
pub type Result<T> = std::result::Result<T, CorpusError>;
