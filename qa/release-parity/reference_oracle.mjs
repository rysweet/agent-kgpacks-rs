// Reference oracle for the `pack release-plan` command — a standalone,
// dependency-free reimplementation of the canonical `agent-kgpacks-ts` release
// tooling (`scripts/release-pack.mjs`) + `packVersionFromReleaseTag`
// (`packages/packs/src/versioning.ts`), used by qa/release-parity/verify.sh to
// cross-check the Rust `kgpacks pack release-plan` output against the TypeScript
// reference semantics.
//
// It mirrors, with no TS build step required:
//   * versioning.ts   `packVersionFromReleaseTag` (unpadded dated-tag version)
//   * release-pack.mjs `deriveVersionFromTag` / `buildProvenance` / `publishTo`
//   * manifest.ts      the name/version/provenance validate gate
//
// Determinism note: `buildProvenance` defaults a missing `build.date` to
// `new Date().toISOString()`. The parity fixtures always carry `build.date`, so
// the oracle and the Rust CLI never diverge on a live timestamp.
//
// Usage:
//   node reference_oracle.mjs release-plan <packsDir> <pack> <tag> \
//        [model] [corpusCommit] [corpusDate]      # emit TS-parity plan JSON
//   node reference_oracle.mjs canon <jsonFile>    # emit canonical (sorted-key) JSON

import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const PACK_NAME_RE = /^[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}$/;
const MANIFEST_FILENAME = 'manifest.json';
const LATEST_POINTER_TAG = 'packs';
const SEMVER_RE =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-.]+)?(?:\+[0-9A-Za-z-.]+)?$/;

function isPlainObject(v) {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

const PROVENANCE_STRING_FIELDS = {
  corpus: ['name', 'commit', 'date'],
  embedding: ['model'],
  build: ['date', 'tool_version'],
};

// Ports packages/packs/src/manifest.ts `validateProvenance`.
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

function loadManifest(packDir) {
  const raw = readFileSync(join(packDir, MANIFEST_FILENAME), 'utf8');
  const value = JSON.parse(raw);
  if (!isPlainObject(value)) throw new Error('manifest must be an object');
  const { name, version } = value;
  if (typeof name !== 'string' || !PACK_NAME_RE.test(name)) throw new Error('invalid manifest name');
  if (typeof version !== 'string' || !SEMVER_RE.test(version)) {
    throw new Error('invalid manifest version');
  }
  if (value.provenance != null) validateProvenance(value.provenance);
  return value;
}

// Ports versioning.ts `packVersionFromReleaseTag` (returned as null on failure,
// matching release-pack.mjs `deriveVersionFromTag`).
const RELEASE_TAG_RE = /-(\d{4})\.(\d{2})(?:\.(\d+))?$/;
function deriveVersionFromTag(tag) {
  const m = RELEASE_TAG_RE.exec(typeof tag === 'string' ? tag : '');
  if (!m) return null;
  const month = Number(m[2]);
  if (month < 1 || month > 12) return null;
  const version = `${Number(m[1])}.${month}.${m[3] !== undefined ? Number(m[3]) : 0}`;
  return SEMVER_RE.test(version) ? version : null;
}

function resolveModel(manifest, model) {
  const resolved = model ?? manifest.model ?? manifest.synthesis_model;
  return typeof resolved === 'string' ? resolved : undefined;
}

// Ports release-pack.mjs `buildProvenance`.
function buildProvenance(manifest, { model, corpusCommit, corpusDate, nowIso }) {
  const base = isPlainObject(manifest.provenance) ? manifest.provenance : {};
  const corpus = { ...(isPlainObject(base.corpus) ? base.corpus : {}) };
  if (corpusCommit) corpus.commit = corpusCommit;
  if (corpusDate) corpus.date = corpusDate;
  const resolvedModel = resolveModel(manifest, model);
  const embedding = { ...(isPlainObject(base.embedding) ? base.embedding : {}) };
  if (resolvedModel && !embedding.model) embedding.model = resolvedModel;
  const build = { ...(isPlainObject(base.build) ? base.build : {}) };
  if (!build.date) build.date = nowIso;
  const provenance = {};
  if (Object.keys(corpus).length) provenance.corpus = corpus;
  if (Object.keys(embedding).length) provenance.embedding = embedding;
  if (Object.keys(build).length) provenance.build = build;
  return Object.keys(provenance).length ? provenance : null;
}

function publishTargets(tag) {
  return tag === LATEST_POINTER_TAG ? [tag] : [tag, LATEST_POINTER_TAG];
}

function releasePlan(packsDir, pack, tag, model, corpusCommit, corpusDate) {
  const manifest = loadManifest(join(packsDir, pack));
  const nowIso = 'UNUSED-NOW'; // fixtures always carry build.date
  return {
    name: manifest.name,
    tag,
    version: deriveVersionFromTag(tag) ?? String(manifest.version),
    model: resolveModel(manifest, model) ?? null,
    provenance: buildProvenance(manifest, { model, corpusCommit, corpusDate, nowIso }),
    publishTargets: publishTargets(tag),
    indexFilename: `${pack}.pack-release.json`,
  };
}

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
  const argv = process.argv.slice(2);
  const mode = argv[0];
  if (mode === 'release-plan') {
    const [, packsDir, pack, tag, model, corpusCommit, corpusDate] = argv;
    if (!packsDir || !pack || !tag) {
      throw new Error(
        'usage: reference_oracle.mjs release-plan <packsDir> <pack> <tag> [model] [commit] [date]',
      );
    }
    const plan = releasePlan(
      packsDir,
      pack,
      tag,
      model || undefined,
      corpusCommit || undefined,
      corpusDate || undefined,
    );
    process.stdout.write(JSON.stringify(plan, null, 2) + '\n');
    return;
  }
  if (mode === 'canon') {
    const arg = argv[1];
    if (!arg) throw new Error('usage: reference_oracle.mjs canon <jsonFile>');
    const parsed = JSON.parse(readFileSync(arg, 'utf8'));
    process.stdout.write(JSON.stringify(sortKeys(parsed)) + '\n');
    return;
  }
  throw new Error(`unknown mode: ${mode ?? '(none)'} (expected 'release-plan' or 'canon')`);
}

main();
