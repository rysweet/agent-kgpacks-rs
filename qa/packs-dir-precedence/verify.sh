#!/usr/bin/env bash
# WS4 packs-directory resolution check (qa-team / gadugi-agentic-test).
#
# Proves the Rust `kgpacks` binary resolves the directory that holds installed
# packs with the WS4 precedence (issue #19), end to end through the REAL command
# surface: the `status` command reports the resolved directory in its `packsDir`
# field, so each case asserts the winning directory.
#
# Precedence (highest first):
#   1. the `--packs-dir <dir>` flag (explicit override);
#   2. the `KGPACKS_PACKS_DIR` environment variable;
#   3. the XDG default: `$XDG_DATA_HOME/kgpacks` when `XDG_DATA_HOME` is set
#      (non-empty), otherwise `$HOME/.local/share/kgpacks`.
# Empty / whitespace-only overrides (flag and env) are treated as unset.
#
# Each invocation sets the resolution variables on the CHILD process only (via
# `env`), never on this harness's own environment, so cases never leak into one
# another. This mirrors the in-tree e2e test but exercises the shipped binary
# from the outside.
#
# Prints `PACKS_DIR_PRECEDENCE_OK` and exits 0 on success; prints the offending
# case and exits 1 on any divergence.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"
cd "$REPO_ROOT"

# Build once, then invoke the compiled binary directly so no `cargo`/inherited
# environment perturbs the per-case variables under test. (Invoking `cargo run`
# instead would forward the manipulated env — including an unset HOME — to cargo
# itself, breaking its ~/.cargo access; the built binary needs no such state.)
cargo build -q -p kgpacks-cli --locked
# Honour a CARGO_TARGET_DIR override (used by parallel worktrees); default to the
# workspace-local `target/` as CI does.
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
BIN="$TARGET_DIR/debug/kgpacks"
if [ ! -x "$BIN" ]; then
  echo "PACKS_DIR_PRECEDENCE_FAILED: built binary not found at $BIN"
  exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
FLAG_DIR="$WORK/flag"
ENV_DIR="$WORK/env"
XDG_HOME="$WORK/xdg"
HOME_DIR="$WORK/home"

# Extract the `packsDir` string the `status` payload reports.
packs_dir_of() {
  sed -n 's/.*"packsDir"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1
}

assert_eq() {
  # assert_eq <case-name> <expected> <actual>
  if [ "$2" != "$3" ]; then
    echo "PACKS_DIR_PRECEDENCE_FAILED: $1"
    echo "  expected: $2"
    echo "  actual:   $3"
    exit 1
  fi
}

# (1) --packs-dir flag wins over BOTH the env override and the XDG default.
got="$(env -u HOME KGPACKS_PACKS_DIR="$ENV_DIR" XDG_DATA_HOME="$XDG_HOME" \
  "$BIN" --packs-dir "$FLAG_DIR" status | packs_dir_of)"
assert_eq "flag must beat env + xdg" "$FLAG_DIR" "$got"

# (2) KGPACKS_PACKS_DIR wins over the XDG default when there is no flag.
got="$(env -u HOME KGPACKS_PACKS_DIR="$ENV_DIR" XDG_DATA_HOME="$XDG_HOME" \
  "$BIN" status | packs_dir_of)"
assert_eq "env must beat xdg default" "$ENV_DIR" "$got"

# (3) With no flag and no env override, the default is $XDG_DATA_HOME/kgpacks.
got="$(env -u KGPACKS_PACKS_DIR -u HOME XDG_DATA_HOME="$XDG_HOME" \
  "$BIN" status | packs_dir_of)"
assert_eq "xdg default" "$XDG_HOME/kgpacks" "$got"

# (4) With XDG unset too, the default falls back to $HOME/.local/share/kgpacks.
got="$(env -u KGPACKS_PACKS_DIR -u XDG_DATA_HOME HOME="$HOME_DIR" \
  "$BIN" status | packs_dir_of)"
assert_eq "home fallback default" "$HOME_DIR/.local/share/kgpacks" "$got"

# (5) A blank (whitespace-only) env override is treated as unset and falls
#     through to the XDG default rather than resolving to an empty path.
got="$(env -u HOME KGPACKS_PACKS_DIR="   " XDG_DATA_HOME="$XDG_HOME" \
  "$BIN" status | packs_dir_of)"
assert_eq "blank env falls through to xdg default" "$XDG_HOME/kgpacks" "$got"

echo "checked flag > KGPACKS_PACKS_DIR > \$XDG_DATA_HOME/kgpacks > \$HOME/.local/share/kgpacks (blank overrides ignored)"
echo "PACKS_DIR_PRECEDENCE_OK"
