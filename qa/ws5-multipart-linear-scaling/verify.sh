#!/usr/bin/env bash
# WS5 coverage harness (qa-team / gadugi-agentic-test).
#
# Runs the committed WS5 acceptance coverage and asserts it passes:
#
#   * >2 GiB multi-part release accounting over a synthetic pack
#     (kgpacks-packs `tests/multipart_release.rs` + the `release`/`sha256`
#     unit tests): a genuine multi-part split with a tiny part-size over
#     incompressible bytes (every non-final part == part_size,
#     sum(parts) == total, per-part + overall SHA-256, byte-exact reassembly)
#     and the >2 GiB size accounting computed without materializing gigabytes.
#
#   * Linear-scaling loader guard (structural, not timing) in both
#     kgpacks-packs and kgpacks-ingestion `tests/linear_scaling_guard.rs`:
#     loading 2N records issues at most ~3x the statements of N, and NO
#     edge-creation statement uses the O(N^2) comma two-pattern
#     `MATCH (a {..}), (b {..})` (PK-indexed single-MATCH clauses only).
#
# These are the same tests CI runs as part of `cargo test --workspace`; this
# harness is the qa-team entry point that drives just the WS5 subset and makes
# the pass/fail explicit.
#
# Prints `WS5_COVERAGE_OK` and exits 0 on success; prints the failing output and
# exits 1 on any failure.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"
cd "$REPO_ROOT"

if ! command -v cargo >/dev/null 2>&1; then
  echo "WS5_COVERAGE_FAILED: cargo (Rust toolchain) is required on PATH"
  exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
LOG="$WORK/ws5.log"

fail() {
  echo "WS5_COVERAGE_FAILED: $1"
  shift || true
  for extra in "$@"; do echo "$extra"; done
  [ -f "$LOG" ] && { echo "--- test output ---"; cat "$LOG"; }
  exit 1
}

# 1) Multi-part release accounting + the release/sha256 unit tests.
echo "== WS5: multi-part release accounting (kgpacks-packs) =="
cargo test -p kgpacks-packs --locked --lib --test multipart_release 2>&1 | tee "$LOG" \
  || fail "multi-part release / sha256 tests failed"

# 2) Linear-scaling structural guards (packs + ingestion).
echo "== WS5: linear-scaling guards (kgpacks-packs + kgpacks-ingestion) =="
cargo test -p kgpacks-packs -p kgpacks-ingestion --locked \
  --test linear_scaling_guard 2>&1 | tee -a "$LOG" \
  || fail "linear-scaling guard tests failed"

# Defensive: assert no test binary reported a failure and that tests actually ran
# (guards against a vacuous "0 passed" pass).
if grep -qE "test result: FAILED" "$LOG"; then
  fail "a WS5 test binary reported FAILED"
fi
if ! grep -qE "test result: ok\. [1-9][0-9]* passed" "$LOG"; then
  fail "expected at least one WS5 test to run and pass"
fi

echo "checked WS5 multi-part accounting (incl. >2 GiB, no materialization) + linear-scaling guards"
echo "WS5_COVERAGE_OK"
