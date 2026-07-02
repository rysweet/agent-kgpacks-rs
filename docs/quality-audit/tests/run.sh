#!/usr/bin/env bash
# File: docs/quality-audit/tests/run.sh
#
# Runs the Ten-Wave Quality Audit contract & acceptance suite.
#
# Suites:
#   test_repo_hygiene.sh    (A3 / req 7)      offline  -- red until bootstrap PR
#   test_docs_contract.sh   (req 1-9, R1-R4)  offline  -- locks the doc contract
#   test_cargo_integrity.sh (A8 / req 6,7)    offline  -- Cargo.lock + CI gates
#   test_audit_closeout.sh  (req 3,6,8,9)     live gh  -- red until audit is done
#
# Usage:
#   ./run.sh                 # run everything
#   ./run.sh --offline       # skip the live GitHub closeout suite
#   AUDIT_SKIP_LIVE=1 ./run.sh
#   ./run.sh test_docs_contract.sh   # run one suite
#
# Exit code: 0 only if every selected suite passes.

set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

OFFLINE=0
SELECT=()
for arg in "$@"; do
  case "$arg" in
    --offline) OFFLINE=1 ;;
    -h|--help) grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) SELECT+=("$arg") ;;
  esac
done

if [ "${#SELECT[@]}" -eq 0 ]; then
  SELECT=(test_repo_hygiene.sh test_docs_contract.sh test_cargo_integrity.sh test_audit_closeout.sh)
fi

total_suites=0
failed_suites=0

for suite in "${SELECT[@]}"; do
  path="$HERE/$suite"
  if [ ! -f "$path" ]; then
    printf '# MISSING SUITE: %s\n' "$suite"
    failed_suites=$((failed_suites + 1))
    total_suites=$((total_suites + 1))
    continue
  fi
  if [ "$suite" = "test_audit_closeout.sh" ] && [ "$OFFLINE" = "1" ]; then
    printf '# SKIP (offline): %s\n' "$suite"
    continue
  fi
  total_suites=$((total_suites + 1))
  if bash "$path"; then
    printf '# SUITE PASS: %s\n\n' "$suite"
  else
    printf '# SUITE FAIL: %s\n\n' "$suite"
    failed_suites=$((failed_suites + 1))
  fi
done

printf '# ================================\n'
printf '# suites run: %d, failed: %d\n' "$total_suites" "$failed_suites"
if [ "$failed_suites" -eq 0 ]; then
  printf '# RESULT: PASS\n'
  exit 0
fi
printf '# RESULT: FAIL (expected while the audit is in progress — this is the TDD contract)\n'
exit 1
