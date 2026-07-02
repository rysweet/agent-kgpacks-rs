# File: docs/quality-audit/tests/lib.sh
# Tiny, dependency-free assertion harness for the Ten-Wave Quality Audit
# contract tests. Sourced by every test_*.sh. Uses only POSIX tools + git/gh.
#
# A test script sources this, calls t_begin, runs assertions (ok/nok are
# tracked automatically), then calls t_end which exits non-zero if any
# assertion failed. This lets a script keep running after a failed assertion
# (so we see the full picture) while still failing the suite overall.

# Do NOT set -e: assertions rely on non-zero exit codes without aborting.
set -uo pipefail

T_PASS=0
T_FAIL=0
T_SUITE="(unnamed)"

t_begin() {
  T_SUITE="$1"
  T_PASS=0
  T_FAIL=0
  printf '# ---- %s ----\n' "$T_SUITE"
}

ok() {
  T_PASS=$((T_PASS + 1))
  printf 'ok %d - %s\n' "$((T_PASS + T_FAIL))" "$1"
}

nok() {
  T_FAIL=$((T_FAIL + 1))
  printf 'not ok %d - %s\n' "$((T_PASS + T_FAIL))" "$1"
  if [ "${2:-}" != "" ]; then
    printf '#   %s\n' "$2"
  fi
}

# assert <desc> <cmd...>  -> pass when cmd exits 0
assert() {
  local desc=$1
  shift
  if "$@" >/dev/null 2>&1; then
    ok "$desc"
  else
    nok "$desc" "expected success, got exit $? from: $*"
  fi
}

# refute <desc> <cmd...>  -> pass when cmd exits non-zero
refute() {
  local desc=$1
  shift
  if "$@" >/dev/null 2>&1; then
    nok "$desc" "expected failure, got success from: $*"
  else
    ok "$desc"
  fi
}

# --- file content predicates (for use with assert / refute) ---

# file_has <file> <ERE>       -> extended-regex match
file_has() { grep -Eq -- "$2" "$1"; }
# file_has_i <file> <ERE>     -> case-insensitive extended-regex match
file_has_i() { grep -Eiq -- "$2" "$1"; }
# file_has_str <file> <str>   -> fixed-string match
file_has_str() { grep -Fq -- "$2" "$1"; }

t_end() {
  printf '# %s: %d passed, %d failed\n' "$T_SUITE" "$T_PASS" "$T_FAIL"
  [ "$T_FAIL" -eq 0 ]
}
