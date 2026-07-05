// Reference oracle for the read-path `pack` command group — a standalone,
// dependency-free reimplementation of the canonical `agent-kgpacks-ts` behavior,
// used by qa/pack-parity/verify.sh to cross-check the Rust
// `kgpacks pack {list,info,validate}` output against the TypeScript reference
// semantics WITHOUT a TS build step.
//
// It mirrors:
//   * packages/packs/src/manifest.ts   `validateManifest` / `loadManifestFromDir`
//                                       (name+version+description+section gates,
//                                        prototype-pollution strip, provenance
//                                        deepSanitize, and — crucially for
//                                        `pack info` — the "rebuild preserving
//                                        every other key" behavior)
//   * packages/packs/src/registry.ts   `listPacks` / `packInfo`
//   * packages/cli/src/commands/pack.ts `pack list` / `pack info` /
//                                        `pack validate` output shapes
//   * packages/cli/src/io.ts           `printJson` = JSON.stringify(v, null, 2)
//
// Usage:
//   node reference_oracle.mjs pack-list     <packsDir>          # TS-parity `pack list`
//   node reference_oracle.mjs pack-info     <packsDir> <name>   # TS-parity `pack info`
//   node reference_oracle.mjs pack-validate <packsDir> <name>   # TS-parity `pack validate`
//   node reference_oracle.mjs canon         <jsonFile>          # canonical (sorted-key) JSON

import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

// Ports packages/packs/src/manifest.ts.
const MANIFEST_FILENAME = 'manifest.json';
const PACK_NAME_RE = /^[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}$/;
// Verbatim from packages/packs/src/versioning.ts.
const SEMVER_RE =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$/;
const DANGEROUS_KEYS = new Set(['__proto__', 'constructor', 'prototype']);

function isPlainObject(v) {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

// Ports `validateProvenance`.
const PROVENANCE_STRING_FIELDS = {
  corpus: ['name', 'commit', 'date'],
  embedding: ['model'],
  build: ['date', 'tool_version'],
};

function validateProvenance(value) {
  if (!isPlainObject(value)) throw new Error('provenance must be an object');
  for (const [section, fields] of Object.entries(PROVENANCE_STRING_FIELDS)) {
    const sec = value[section];
    if (sec == null) continue;
    if (!isPlainObject(sec)) throw new Error(`provenance.${section} must be an object`);
    for (const field of fields) {
      const fieldValue = sec[field];
      if (fieldValue == null) continue;
      if (typeof fieldValue !== 'string') {
        throw new Error(`provenance.${section}.${field} must be a string`);
      }
    }
  }
  const embedding = value.embedding;
  if (isPlainObject(embedding) && embedding.dimensions != null) {
    const d = embedding.dimensions;
    if (typeof d !== 'number' || !Number.isFinite(d) || d < 0) {
      throw new Error('provenance.embedding.dimensions must be a non-negative finite number');
    }
  }
}

// Ports `validateGraphStats`.
function validateGraphStats(value) {
  if (!isPlainObject(value)) throw new Error('graph_stats must be an object');
  for (const [key, n] of Object.entries(value)) {
    if (typeof n !== 'number' || !Number.isFinite(n) || n < 0) {
      throw new Error(`graph_stats.${key} must be a non-negative finite number`);
    }
  }
}

// Ports `validateEvalScores`.
function validateEvalScores(value) {
  if (!isPlainObject(value)) throw new Error('eval_scores must be an object');
  for (const [key, n] of Object.entries(value)) {
    if (typeof n !== 'number' || !Number.isFinite(n)) {
      throw new Error(`eval_scores.${key} must be a finite number`);
    }
  }
}

// Ports `deepSanitize`.
function deepSanitize(value) {
  if (Array.isArray(value)) return value.map(deepSanitize);
  if (isPlainObject(value)) {
    const out = {};
    for (const key of Object.keys(value)) {
      if (DANGEROUS_KEYS.has(key)) continue;
      out[key] = deepSanitize(value[key]);
    }
    return out;
  }
  return value;
}

// Ports `validateManifest`: throws on any schema violation and returns the
// manifest rebuilt WITHOUT dangerous keys but preserving every other key
// (provenance sanitized recursively). This is what `pack info` prints.
function validateManifest(value) {
  if (!isPlainObject(value)) throw new Error('manifest must be a JSON object');
  const { name, version } = value;
  if (typeof name !== 'string' || !PACK_NAME_RE.test(name)) {
    throw new Error('invalid pack name');
  }
  if (typeof version !== 'string' || !SEMVER_RE.test(version)) {
    throw new Error('invalid version');
  }
  if ('description' in value && typeof value.description !== 'string') {
    throw new Error('description must be a string');
  }
  if (value.graph_stats != null) validateGraphStats(value.graph_stats);
  if (value.eval_scores != null) validateEvalScores(value.eval_scores);
  if (value.provenance != null) validateProvenance(value.provenance);

  const result = {};
  for (const key of Object.keys(value)) {
    if (DANGEROUS_KEYS.has(key)) continue;
    result[key] = key === 'provenance' ? deepSanitize(value[key]) : value[key];
  }
  return result;
}

function loadManifestFromDir(packDir) {
  const raw = readFileSync(join(packDir, MANIFEST_FILENAME), 'utf8');
  return validateManifest(JSON.parse(raw));
}

// Ports packages/packs/src/registry.ts `listPacks`.
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
    packs.push({ name: manifest.name, version: manifest.version, path, manifest });
  }
  return packs;
}

// Ports `pack list` (packages/cli/src/commands/pack.ts). The collation is pinned
// to 'en-US' — the ICU root order the Rust `localecompare_pack_name` reproduces —
// so the check is deterministic and independent of the CI host locale (pack names
// are ASCII `[a-zA-Z0-9_-]`, so no locale-specific casing can arise).
function packList(packsDir) {
  return listPacks(packsDir)
    .map((p) => ({
      name: p.name,
      version: p.version,
      description: typeof p.manifest.description === 'string' ? p.manifest.description : '',
    }))
    .sort((a, b) => a.name.localeCompare(b.name, 'en-US'));
}

// Ports `packInfo` (registry.ts) + `pack info` (pack.ts): validate name, require
// manifest.json, then print the full validated manifest.
function packInfo(packsDir, name) {
  if (typeof name !== 'string' || !PACK_NAME_RE.test(name)) {
    throw new Error(`pack not found: ${name}`);
  }
  const dir = join(packsDir, name);
  if (!existsSync(join(dir, MANIFEST_FILENAME))) {
    throw new Error(`pack not found: ${name}`);
  }
  return loadManifestFromDir(dir);
}

// Ports `pack validate` (pack.ts): resolve an existing pack dir with a valid
// name + manifest, load+validate, then print `{ valid, name, version }`.
function packValidate(packsDir, name) {
  if (typeof name !== 'string' || !PACK_NAME_RE.test(name)) {
    throw new Error(`pack not found: ${name}`);
  }
  const dir = join(packsDir, name);
  if (!existsSync(join(dir, MANIFEST_FILENAME))) {
    throw new Error(`pack not found: ${name}`);
  }
  const manifest = loadManifestFromDir(dir);
  return { valid: true, name: manifest.name, version: manifest.version };
}

// Recursively sort object keys so two structurally-equal JSON documents (which
// may differ only in key order — serde_json's alphabetical output vs the
// reference's insertion order) canonicalize to the same string. JSON.parse also
// normalizes numeric spelling (e.g. `10.0` -> `10`), so the one intentional
// numeric-format difference between the ports is neutralized here too.
function sortKeys(value) {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (value && typeof value === 'object') {
    const out = {};
    for (const key of Object.keys(value).sort()) out[key] = sortKeys(value[key]);
    return out;
  }
  return value;
}

function emit(value) {
  process.stdout.write(JSON.stringify(value, null, 2) + '\n');
}

function main() {
  const [mode, arg, arg2] = process.argv.slice(2);
  switch (mode) {
    case 'pack-list':
      if (!arg) throw new Error('usage: reference_oracle.mjs pack-list <packsDir>');
      emit(packList(arg));
      return;
    case 'pack-info':
      if (!arg || !arg2) throw new Error('usage: reference_oracle.mjs pack-info <packsDir> <name>');
      emit(packInfo(arg, arg2));
      return;
    case 'pack-validate':
      if (!arg || !arg2) {
        throw new Error('usage: reference_oracle.mjs pack-validate <packsDir> <name>');
      }
      emit(packValidate(arg, arg2));
      return;
    case 'canon': {
      if (!arg) throw new Error('usage: reference_oracle.mjs canon <jsonFile>');
      const parsed = JSON.parse(readFileSync(arg, 'utf8'));
      process.stdout.write(JSON.stringify(sortKeys(parsed)) + '\n');
      return;
    }
    default:
      throw new Error(
        `unknown mode: ${mode ?? '(none)'} (expected pack-list | pack-info | pack-validate | canon)`,
      );
  }
}

main();
