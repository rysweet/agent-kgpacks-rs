#!/usr/bin/env bash
# Cross-implementation `status` parity check (qa-team / gadugi-agentic-test).
#
# Proves the Rust `kgpacks status` command produces a payload structurally
# identical to the canonical `agent-kgpacks-ts` `status` command
# (`{ packsDir, count, packs: [{ name, version, dbPresent }] }`, packs sorted by
# name) over a shared, synthetic packs directory that exercises:
#   * packs WITH and WITHOUT a database store present (dbPresent true/false),
#   * a directory whose manifest is invalid (missing version) -> skipped,
#   * a directory with no manifest -> skipped,
#   * a stray non-directory entry at the root -> skipped,
#   * name sorting that DISCRIMINATES ICU `localeCompare` from naive codepoint
#     order: an underscore-vs-hyphen pair (`my_pack`/`my-pack`), an
#     underscore-vs-digit pair (`a_1`/`a1`), and a case-only pair
#     (`alpha`/`Alpha`) — so the harness actually falsifies the sort behavior.
#
# Because JSON object key order is not semantically meaningful (serde_json emits
# keys alphabetically; the reference preserves insertion order), both outputs are
# canonicalized (recursively sorted keys) before comparison.
#
# Backend note: `dbPresent` is checked against each implementation's own store
# filename — LadybugDB `graph.lbug` for Rust, `pack.db` for the reference — so
# the fixture materializes BOTH for a "present" pack. That store filename is the
# one intentional backend difference; the status PAYLOAD is at full parity.
#
# Scope note: this harness always passes an explicit `--packs-dir`, so it asserts
# parity of the status PAYLOAD, not the DEFAULT packs-dir resolution. The Rust
# port still defaults to `./packs` whereas the reference defaults to an XDG data
# dir — a pre-existing, separately-tracked divergence (issue #19), shared by all
# commands and out of scope for the `status` payload parity checked here.
#
# 1. Materializes the shared fixture packs dir.
# 2. Runs the Rust `kgpacks status` over it.
# 3. Runs the live TypeScript reference oracle over the SAME fixture.
# 4. Canonicalizes both and asserts byte-for-byte equality — a true side-by-side
#    Rust-vs-TS comparison, not just a snapshot.
# 5. Asserts the concrete expected `packs` array (a path-independent golden).
#
# Prints `STATUS_PARITY_OK` and exits 0 on success; prints the diff and exits 1
# on any divergence.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"
ORACLE="$HERE/reference_oracle.mjs"

cd "$REPO_ROOT"

if ! command -v node >/dev/null 2>&1; then
  echo "STATUS_PARITY_FAILED: node is required to run the TypeScript reference oracle"
  exit 1
fi

WORK="$(mktemp -d)"
FIX="$WORK/packs"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$FIX"

# --- Build the shared fixture packs directory. -------------------------------
# A "present" pack gets BOTH store filenames so RS (graph.lbug) and the oracle
# (pack.db) each see dbPresent=true over the same directory.
make_pack() {
  local dir="$1" name="$2" version="$3" db_present="$4"
  mkdir -p "$FIX/$dir"
  printf '{"name":"%s","version":"%s"}' "$name" "$version" > "$FIX/$dir/manifest.json"
  if [ "$db_present" = "yes" ]; then
    : > "$FIX/$dir/graph.lbug" # Rust (LadybugDB) store
    : > "$FIX/$dir/pack.db"    # reference store
  fi
}

make_pack alpha   alpha   1.0.0 yes
make_pack Alpha   Alpha   1.0.1 no
make_pack a1      a1      2.0.0 no
make_pack a_1     a_1     2.0.1 yes
make_pack my-pack my-pack 3.0.0 no
make_pack my_pack my_pack 3.0.1 yes
make_pack bravo   bravo   0.5.0 no

# Directory with an invalid manifest (missing required `version`) -> skipped.
mkdir -p "$FIX/bad-manifest"
printf '{"name":"bad-manifest"}' > "$FIX/bad-manifest/manifest.json"
# Directory with no manifest at all -> skipped.
mkdir -p "$FIX/no-manifest"
# Stray non-directory entry at the root -> skipped.
: > "$FIX/stray.txt"

# --- Run both implementations over the SAME fixture. -------------------------
RS_JSON="$WORK/rs.json"
TS_JSON="$WORK/ts.json"

cargo run -q -p kgpacks-cli --locked -- status --packs-dir "$FIX" > "$RS_JSON"
node "$ORACLE" status "$FIX" > "$TS_JSON"

RS_CANON="$(node "$ORACLE" canon "$RS_JSON")"
TS_CANON="$(node "$ORACLE" canon "$TS_JSON")"

# --- 4: Rust output must equal the live TypeScript reference (canonicalized). -
if [ "$RS_CANON" != "$TS_CANON" ]; then
  echo "STATUS_PARITY_FAILED: Rust output diverged from the live agent-kgpacks-ts reference oracle"
  echo "--- Rust (canonical) ---"
  echo "$RS_CANON"
  echo "--- agent-kgpacks-ts (canonical) ---"
  echo "$TS_CANON"
  exit 1
fi

# --- 5: Concrete, path-independent golden for the `packs` array. -------------
# Order encodes the ICU `localeCompare` semantics (verified against Node): the
# underscore/hyphen, underscore/digit, and lowercase/uppercase pairs each sort
# in the reference order, not naive codepoint order.
EXPECTED_PACKS='"packs":[{"dbPresent":true,"name":"a_1","version":"2.0.1"},{"dbPresent":false,"name":"a1","version":"2.0.0"},{"dbPresent":true,"name":"alpha","version":"1.0.0"},{"dbPresent":false,"name":"Alpha","version":"1.0.1"},{"dbPresent":false,"name":"bravo","version":"0.5.0"},{"dbPresent":true,"name":"my_pack","version":"3.0.1"},{"dbPresent":false,"name":"my-pack","version":"3.0.0"}]'
if ! printf '%s' "$RS_CANON" | grep -qF "$EXPECTED_PACKS"; then
  echo "STATUS_PARITY_FAILED: Rust status payload did not match the expected packs golden"
  echo "--- Rust (canonical) ---"
  echo "$RS_CANON"
  echo "--- expected substring ---"
  echo "$EXPECTED_PACKS"
  exit 1
fi

# --- 5b: count must reflect only the seven valid packs. ----------------------
if ! printf '%s' "$RS_CANON" | grep -qF '"count":7'; then
  echo "STATUS_PARITY_FAILED: expected count=7 (seven valid packs), got:"
  echo "$RS_CANON"
  exit 1
fi

echo "checked Rust status == live agent-kgpacks-ts oracle == expected golden"
echo "STATUS_PARITY_OK"
