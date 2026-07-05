// Reference oracle for the `status` command — a standalone, dependency-free
// reimplementation of the canonical `agent-kgpacks-ts` behavior, used by
// qa/status-parity/verify.sh to cross-check the Rust `kgpacks status` output
// against the TypeScript reference semantics.
//
// It mirrors, with no TS build step required:
//   * packages/packs/src/registry.ts   `listPacks`
//   * packages/packs/src/manifest.ts   `loadManifestFromDir` (name+version gate)
//   * packages/cli/src/commands/status.ts  the `{ packsDir, count, packs }` shape
//   * packages/cli/src/io.ts           `printJson` = JSON.stringify(v, null, 2)+'\n'
//
// Backend note: the reference checks `dbPresent` against its own store file
// (`DB_FILENAME = 'pack.db'`), whereas the Rust port uses LadybugDB
// (`graph.lbug`). That store filename is the one intentional backend difference;
// the parity harness materializes both names for a "present" pack so the
// STRUCTURAL parity of the status payload can be asserted directly.
//
// Usage:
//   node reference_oracle.mjs status <packsDir>   # emit TS-parity status JSON
//   node reference_oracle.mjs canon  <jsonFile>   # emit canonical (sorted-key) JSON

import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

// Ports packages/packs/src/manifest.ts PACK_NAME_RE and the CLI DB_FILENAME.
const PACK_NAME_RE = /^[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}$/;
const MANIFEST_FILENAME = 'manifest.json';
const DB_FILENAME = 'pack.db'; // reference store filename (RS port uses graph.lbug)

// Minimal SemVer 2.0 recognizer, matching the reference `isValidSemver` gate
// closely enough for pack manifests (core + optional prerelease/build).
const SEMVER_RE =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-.]+)?(?:\+[0-9A-Za-z-.]+)?$/;

// Ports loadManifestFromDir + the validate gate. NOTE: this mirrors the *current
// Rust* manifest gate that `list_packs` relies on — a valid `name` (PACK_NAME_RE)
// and `version` (SemVer). The full reference `validateManifest` additionally
// validates optional `provenance`/`graph_stats`/`eval_scores` blocks; the Rust
// port does not yet validate `provenance` (tracked as a separate manifest-parity
// follow-up), so this oracle deliberately matches the Rust gate the `status`
// read-path actually uses. The status fixtures never include those optional
// blocks, so both sides agree on which directories are valid packs. Throws on any
// violation so the caller skips the directory, exactly like the reference.
function loadManifestFromDir(packDir) {
  const raw = readFileSync(join(packDir, MANIFEST_FILENAME), 'utf8');
  const value = JSON.parse(raw);
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('manifest must be an object');
  }
  const { name, version } = value;
  if (typeof name !== 'string' || !PACK_NAME_RE.test(name)) {
    throw new Error('invalid manifest name');
  }
  if (typeof version !== 'string' || !SEMVER_RE.test(version)) {
    throw new Error('invalid manifest version');
  }
  return { name, version };
}

// Ports packages/packs/src/registry.ts listPacks.
function listPacks(installRoot) {
  let dirents;
  try {
    dirents = readdirSync(installRoot, { withFileTypes: true });
  } catch {
    return [];
  }
  const packs = [];
  for (const dirent of dirents) {
    if (!dirent.isDirectory()) continue;
    const path = join(installRoot, dirent.name);
    let manifest;
    try {
      manifest = loadManifestFromDir(path);
    } catch {
      continue; // directories without a valid manifest are skipped
    }
    packs.push({ name: manifest.name, version: manifest.version, path });
  }
  return packs;
}

// Ports packages/cli/src/commands/status.ts.
//
// The reference sorts with `name.localeCompare(b)` (no explicit locale). To keep
// the parity check deterministic and independent of the CI host locale, we pin
// the collation to 'en-US' — which, for the ASCII pack-name character set
// (PACK_NAME_RE), is the ICU root order the Rust `localecompare_pack_name`
// comparator reproduces. (Pack names are ASCII `[a-zA-Z0-9_-]`, so no
// locale-specific casing such as Turkish i/İ can arise in practice.)
function status(packsDir) {
  const packs = listPacks(packsDir)
    .map((p) => ({
      name: p.name,
      version: p.version,
      dbPresent: existsSync(join(p.path, DB_FILENAME)),
    }))
    .sort((a, b) => a.name.localeCompare(b.name, 'en-US'));
  return { packsDir, count: packs.length, packs };
}

// Recursively sort object keys so two structurally-equal JSON documents (which
// may differ only in key order, e.g. serde_json's alphabetical output vs the
// reference's insertion order) canonicalize to the same string.
function sortKeys(value) {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (value && typeof value === 'object') {
    const out = {};
    for (const key of Object.keys(value).sort()) out[key] = sortKeys(value[key]);
    return out;
  }
  return value;
}

function main() {
  const [mode, arg] = process.argv.slice(2);
  if (mode === 'status') {
    if (!arg) throw new Error('usage: reference_oracle.mjs status <packsDir>');
    process.stdout.write(JSON.stringify(status(arg), null, 2) + '\n');
    return;
  }
  if (mode === 'canon') {
    if (!arg) throw new Error('usage: reference_oracle.mjs canon <jsonFile>');
    const parsed = JSON.parse(readFileSync(arg, 'utf8'));
    process.stdout.write(JSON.stringify(sortKeys(parsed)) + '\n');
    return;
  }
  throw new Error(`unknown mode: ${mode ?? '(none)'} (expected 'status' or 'canon')`);
}

main();
