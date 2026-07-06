#!/usr/bin/env bash
# Cross-implementation `pack release-plan` parity check (qa-team /
# gadugi-agentic-test).
#
# Proves the Rust offline release planner (`kgpacks pack release-plan`, over
# `kgpacks-packs::plan_release` / `pack_version_from_release_tag` /
# `build_release_provenance` / `publish_targets`) computes byte-for-byte the same
# plan as the canonical `agent-kgpacks-ts` release tooling
# (`scripts/release-pack.mjs` + `packages/packs/src/versioning.ts`
# `packVersionFromReleaseTag`). This closes the WS3 half of agent-kgpacks-rs #18:
# versioned dated release tags (`cve-YYYY.MM[.N]` → unpadded SemVer core) with a
# stable `packs` latest-pointer, and manifest `provenance` mirrored into the
# `<name>.pack-release.json` index.
#
# Method (a true side-by-side falsifiable check, mirroring qa/status-parity):
#   1. Materialize ONE shared packs dir with a `cve` pack whose manifest carries
#      a full `provenance` block (incl. `build.date`, so no live timestamp is
#      injected on either side).
#   2. For each (tag, overrides) case, run the Rust `pack release-plan` and the
#      shared JS reference oracle over the SAME dir.
#   3. Canonicalize (recursively sort keys) and assert byte-for-byte equality.
#   4. Assert the concrete WS3 outcomes: the dated tag derives the UNPADDED
#      version, moves the `packs` latest-pointer, and mirrors the provenance;
#      the bare `packs` pointer carries no dated version (manifest fallback).
#
# Prints `RELEASE_PARITY_OK` and exits 0 on success; prints the diff and exits 1
# on any divergence.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"
ORACLE="$HERE/reference_oracle.mjs"

cd "$REPO_ROOT"

if ! command -v node >/dev/null 2>&1; then
  echo "RELEASE_PARITY_FAILED: node is required to run the TypeScript reference oracle"
  exit 1
fi
if [ ! -f "$ORACLE" ]; then
  echo "RELEASE_PARITY_FAILED: reference oracle not found at $ORACLE"
  exit 1
fi

WORK="$(mktemp -d)"
FIX="$WORK/packs"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$FIX/cve"

# A pack whose manifest carries a full provenance block (build.date present, so
# neither implementation injects a live timestamp).
cat > "$FIX/cve/manifest.json" <<'JSON'
{
  "name": "cve",
  "version": "0.1.0",
  "provenance": {
    "corpus": { "name": "cvelistV5", "commit": "abc123", "date": "2026-01-01" },
    "embedding": { "model": "Xenova/bge-base-en-v1.5", "dimensions": 768 },
    "build": { "date": "2026-01-02T00:00:00Z", "tool_version": "0.1.0" }
  }
}
JSON

# Prebuild once so the per-case `cargo run` invocations are fast and quiet.
cargo build -q -p kgpacks-cli --locked

run_case() {
  # run_case <label> <tag> [model] [corpus-commit] [corpus-date]
  local label="$1" tag="$2" model="${3:-}" commit="${4:-}" cdate="${5:-}"
  local rs="$WORK/$label.rs.json" ts="$WORK/$label.ts.json"

  local rs_args=(pack release-plan cve --packs-dir "$FIX" --tag "$tag")
  local ts_args=("$FIX" cve "$tag")
  if [ -n "$model" ]; then rs_args+=(--model "$model"); ts_args+=("$model"); else ts_args+=(""); fi
  if [ -n "$commit" ]; then rs_args+=(--corpus-commit "$commit"); ts_args+=("$commit"); else ts_args+=(""); fi
  if [ -n "$cdate" ]; then rs_args+=(--corpus-date "$cdate"); ts_args+=("$cdate"); fi

  cargo run -q -p kgpacks-cli --locked -- "${rs_args[@]}" > "$rs"
  node "$ORACLE" release-plan "${ts_args[@]}" > "$ts"

  local rs_canon ts_canon
  rs_canon="$(node "$ORACLE" canon "$rs")"
  ts_canon="$(node "$ORACLE" canon "$ts")"
  if [ "$rs_canon" != "$ts_canon" ]; then
    echo "RELEASE_PARITY_FAILED: Rust plan diverged from the agent-kgpacks-ts reference oracle ($label)"
    echo "--- Rust (canonical) ---"; echo "$rs_canon"
    echo "--- agent-kgpacks-ts (canonical) ---"; echo "$ts_canon"
    exit 1
  fi
  echo "$rs"
}

assert_contains() {
  # assert_contains <file> <needle> <message>
  if ! grep -qF "$2" "$1"; then
    echo "RELEASE_PARITY_FAILED: $3"
    cat "$1"
    exit 1
  fi
}

# --- Case 1: dated tag → unpadded version + latest-pointer + mirrored prov. ---
DATED="$(run_case dated "cve-2025.06")"
assert_contains "$DATED" '"version": "2025.6.0"' "dated tag must derive UNPADDED version 2025.6.0"
assert_contains "$DATED" '"cve-2025.06"' "dated tag must appear in publishTargets"
assert_contains "$DATED" '"packs"' "publishTargets must include the packs latest-pointer"
assert_contains "$DATED" '"cve.pack-release.json"' "indexFilename must be <name>.pack-release.json"
assert_contains "$DATED" '"commit": "abc123"' "provenance must be mirrored from the manifest"

# --- Case 2: dated tag with an explicit patch. -------------------------------
run_case dated_patch "cve-2024.12.3" >/dev/null

# --- Case 3: overrides fill corpus commit/date and the top-level model. ------
OVR="$(run_case overrides "cve-2027.03" "custom/model" "deadbeef" "2027-03-01")"
assert_contains "$OVR" '"version": "2027.3.0"' "override case must still derive the dated version"

# --- Case 4: bare packs pointer → no dated version (manifest fallback). -------
POINTER="$(run_case pointer "packs")"
assert_contains "$POINTER" '"version": "0.1.0"' "the packs pointer must fall back to the manifest version"
assert_contains "$POINTER" '"publishTargets": [' "publishTargets present for the pointer"

echo "checked Rust pack release-plan == agent-kgpacks-ts reference oracle == expected WS3 outcomes"
echo "RELEASE_PARITY_OK"
