#!/usr/bin/env bash
# Cross-implementation manifest `provenance` validation parity check
# (qa-team / gadugi-agentic-test).
#
# Proves the Rust manifest validator (`kgpacks-packs::validate_manifest`, reached
# through the `kgpacks status` read-path via `registry::list_packs`) accepts and
# rejects `provenance` blocks byte-for-byte identically to the canonical
# `agent-kgpacks-ts` `validateManifest` (`packages/packs/src/manifest.ts`
# `validateProvenance`). This closes agent-kgpacks-rs #28: before the fix the Rust
# port ignored the `provenance` block, so a pack with a malformed `provenance`
# block was LISTED by Rust but SKIPPED by the reference — a manifest-schema
# parity gap. After the fix both skip it.
#
# Method (a true side-by-side falsifiable check, mirroring qa/status-parity):
#   1. Materialize ONE shared packs dir mixing packs with valid provenance
#      (full, null-valued declared fields, `provenance: null`, and no provenance)
#      against packs with EACH documented malformed-provenance shape.
#   2. Run the Rust `kgpacks status` over it.
#   3. Run the shared JS reference oracle — a live-run, dependency-free port of
#      the agent-kgpacks-ts `validateManifest` semantics (the provenance-aware
#      qa/status-parity/reference_oracle.mjs) over the SAME dir.
#   4. Canonicalize (recursively sort keys) and assert byte-for-byte equality:
#      both implementations must agree on exactly which packs are valid.
#   5. Assert the concrete outcome: the four valid-provenance packs are listed
#      (count == 4) and none of the malformed-provenance packs leak in.
#
# Prints `PROVENANCE_PARITY_OK` and exits 0 on success; prints the diff and
# exits 1 on any divergence.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"
ORACLE="$REPO_ROOT/qa/status-parity/reference_oracle.mjs"

cd "$REPO_ROOT"

if ! command -v node >/dev/null 2>&1; then
  echo "PROVENANCE_PARITY_FAILED: node is required to run the TypeScript reference oracle"
  exit 1
fi
if [ ! -f "$ORACLE" ]; then
  echo "PROVENANCE_PARITY_FAILED: shared reference oracle not found at $ORACLE"
  exit 1
fi

WORK="$(mktemp -d)"
FIX="$WORK/packs"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$FIX"

write_pack() {
  # write_pack <dir> <manifest-json>
  local dir="$1" json="$2"
  mkdir -p "$FIX/$dir"
  printf '%s' "$json" > "$FIX/$dir/manifest.json"
}

# --- Valid packs (accepted by BOTH implementations). -------------------------
# Full, well-formed provenance block.
write_pack valid-full '{"name":"valid-full","version":"1.0.0","provenance":{"corpus":{"name":"cvelistV5","commit":"abc123","date":"2026-01-01"},"embedding":{"model":"bge-base-en-v1.5","dimensions":768},"build":{"date":"2026-01-02T00:00:00Z","tool_version":"0.1.0"}}}'
# Undeterminable declared fields recorded as null, unknown section tolerated.
write_pack valid-nulls '{"name":"valid-nulls","version":"1.0.0","provenance":{"corpus":{"name":"cvelistV5","commit":null,"date":null},"embedding":{"model":"hash","dimensions":0},"extra_section":{"note":"tolerated"}}}'
# `provenance: null` is treated as absent (accepted).
write_pack valid-none '{"name":"valid-none","version":"1.0.0","provenance":null}'
# Control: no provenance at all.
write_pack valid-no-prov '{"name":"valid-no-prov","version":"1.0.0"}'

# --- Malformed provenance (REJECTED by BOTH -> skipped, not listed). ---------
# provenance is not an object.
write_pack bad-prov-notobj '{"name":"bad-prov-notobj","version":"1.0.0","provenance":"nope"}'
# a section is not an object.
write_pack bad-section '{"name":"bad-section","version":"1.0.0","provenance":{"corpus":"not-an-object"}}'
# a declared string field is not a string.
write_pack bad-field '{"name":"bad-field","version":"1.0.0","provenance":{"corpus":{"name":123}}}'
# embedding.dimensions is negative.
write_pack bad-dims '{"name":"bad-dims","version":"1.0.0","provenance":{"embedding":{"model":"m","dimensions":-1}}}'
# embedding.dimensions is a string, not a number.
write_pack bad-dims-str '{"name":"bad-dims-str","version":"1.0.0","provenance":{"embedding":{"model":"m","dimensions":"768"}}}'

# Non-provenance skips, to keep the fixture honest.
mkdir -p "$FIX/no-manifest"   # no manifest -> skipped
: > "$FIX/stray.txt"          # stray file  -> skipped

# --- Run both implementations over the SAME fixture. -------------------------
RS_JSON="$WORK/rs.json"
TS_JSON="$WORK/ts.json"

cargo run -q -p kgpacks-cli --locked -- status --packs-dir "$FIX" > "$RS_JSON"
node "$ORACLE" status "$FIX" > "$TS_JSON"

RS_CANON="$(node "$ORACLE" canon "$RS_JSON")"
TS_CANON="$(node "$ORACLE" canon "$TS_JSON")"

# --- 4: Rust must equal the JS reference oracle output (canonicalized). --------
if [ "$RS_CANON" != "$TS_CANON" ]; then
  echo "PROVENANCE_PARITY_FAILED: Rust manifest-provenance gate diverged from the agent-kgpacks-ts reference oracle"
  echo "--- Rust (canonical) ---"
  echo "$RS_CANON"
  echo "--- agent-kgpacks-ts (canonical) ---"
  echo "$TS_CANON"
  exit 1
fi

# --- 5: Concrete outcome — only the four valid-provenance packs are listed. ---
if ! printf '%s' "$RS_CANON" | grep -qF '"count":4'; then
  echo "PROVENANCE_PARITY_FAILED: expected count=4 (four valid-provenance packs), got:"
  echo "$RS_CANON"
  exit 1
fi
for good in valid-full valid-nulls valid-none valid-no-prov; do
  if ! printf '%s' "$RS_CANON" | grep -qF "\"name\":\"$good\""; then
    echo "PROVENANCE_PARITY_FAILED: expected valid pack '$good' to be listed"
    echo "$RS_CANON"
    exit 1
  fi
done
for bad in bad-prov-notobj bad-section bad-field bad-dims bad-dims-str; do
  if printf '%s' "$RS_CANON" | grep -qF "\"name\":\"$bad\""; then
    echo "PROVENANCE_PARITY_FAILED: malformed-provenance pack '$bad' leaked into the listing"
    echo "$RS_CANON"
    exit 1
  fi
done

echo "checked Rust manifest-provenance gate == agent-kgpacks-ts reference oracle == expected outcome"
echo "PROVENANCE_PARITY_OK"
