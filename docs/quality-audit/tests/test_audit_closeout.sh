#!/usr/bin/env bash
# File: docs/quality-audit/tests/test_audit_closeout.sh
#
# Acceptance gate: the audit is DONE only when the live GitHub state proves it
# (requirements 3, 6, 8, 9). This is the executable definition of "done".
#
# TDD status: this suite FAILS until the audit engineer has:
#   - created the labeled umbrella issue,
#   - logged exactly ten `Status: COMPLETE` wave comments,
#   - driven every `quality-audit` PR to a terminal state (merged/closed),
#   - merged the bootstrap ("wave 0") hygiene PR,
# at which point it PASSES. Read-only; needs an authenticated `gh`.
#
# Requires network + `gh`. Skip locally with:  AUDIT_SKIP_LIVE=1

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
. "$HERE/lib.sh"

t_begin "audit closeout (live GitHub / req 3,6,8,9)"

if [ "${AUDIT_SKIP_LIVE:-0}" = "1" ]; then
  nok "live GitHub closeout verified" "skipped via AUDIT_SKIP_LIVE=1 (cannot prove done)"
  t_end
  exit $?
fi

if ! command -v gh >/dev/null 2>&1; then
  nok "gh CLI available" "gh not installed — cannot verify completion"
  t_end
  exit $?
fi

if ! gh auth status >/dev/null 2>&1; then
  nok "gh authenticated" "not logged in — cannot verify completion"
  t_end
  exit $?
fi
ok "gh authenticated"

REPO="$(gh repo view --json nameWithOwner -q .nameWithOwner 2>/dev/null || echo rysweet/agent-kgpacks-rs)"
printf '# repo under test: %s\n' "$REPO"

# --- Umbrella issue exists and is labeled ---
UNUM="$(gh issue list --repo "$REPO" --label quality-audit --state all --limit 1000 \
  --search 'Ten-Wave Quality Audit in:title' --json number -q '.[0].number' 2>/dev/null || true)"
if [ -n "$UNUM" ]; then
  ok "umbrella issue exists and is labeled quality-audit (#$UNUM)"
else
  nok "umbrella issue exists and is labeled quality-audit" \
    "no open/closed issue titled 'Ten-Wave Quality Audit' with label quality-audit"
  t_end
  exit $?
fi

# --- Exactly ten Status: COMPLETE wave comments ---
COMPLETE="$(gh issue view "$UNUM" --repo "$REPO" --json comments \
  -q '.comments[].body' 2>/dev/null | grep -c 'Status: COMPLETE' || true)"
COMPLETE="${COMPLETE:-0}"
if [ "$COMPLETE" -eq 10 ]; then
  ok "exactly ten waves logged Status: COMPLETE (found $COMPLETE)"
else
  nok "exactly ten waves logged Status: COMPLETE" "found $COMPLETE of 10"
fi

# --- No audit PR left open ---
OPEN="$(gh pr list --repo "$REPO" --label quality-audit --state open --limit 1000 \
  --json number -q 'length' 2>/dev/null || echo ERR)"
if [ "$OPEN" = "0" ]; then
  ok "no quality-audit PRs remain open"
else
  nok "no quality-audit PRs remain open" "open count: $OPEN (audit still in progress or gh error)"
fi

# --- Every audit PR reached a terminal state (MERGED or CLOSED) ---
NONTERMINAL="$(gh pr list --repo "$REPO" --label quality-audit --state all --limit 1000 \
  --json number,state -q '[.[] | select(.state != "MERGED" and .state != "CLOSED")] | length' \
  2>/dev/null || echo ERR)"
if [ "$NONTERMINAL" = "0" ]; then
  ok "every quality-audit PR is terminal (merged or closed)"
else
  nok "every quality-audit PR is terminal (merged or closed)" \
    "non-terminal PR count: $NONTERMINAL"
fi

# --- Bootstrap ("wave 0") hygiene PR exists and is merged ---
BOOT_MERGED="$(gh pr list --repo "$REPO" --label quality-audit --state all --limit 1000 \
  --search 'audit(bootstrap) in:title' --json state,title \
  -q '[.[] | select(.state == "MERGED")] | length' 2>/dev/null || echo 0)"
if [ "${BOOT_MERGED:-0}" -ge 1 ]; then
  ok "bootstrap (wave 0) hygiene PR is merged"
else
  nok "bootstrap (wave 0) hygiene PR is merged" \
    "no MERGED PR titled 'audit(bootstrap): …' found"
fi

t_end
