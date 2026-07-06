#!/usr/bin/env bash
# WS8 coverage harness (qa-team / gadugi-agentic-test).
#
# Runs the committed WS8 acceptance coverage and asserts it passes:
#
#   * Entity-graph traversal (kgpacks-query `tests/entity_graph.rs`): the
#     transport-agnostic `entity_graph` over Entity / HAS_ENTITY /
#     ENTITY_RELATION — co-occurrence depth 1/2, type filter, auto-mode
#     selection (relation when ENTITY_RELATION edges exist, else co-occurrence),
#     relation traversal, deterministic ordering, `limit`, strict depth
#     validation and the unknown-seed error.
#
#   * Scalable bulk ENTITY_RELATION load (kgpacks-ingestion
#     `tests/entity_relations.rs`): the COPY-first bulk loader and its
#     PK-indexed UNWIND fallback — correctness, property/CSV round-trip, append
#     into a non-empty table, an empty no-op, dangling-endpoint robustness, and
#     a 5,000-edge batch (the scalable, non-O(N^2) path at size).
#
#   * Backend GET /api/v1/graph/entities (kgpacks-backend
#     `tests/graph_entities.rs`): a valid request builds the neighborhood; an
#     unknown seed -> 404 NOT_FOUND; a missing entity -> 400 MISSING_PARAMETER;
#     an out-of-range depth -> 400 INVALID_PARAMETER; depth/limit honored.
#
# These are the same tests CI runs as part of `cargo test --workspace`; this
# harness is the qa-team entry point that drives just the WS8 subset and makes
# the pass/fail explicit.
#
# Prints `WS8_COVERAGE_OK` and exits 0 on success; prints the failing output and
# exits 1 on any failure.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"
cd "$REPO_ROOT"

if ! command -v cargo >/dev/null 2>&1; then
  echo "WS8_COVERAGE_FAILED: cargo (Rust toolchain) is required on PATH"
  exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
LOG="$WORK/ws8.log"

fail() {
  echo "WS8_COVERAGE_FAILED: $1"
  shift || true
  for extra in "$@"; do echo "$extra"; done
  [ -f "$LOG" ] && { echo "--- test output ---"; cat "$LOG"; }
  exit 1
}

# 1) Entity-graph traversal query.
echo "== WS8: entity-graph traversal (kgpacks-query) =="
cargo test -p kgpacks-query --locked --test entity_graph 2>&1 | tee "$LOG" \
  || fail "entity_graph query tests failed"

# 2) Scalable bulk ENTITY_RELATION load.
echo "== WS8: scalable bulk ENTITY_RELATION load (kgpacks-ingestion) =="
cargo test -p kgpacks-ingestion --locked --test entity_relations 2>&1 | tee -a "$LOG" \
  || fail "bulk ENTITY_RELATION loader tests failed"

# 3) Backend GET /api/v1/graph/entities.
echo "== WS8: backend graph-entities API (kgpacks-backend) =="
cargo test -p kgpacks-backend --locked --test graph_entities 2>&1 | tee -a "$LOG" \
  || fail "backend graph-entities route/service tests failed"

# Defensive: assert no test binary reported a failure and that tests actually ran
# (guards against a vacuous "0 passed" pass).
if grep -qE "test result: FAILED" "$LOG"; then
  fail "a WS8 test binary reported FAILED"
fi
if ! grep -qE "test result: ok\. [1-9][0-9]* passed" "$LOG"; then
  fail "expected at least one WS8 test to run and pass"
fi

echo "checked WS8 entity-graph traversal + scalable bulk ENTITY_RELATION load + backend API"
echo "WS8_COVERAGE_OK"
