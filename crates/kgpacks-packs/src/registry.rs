//! Installed-pack registry queries over an install root.
//!
//! Ports the read-path of `packages/packs/src/registry.ts`: enumerate the packs
//! installed under a directory. `list_packs` mirrors the reference `listPacks`
//! byte-for-byte in behavior — it reads the immediate subdirectories of the
//! install root, loads each one's `manifest.json`, and skips any entry that is
//! not a directory or lacks a valid manifest. A missing/unreadable install root
//! yields an empty list rather than an error, exactly like the reference (so a
//! caller such as `status` never fails on a fresh machine with no packs dir).
//!
//! The tarball installer, `pack_info`/`remove_pack` (the write-path registry
//! surfaces) remain follow-ups; only the read-path `list_packs` needed by the
//! CLI `status` command is ported here.

use std::path::{Path, PathBuf};

use crate::manifest::{load_manifest_from_dir, PackManifest};

/// A pack discovered under an install root by [`list_packs`].
///
/// Mirrors the reference `InstalledPack`: the pack's `name` and `version`
/// (hoisted from its manifest for convenience), its on-disk `path`, and the
/// full parsed `manifest`.
#[derive(Debug, Clone, PartialEq)]
pub struct InstalledPack {
    /// Pack name, from the manifest.
    pub name: String,
    /// Pack version, from the manifest.
    pub version: String,
    /// Absolute-or-relative path to the pack directory (`<install_root>/<dir>`).
    pub path: PathBuf,
    /// The pack's parsed manifest.
    pub manifest: PackManifest,
}

/// List the packs installed directly under `install_root`.
///
/// Ports `listPacks` from `packages/packs/src/registry.ts`:
///
/// * A missing or unreadable `install_root` returns an empty list (never an
///   error).
/// * Only immediate **directory** entries are considered (regular files and
///   symlinks are skipped, matching the reference's `dirent.isDirectory()`).
/// * A directory without a valid `manifest.json` is skipped, not an error.
///
/// The result preserves directory-iteration order (filesystem-defined); the
/// caller sorts when a stable order is required (as the `status` command does).
pub fn list_packs(install_root: impl AsRef<Path>) -> Vec<InstalledPack> {
    let entries = match std::fs::read_dir(install_root.as_ref()) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let mut packs = Vec::new();
    // `entries.flatten()` skips any entry that fails to read: like the reference
    // — whose `readdirSync(..., { withFileTypes: true })` classifies each entry
    // up front and simply skips anything that isn't a valid pack — an entry that
    // cannot be classified or read is treated as "not a pack" and skipped, never
    // aborting the scan or masking the packs that were found. A wholly
    // missing/unreadable root is the only empty-list case (handled above).
    for entry in entries.flatten() {
        // Only directories are pack candidates. `file_type` does not follow
        // symlinks, so a symlink-to-directory is skipped just like the
        // reference's `dirent.isDirectory()`; an errored file-type read is
        // likewise treated as "not a directory" and skipped.
        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }

        let path = entry.path();
        // Directories without a valid manifest are skipped, not fatal.
        let manifest = match load_manifest_from_dir(&path) {
            Ok(manifest) => manifest,
            Err(_) => continue,
        };

        packs.push(InstalledPack {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            path,
            manifest,
        });
    }
    packs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use tempfile::tempdir;

    fn write_pack(root: &Path, dir: &str, manifest_json: &str) {
        let pack_dir = root.join(dir);
        fs::create_dir_all(&pack_dir).expect("create pack dir");
        fs::write(pack_dir.join("manifest.json"), manifest_json).expect("write manifest");
    }

    #[test]
    fn missing_root_yields_empty_list() {
        let tmp = tempdir().expect("tempdir");
        let missing = tmp.path().join("does-not-exist");
        assert!(list_packs(&missing).is_empty());
    }

    #[test]
    fn empty_root_yields_empty_list() {
        let tmp = tempdir().expect("tempdir");
        assert!(list_packs(tmp.path()).is_empty());
    }

    #[test]
    fn lists_valid_packs_with_name_and_version() {
        let tmp = tempdir().expect("tempdir");
        write_pack(tmp.path(), "alpha", r#"{"name":"alpha","version":"1.2.3"}"#);
        write_pack(tmp.path(), "beta", r#"{"name":"beta","version":"0.1.0"}"#);

        let mut packs = list_packs(tmp.path());
        packs.sort_by(|a, b| a.name.cmp(&b.name));

        assert_eq!(packs.len(), 2);
        assert_eq!(packs[0].name, "alpha");
        assert_eq!(packs[0].version, "1.2.3");
        assert_eq!(packs[0].path, tmp.path().join("alpha"));
        assert_eq!(packs[1].name, "beta");
        assert_eq!(packs[1].version, "0.1.0");
    }

    #[test]
    fn skips_non_directories() {
        let tmp = tempdir().expect("tempdir");
        write_pack(tmp.path(), "real", r#"{"name":"real","version":"1.0.0"}"#);
        // A stray regular file at the install root must be ignored.
        fs::write(tmp.path().join("stray.txt"), "not a pack").expect("write stray");

        let packs = list_packs(tmp.path());
        assert_eq!(packs.len(), 1);
        assert_eq!(packs[0].name, "real");
    }

    #[test]
    fn skips_directories_without_a_manifest() {
        let tmp = tempdir().expect("tempdir");
        fs::create_dir_all(tmp.path().join("no-manifest")).expect("mkdir");
        write_pack(tmp.path(), "good", r#"{"name":"good","version":"2.0.0"}"#);

        let packs = list_packs(tmp.path());
        assert_eq!(packs.len(), 1);
        assert_eq!(packs[0].name, "good");
    }

    #[test]
    fn skips_directories_with_an_invalid_manifest() {
        let tmp = tempdir().expect("tempdir");
        // Present but schema-invalid (missing required `version`).
        write_pack(tmp.path(), "broken", r#"{"name":"broken"}"#);
        write_pack(tmp.path(), "ok", r#"{"name":"ok","version":"1.0.0"}"#);

        let packs = list_packs(tmp.path());
        assert_eq!(packs.len(), 1);
        assert_eq!(packs[0].name, "ok");
    }
}
