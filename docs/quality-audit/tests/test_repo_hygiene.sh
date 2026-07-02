#!/usr/bin/env bash
# File: docs/quality-audit/tests/test_repo_hygiene.sh
#
# Contract: Repository hygiene (design decision A3, requirement 7).
#
# The bootstrap ("wave 0") housekeeping PR must add `.claude/runtime/` and other
# session/runtime artifacts to `.gitignore` so audit branches never stage them.
#
# TDD status: these assertions FAIL until the bootstrap hygiene PR lands, then
# PASS. This is the intended red->green transition for the bootstrap step.

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
. "$HERE/lib.sh"

REPO_ROOT="$(git -C "$HERE" rev-parse --show-toplevel)"
GITIGNORE="$REPO_ROOT/.gitignore"

t_begin "repo hygiene (A3 / req 7)"

assert ".gitignore exists" test -f "$GITIGNORE"

# The core requirement: .claude/runtime/ is git-ignored.
assert ".gitignore references .claude/runtime" \
  file_has_str "$GITIGNORE" ".claude/runtime/"

assert "git actually ignores .claude/runtime/ contents" \
  git -C "$REPO_ROOT" check-ignore -q .claude/runtime/sessions.jsonl

assert "git ignores the .claude/runtime/ directory path" \
  git -C "$REPO_ROOT" check-ignore -q .claude/runtime/

# Defensive invariants (should hold before AND after the hygiene PR).
tracked="$(git -C "$REPO_ROOT" ls-files -- .claude/runtime/ 2>/dev/null)"
if [ -z "$tracked" ]; then
  ok "no .claude/runtime/ files are tracked by git"
else
  nok "no .claude/runtime/ files are tracked by git" "tracked: $tracked"
fi

staged="$(git -C "$REPO_ROOT" diff --cached --name-only -- .claude/runtime/ 2>/dev/null)"
if [ -z "$staged" ]; then
  ok "no .claude/runtime/ files are staged"
else
  nok "no .claude/runtime/ files are staged" "staged: $staged"
fi

t_end
