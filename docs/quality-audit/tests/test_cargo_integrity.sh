#!/usr/bin/env bash
# File: docs/quality-audit/tests/test_cargo_integrity.sh
#
# Contract: `--locked` / Cargo.lock integrity and the CI gate shape
# (design decisions A8, requirements 6 & 7). Fully offline: it inspects the
# committed Cargo.lock and .github/workflows/ci.yml. It does NOT compile
# (the LadybugDB C++ build is ~22 min and networked).

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
. "$HERE/lib.sh"

REPO_ROOT="$(git -C "$HERE" rev-parse --show-toplevel)"
LOCK="$REPO_ROOT/Cargo.lock"
ROOT_MANIFEST="$REPO_ROOT/Cargo.toml"
CI="$REPO_ROOT/.github/workflows/ci.yml"

t_begin "cargo & CI integrity (A8 / req 6,7)"

assert "Cargo.lock exists" test -f "$LOCK"
assert "Cargo.lock is a real lockfile ([[package]] present)" \
  file_has "$LOCK" '^\[\[package\]\]'

# Every workspace member crate must be resolved in the lockfile.
for crate in kgpacks-agent kgpacks-backend kgpacks-cli kgpacks-db \
             kgpacks-embeddings kgpacks-eval kgpacks-ingestion kgpacks-mcp \
             kgpacks-packs kgpacks-query; do
  assert "Cargo.lock resolves workspace crate: $crate" \
    file_has "$LOCK" "^name = \"$crate\"$"
done

# Pinned upstream engine must be locked at the manifest's pinned version.
# Helper takes positional args (lockfile, version) so nothing is interpolated
# into a `bash -c` string — no injection surface, no quoting hazards.
lock_locks_pkg_version() {
  # $1=lockfile  $2=package name  $3=version
  grep -A2 -- "^name = \"$2\"\$" "$1" | grep -Fq -- "version = \"$3\""
}
assert "root manifest pins lbug =0.15.3" file_has_str "$ROOT_MANIFEST" 'lbug = "=0.15.3"'
assert "Cargo.lock locks lbug 0.15.3" lock_locks_pkg_version "$LOCK" "lbug" "0.15.3"

# CI must enforce all four gates under --locked, plus the copilot-feature step.
assert "CI runs fmt check" file_has_str "$CI" 'cargo fmt --all -- --check'
assert "CI builds with --locked" \
  file_has_str "$CI" 'cargo build --workspace --all-targets --locked'
assert "CI tests with --locked" \
  file_has_str "$CI" 'cargo test --workspace --locked'
assert "CI runs clippy -D warnings with --locked" \
  file_has_str "$CI" 'cargo clippy --workspace --all-targets --locked -- -D warnings'
assert "CI exercises the copilot feature (real transport)" \
  file_has_str "$CI" '--features copilot'

t_end
