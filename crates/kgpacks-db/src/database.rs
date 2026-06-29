//! LadybugDB-backed graph store: a thin, safe wrapper over the `lbug` crate.
//!
//! Rust port of the TypeScript `@kgpacks/db` package (`packages/db/src/database.ts`
//! + `index.ts`). The surface mirrors the reference one-to-one:
//!
//! | TypeScript                         | Rust                                        |
//! | ---------------------------------- | ------------------------------------------- |
//! | `new Database(path?, options?)`    | [`Database::open`] / [`Database::in_memory`]|
//! | `DatabaseOptions`                  | [`DatabaseOptions`]                         |
//! | `database.connect()`               | [`Database::connect`]                       |
//! | `database.close()` (idempotent)    | [`Database::close`]                         |
//! | `connection.run<T>(cypher, params)`| [`Connection::run`] / [`Connection::run_params`] |
//! | `connection.loadExtension(name)`   | [`Connection::load_extension`]              |
//! | `connection.close()` (idempotent)  | [`Connection::close`]                       |
//!
//! Parameters are always **bound** by the driver (prepared-statement execute),
//! never string-interpolated into the query text.

use std::path::Path;

use crate::error::{Error, Result};
use lbug::Value;

/// A single result row keyed by the statement's `RETURN` aliases.
///
/// Mirrors the TypeScript `Row = Record<string, unknown>`. Column order from the
/// query is not preserved (look values up by alias); row order follows the
/// query's `ORDER BY`.
pub type Row = std::collections::HashMap<String, Value>;

/// Tuning options forwarded to the underlying LadybugDB instance.
///
/// Every field is optional; a `None` field falls back to the engine default
/// (mirrors passing `undefined` from the TypeScript `DatabaseOptions`).
///
/// `auto_checkpoint = Some(false)` is the key knob for large bulk loads: with
/// automatic checkpoints on, every committed write batch can trigger checkpoint
/// work whose cost grows with the database size. With it off, writes only append
/// to the WAL during the load and a single checkpoint is taken at
/// [`Database::close`], keeping per-batch cost flat.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatabaseOptions {
    /// Buffer-pool size in bytes.
    pub buffer_pool_size: Option<u64>,
    /// Enable on-disk compression.
    pub enable_compression: Option<bool>,
    /// Open read-only.
    pub read_only: Option<bool>,
    /// Maximum database size in bytes.
    pub max_db_size: Option<u64>,
    /// Enable automatic checkpoints during operation (engine default: `true`).
    pub auto_checkpoint: Option<bool>,
    /// WAL-size threshold (bytes) that triggers an automatic checkpoint.
    pub checkpoint_threshold: Option<i64>,
}

impl DatabaseOptions {
    /// Build the `lbug::SystemConfig`, applying only the fields the caller set
    /// and leaving every other knob at the engine default.
    fn to_system_config(&self) -> lbug::SystemConfig {
        let mut config = lbug::SystemConfig::default();
        if let Some(size) = self.buffer_pool_size {
            config = config.buffer_pool_size(size);
        }
        if let Some(enabled) = self.enable_compression {
            config = config.enable_compression(enabled);
        }
        if let Some(read_only) = self.read_only {
            config = config.read_only(read_only);
        }
        if let Some(size) = self.max_db_size {
            config = config.max_db_size(size);
        }
        if let Some(enabled) = self.auto_checkpoint {
            config = config.auto_checkpoint(enabled);
        }
        if let Some(threshold) = self.checkpoint_threshold {
            config = config.checkpoint_threshold(threshold);
        }
        config
    }
}

/// A thin handle over a LadybugDB instance.
///
/// Open an ephemeral database with [`Database::in_memory`] or an on-disk one
/// with [`Database::open`]. [`Database::close`] is idempotent.
#[derive(Debug)]
pub struct Database {
    inner: Option<lbug::Database>,
}

impl Database {
    /// Open an ephemeral in-memory database with the engine defaults.
    ///
    /// Mirrors `new Database()` (the TypeScript default path of `':memory:'`).
    pub fn in_memory() -> Result<Self> {
        Self::in_memory_with_options(DatabaseOptions::default())
    }

    /// Open an ephemeral in-memory database with the given tuning options.
    pub fn in_memory_with_options(options: DatabaseOptions) -> Result<Self> {
        let inner = lbug::Database::in_memory(options.to_system_config())?;
        Ok(Self { inner: Some(inner) })
    }

    /// Open (creating if absent) an on-disk database at `path` with the engine
    /// defaults.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_options(path, DatabaseOptions::default())
    }

    /// Open (creating if absent) an on-disk database at `path` with the given
    /// tuning options.
    pub fn open_with_options(path: impl AsRef<Path>, options: DatabaseOptions) -> Result<Self> {
        let inner = lbug::Database::new(path, options.to_system_config())?;
        Ok(Self { inner: Some(inner) })
    }

    /// Return a fresh [`Connection`] bound to this database.
    ///
    /// Each call yields a new connection; use one connection per logical unit of
    /// work. Errors with [`Error::DatabaseClosed`] if [`Database::close`] ran.
    pub fn connect(&self) -> Result<Connection<'_>> {
        let db = self.inner.as_ref().ok_or(Error::DatabaseClosed)?;
        let conn = lbug::Connection::new(db)?;
        Ok(Connection { inner: Some(conn) })
    }

    /// Close the database and release native resources. Idempotent.
    ///
    /// Dropping the underlying engine handle triggers a final checkpoint, so an
    /// on-disk database opened with `auto_checkpoint = Some(false)` is durable
    /// and self-contained (no `.wal` sidecar) after this returns.
    pub fn close(&mut self) {
        self.inner = None;
    }

    /// Whether the database is still open (i.e. [`Database::close`] has not run).
    pub fn is_open(&self) -> bool {
        self.inner.is_some()
    }
}

/// Executes Cypher against an open [`Database`].
///
/// Obtain one via [`Database::connect`]. A connection is not safe for concurrent
/// in-flight queries; use one connection per logical unit of work.
pub struct Connection<'db> {
    inner: Option<lbug::Connection<'db>>,
}

impl std::fmt::Debug for Connection<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("open", &self.inner.is_some())
            .finish()
    }
}

impl Connection<'_> {
    /// Execute a Cypher statement with no parameters and collect all rows.
    ///
    /// Mirrors `connection.run(cypher)` (the no-params branch, which runs the
    /// statement directly).
    pub fn run(&self, cypher: &str) -> Result<Vec<Row>> {
        self.execute_internal(cypher, None)
    }

    /// Execute a Cypher statement with **bound** named parameters and collect
    /// all rows.
    ///
    /// Mirrors `connection.run(cypher, params)`: the statement is prepared and
    /// the values are bound by the driver — never interpolated into the query
    /// text. Reference named parameters in Cypher as `$name`.
    pub fn run_params(&self, cypher: &str, params: Vec<(&str, Value)>) -> Result<Vec<Row>> {
        self.execute_internal(cypher, Some(params))
    }

    fn execute_internal(
        &self,
        cypher: &str,
        params: Option<Vec<(&str, Value)>>,
    ) -> Result<Vec<Row>> {
        let conn = self.inner.as_ref().ok_or(Error::ConnectionClosed)?;
        let result = match params {
            None => conn.query(cypher)?,
            Some(params) => {
                let mut prepared = conn.prepare(cypher)?;
                conn.execute(&mut prepared, params)?
            }
        };

        let column_names = result.get_column_names();
        let mut rows = Vec::with_capacity(result.get_num_tuples() as usize);
        for tuple in result {
            let mut row = Row::with_capacity(column_names.len());
            for (name, value) in column_names.iter().zip(tuple) {
                row.insert(name.clone(), value);
            }
            rows.push(row);
        }
        Ok(rows)
    }

    /// Install and load a LadybugDB extension, issuing the
    /// `INSTALL <name>` + `LOAD EXTENSION <name>` sequence so callers don't
    /// repeat it.
    pub fn load_extension(&self, name: &str) -> Result<()> {
        self.run(&format!("INSTALL {name}"))?;
        self.run(&format!("LOAD EXTENSION {name}"))?;
        Ok(())
    }

    /// Close the connection and release native resources. Idempotent.
    pub fn close(&mut self) {
        self.inner = None;
    }

    /// Whether the connection is still open (i.e. [`Connection::close`] has not
    /// run).
    pub fn is_open(&self) -> bool {
        self.inner.is_some()
    }
}
