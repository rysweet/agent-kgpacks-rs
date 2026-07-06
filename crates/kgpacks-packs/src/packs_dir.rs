//! Resolution of the directory that holds installed knowledge packs.
//!
//! This is the **single** source of truth for where packs live, shared by the
//! CLI (`kgpacks-cli`) and the MCP server (`kgpacks-mcp`) so a pack installed by
//! one is found by the other. It ports the WS4 behaviour of the reference
//! `agent-kgpacks-ts` port.
//!
//! Resolution precedence (highest first):
//!
//! 1. an **explicit** override — the CLI's `--packs-dir <dir>` flag or a
//!    programmatic injection (they collapse to the same argument at this layer);
//! 2. the [`PACKS_DIR_ENV`] (`KGPACKS_PACKS_DIR`) environment variable;
//! 3. the **XDG default**: `$XDG_DATA_HOME/kgpacks` when `XDG_DATA_HOME` is set
//!    (and non-empty), else `~/.local/share/kgpacks`.
//!
//! Empty or whitespace-only overrides (both the explicit argument and the env
//! var) are treated as **unset**, so a blank value falls through to the next
//! level rather than resolving to an empty path.

use std::path::{Path, PathBuf};

/// Environment variable naming the directory that holds installed packs.
pub const PACKS_DIR_ENV: &str = "KGPACKS_PACKS_DIR";

/// Directory name, under the XDG data dir, that holds this project's packs.
const APP_DIR: &str = "kgpacks";

/// Treat empty / whitespace-only values as unset, mirroring the reference
/// port's blank-override handling. A value that is entirely whitespace resolves
/// to `None`; any value with non-whitespace content is returned unchanged.
fn non_blank(value: Option<&str>) -> Option<&str> {
    match value {
        Some(v) if !v.trim().is_empty() => Some(v),
        _ => None,
    }
}

/// Compute the XDG default packs directory from the given `XDG_DATA_HOME` and
/// `HOME` values (both taken as raw environment strings).
///
/// * `$XDG_DATA_HOME/kgpacks` when `XDG_DATA_HOME` is set and non-empty;
/// * else `$HOME/.local/share/kgpacks`;
/// * else — only if `HOME` is also unset, which is pathological — a
///   working-directory-relative `.local/share/kgpacks` so resolution never
///   panics.
fn default_packs_dir_from(xdg_data_home: Option<&str>, home: Option<&str>) -> PathBuf {
    if let Some(xdg) = non_blank(xdg_data_home) {
        return PathBuf::from(xdg).join(APP_DIR);
    }
    let base = match non_blank(home) {
        Some(home) => PathBuf::from(home),
        None => PathBuf::new(),
    };
    base.join(".local").join("share").join(APP_DIR)
}

/// Pure resolution: given every input explicitly, compute the packs directory.
///
/// This takes no process state, so the full precedence matrix (explicit > env >
/// XDG default, plus XDG/`HOME` fallback and blank-override handling) is
/// deterministically unit-testable.
fn resolve_packs_dir_from(
    explicit: Option<&str>,
    env_override: Option<&str>,
    xdg_data_home: Option<&str>,
    home: Option<&str>,
) -> PathBuf {
    if let Some(dir) = non_blank(explicit) {
        return PathBuf::from(dir);
    }
    if let Some(dir) = non_blank(env_override) {
        return PathBuf::from(dir);
    }
    default_packs_dir_from(xdg_data_home, home)
}

/// The XDG default packs directory, read from the process environment.
///
/// `$XDG_DATA_HOME/kgpacks` when `XDG_DATA_HOME` is set (non-empty), else
/// `~/.local/share/kgpacks`.
pub fn default_packs_dir() -> PathBuf {
    default_packs_dir_from(
        std::env::var("XDG_DATA_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

/// Resolve the packs directory from an optional explicit override, reading the
/// [`PACKS_DIR_ENV`] env var and the XDG environment as needed.
///
/// `explicit` is the CLI `--packs-dir` flag (or a programmatic injection). See
/// the [module docs](self) for the full precedence. Blank overrides — both
/// `explicit` and the env var — are treated as unset.
///
/// When `explicit` is a non-blank directory the process environment is not read
/// at all: resolution short-circuits to the override. This keeps the common
/// `--packs-dir` path free of any environment access (and free of env data
/// races when a caller mutates the environment concurrently).
pub fn resolve_packs_dir(explicit: Option<&str>) -> PathBuf {
    if non_blank(explicit).is_some() {
        return resolve_packs_dir_from(explicit, None, None, None);
    }
    resolve_packs_dir_from(
        None,
        std::env::var(PACKS_DIR_ENV).ok().as_deref(),
        std::env::var("XDG_DATA_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

/// Best-effort creation of the packs directory `dir` so the default location
/// exists on first use.
///
/// Failures (e.g. a permission error, or a parent that cannot be created) are
/// swallowed: this is a convenience so an installed pack lands in a directory
/// that exists, never a hard precondition for read commands, which surface
/// their own "pack not found" errors. Returns `dir` for call-site chaining.
pub fn ensure_packs_dir(dir: &Path) -> &Path {
    let _ = std::fs::create_dir_all(dir);
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_override_wins_over_everything() {
        let dir = resolve_packs_dir_from(
            Some("/flag/packs"),
            Some("/env/packs"),
            Some("/xdg"),
            Some("/home/user"),
        );
        assert_eq!(dir, PathBuf::from("/flag/packs"));
    }

    #[test]
    fn env_override_wins_when_no_explicit() {
        let dir =
            resolve_packs_dir_from(None, Some("/env/packs"), Some("/xdg"), Some("/home/user"));
        assert_eq!(dir, PathBuf::from("/env/packs"));
    }

    #[test]
    fn xdg_data_home_is_the_default_when_no_overrides() {
        let dir = resolve_packs_dir_from(None, None, Some("/xdg"), Some("/home/user"));
        assert_eq!(dir, PathBuf::from("/xdg/kgpacks"));
    }

    #[test]
    fn falls_back_to_home_local_share_when_xdg_unset() {
        let dir = resolve_packs_dir_from(None, None, None, Some("/home/user"));
        assert_eq!(dir, PathBuf::from("/home/user/.local/share/kgpacks"));
    }

    #[test]
    fn blank_explicit_falls_through_to_env() {
        let dir = resolve_packs_dir_from(
            Some("   "),
            Some("/env/packs"),
            Some("/xdg"),
            Some("/home/user"),
        );
        assert_eq!(dir, PathBuf::from("/env/packs"));
    }

    #[test]
    fn blank_env_falls_through_to_xdg_default() {
        let dir = resolve_packs_dir_from(None, Some("  \t "), Some("/xdg"), Some("/home/user"));
        assert_eq!(dir, PathBuf::from("/xdg/kgpacks"));
    }

    #[test]
    fn blank_xdg_data_home_falls_back_to_home() {
        let dir = resolve_packs_dir_from(None, None, Some(""), Some("/home/user"));
        assert_eq!(dir, PathBuf::from("/home/user/.local/share/kgpacks"));
    }

    #[test]
    fn empty_explicit_string_is_treated_as_unset() {
        let dir = resolve_packs_dir_from(Some(""), None, Some("/xdg"), Some("/home/user"));
        assert_eq!(dir, PathBuf::from("/xdg/kgpacks"));
    }

    #[test]
    fn last_resort_when_both_xdg_and_home_are_unset() {
        // Pathological environment (no HOME, no XDG_DATA_HOME): resolution must
        // still yield a path rather than panic.
        let dir = default_packs_dir_from(None, None);
        assert_eq!(dir, PathBuf::from(".local/share/kgpacks"));
    }

    #[test]
    fn ensure_packs_dir_creates_the_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("nested").join("kgpacks");
        assert!(!target.exists());
        let returned = ensure_packs_dir(&target);
        assert_eq!(returned, target.as_path());
        assert!(target.is_dir());
    }
}
