#!/usr/bin/env bash
# Cross-implementation read-path `pack` group parity check
# (qa-team / gadugi-agentic-test).
#
# Proves the Rust `kgpacks pack {list,info,validate}` subcommands produce
# payloads structurally identical to the canonical `agent-kgpacks-ts` `pack`
# command group over a shared, synthetic packs directory that exercises:
#   * `pack list`     — the `[{ name, version, description }]` projection, with
#                       description defaulting to "" when absent, sorted by name
#                       with the ICU `localeCompare` order that DISCRIMINATES from
#                       naive codepoint order (underscore-vs-hyphen `my_pack`/
#                       `my-pack`, underscore-vs-digit `a_1`/`a1`, and case-only
#                       `alpha`/`Alpha` pairs), plus skipped entries (a directory
#                       with an invalid manifest, one with no manifest, and a
#                       stray non-directory file);
#   * `pack info`     — a pack's FULL manifest, including description, graph_stats,
#                       eval_scores, a provenance block, and an unknown top-level
#                       key that must be preserved verbatim;
#   * `pack validate` — the `{ valid, name, version }` shape for a valid pack.
#
# Because JSON object key order is not semantically meaningful (serde_json emits
# keys alphabetically; the reference preserves insertion order) and the two ports
# spell integral numbers differently (`10` vs `10.0`), both outputs are
# canonicalized (recursively sorted keys, re-parsed numbers) before comparison.
#
# Scope note: like `qa/status-parity`, this harness always passes an explicit
# `--packs-dir`, so it asserts parity of the `pack` PAYLOADS, not the DEFAULT
# packs-dir resolution (a pre-existing, separately-tracked divergence, issue #19).
# The read-path `pack` subcommands assert the happy-path payloads; the write/
# network subcommands (`install`/`pull`/`remove`) and ingestion/eval verbs
# (`create`/`update`/`eval`) remain follow-ups (issue #13).
#
# 1. Materializes the shared fixture packs dir.
# 2. Runs each Rust `kgpacks pack …` subcommand over it.
# 3. Runs the live TypeScript reference oracle over the SAME fixture.
# 4. Canonicalizes both and asserts equality — a true side-by-side Rust-vs-TS
#    comparison, not just a snapshot.
# 5. Asserts concrete, path-independent goldens for the list order + validate shape.
#
# Prints `PACK_PARITY_OK` and exits 0 on success; prints the diff and exits 1
# on any divergence.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"
ORACLE="$HERE/reference_oracle.mjs"

cd "$REPO_ROOT"

if ! command -v node >/dev/null 2>&1; then
  echo "PACK_PARITY_FAILED: node is required to run the TypeScript reference oracle"
  exit 1
fi

WORK="$(mktemp -d)"
FIX="$WORK/packs"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$FIX"

# --- Build the shared fixture packs directory. -------------------------------
make_pack() {
  # make_pack <dir> <manifest-json>
  local dir="$1" manifest="$2"
  mkdir -p "$FIX/$dir"
  printf '%s' "$manifest" > "$FIX/$dir/manifest.json"
}

# Sort-discriminating pairs (mirror qa/status-parity), with a mix of present /
# absent `description` fields.
make_pack a_1     '{"name":"a_1","version":"2.0.1","description":"Underscore-one"}'
make_pack a1      '{"name":"a1","version":"2.0.0"}'
make_pack alpha   '{"name":"alpha","version":"1.0.0","description":"Lower alpha"}'
make_pack Alpha   '{"name":"Alpha","version":"1.0.1","description":"Upper alpha"}'
make_pack bravo   '{"name":"bravo","version":"0.5.0"}'
make_pack my-pack '{"name":"my-pack","version":"3.0.0"}'
make_pack my_pack '{"name":"my_pack","version":"3.0.1","description":"Underscore pack"}'

# A rich manifest exercising every optional section + an unknown top-level key
# (`channel`) that both implementations must preserve verbatim.
make_pack rich '{"name":"rich","version":"1.2.3","description":"Rich pack","graph_stats":{"articles":294,"entities":1200,"size_mb":12.5},"eval_scores":{"recall":0.9,"precision":0.75},"provenance":{"corpus":{"name":"cvelistV5","commit":"abc123","date":"2024-01-01"},"embedding":{"model":"hash-256","dimensions":256},"build":{"date":"2024-06-01","tool_version":"0.1.1"}},"channel":"stable"}'

# Directory with an invalid manifest (missing required `version`) -> skipped.
make_pack bad-manifest '{"name":"bad-manifest"}'
# Directory with no manifest at all -> skipped.
mkdir -p "$FIX/no-manifest"
# Stray non-directory entry at the root -> skipped.
: > "$FIX/stray.txt"

canon() { node "$ORACLE" canon "$1"; }

fail() {
  echo "PACK_PARITY_FAILED: $1"
  shift
  for extra in "$@"; do echo "$extra"; done
  exit 1
}

RUN_CLI() { cargo run -q -p kgpacks-cli --locked -- "$@"; }

# =============================================================================
# 1) `pack list`
# =============================================================================
RS_LIST="$WORK/rs-list.json"
TS_LIST="$WORK/ts-list.json"
RUN_CLI pack list --packs-dir "$FIX" > "$RS_LIST"
node "$ORACLE" pack-list "$FIX" > "$TS_LIST"

RS_LIST_CANON="$(canon "$RS_LIST")"
TS_LIST_CANON="$(canon "$TS_LIST")"
if [ "$RS_LIST_CANON" != "$TS_LIST_CANON" ]; then
  fail "pack list diverged from the live agent-kgpacks-ts reference oracle" \
    "--- Rust (canonical) ---" "$RS_LIST_CANON" \
    "--- agent-kgpacks-ts (canonical) ---" "$TS_LIST_CANON"
fi

# Concrete, path-independent golden: the ICU sort order + description defaulting.
EXPECTED_LIST='[{"description":"Underscore-one","name":"a_1","version":"2.0.1"},{"description":"","name":"a1","version":"2.0.0"},{"description":"Lower alpha","name":"alpha","version":"1.0.0"},{"description":"Upper alpha","name":"Alpha","version":"1.0.1"},{"description":"","name":"bravo","version":"0.5.0"},{"description":"Underscore pack","name":"my_pack","version":"3.0.1"},{"description":"","name":"my-pack","version":"3.0.0"},{"description":"Rich pack","name":"rich","version":"1.2.3"}]'
if [ "$RS_LIST_CANON" != "$EXPECTED_LIST" ]; then
  fail "pack list payload did not match the expected golden" \
    "--- Rust (canonical) ---" "$RS_LIST_CANON" \
    "--- expected ---" "$EXPECTED_LIST"
fi

# =============================================================================
# 2) `pack info rich` — the full manifest, verbatim (extra key + provenance).
# =============================================================================
RS_INFO="$WORK/rs-info.json"
TS_INFO="$WORK/ts-info.json"
RUN_CLI pack info rich --packs-dir "$FIX" > "$RS_INFO"
node "$ORACLE" pack-info "$FIX" rich > "$TS_INFO"

RS_INFO_CANON="$(canon "$RS_INFO")"
TS_INFO_CANON="$(canon "$TS_INFO")"
if [ "$RS_INFO_CANON" != "$TS_INFO_CANON" ]; then
  fail "pack info diverged from the live agent-kgpacks-ts reference oracle" \
    "--- Rust (canonical) ---" "$RS_INFO_CANON" \
    "--- agent-kgpacks-ts (canonical) ---" "$TS_INFO_CANON"
fi

# Concrete golden: the full canonical manifest (unknown `channel` key + nested
# provenance preserved verbatim, integral stats normalized).
EXPECTED_INFO='{"channel":"stable","description":"Rich pack","eval_scores":{"precision":0.75,"recall":0.9},"graph_stats":{"articles":294,"entities":1200,"size_mb":12.5},"name":"rich","provenance":{"build":{"date":"2024-06-01","tool_version":"0.1.1"},"corpus":{"commit":"abc123","date":"2024-01-01","name":"cvelistV5"},"embedding":{"dimensions":256,"model":"hash-256"}},"version":"1.2.3"}'
if [ "$RS_INFO_CANON" != "$EXPECTED_INFO" ]; then
  fail "pack info payload did not match the expected golden" \
    "--- Rust (canonical) ---" "$RS_INFO_CANON" \
    "--- expected ---" "$EXPECTED_INFO"
fi

# =============================================================================
# 3) `pack validate rich` — the `{ valid, name, version }` shape.
# =============================================================================
RS_VAL="$WORK/rs-val.json"
TS_VAL="$WORK/ts-val.json"
RUN_CLI pack validate rich --packs-dir "$FIX" > "$RS_VAL"
node "$ORACLE" pack-validate "$FIX" rich > "$TS_VAL"

RS_VAL_CANON="$(canon "$RS_VAL")"
TS_VAL_CANON="$(canon "$TS_VAL")"
if [ "$RS_VAL_CANON" != "$TS_VAL_CANON" ]; then
  fail "pack validate diverged from the live agent-kgpacks-ts reference oracle" \
    "--- Rust (canonical) ---" "$RS_VAL_CANON" \
    "--- agent-kgpacks-ts (canonical) ---" "$TS_VAL_CANON"
fi

EXPECTED_VAL='{"name":"rich","valid":true,"version":"1.2.3"}'
if [ "$RS_VAL_CANON" != "$EXPECTED_VAL" ]; then
  fail "pack validate payload did not match the expected golden" \
    "--- Rust (canonical) ---" "$RS_VAL_CANON" \
    "--- expected ---" "$EXPECTED_VAL"
fi

echo "checked Rust pack {list,info,validate} == live agent-kgpacks-ts oracle == expected goldens"
echo "PACK_PARITY_OK"
