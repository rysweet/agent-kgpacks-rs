#!/usr/bin/env bash
# File: docs/quality-audit/tests/test_docs_contract.sh
#
# Contract: the docs/quality-audit/ Diataxis set encodes the locked, unambiguous
# requirements (1-9) and the four architect revisions:
#   R1 quality-audit label / deterministic discovery
#   R2 bootstrap ("wave 0") disambiguation
#   R3 C8 intentional-omission note
#   R4 --admin self-merge governance
#
# Offline. These lock the documented process contract; a failure means the docs
# drifted from the specification (or a stale artifact reappeared).

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
. "$HERE/lib.sh"

D="$(cd "$HERE/.." && pwd)"        # docs/quality-audit
README="$D/README.md"
USAGE="$D/usage.md"
REF="$D/reference.md"
CFG="$D/configuration.md"
EX="$D/examples.md"
ALL=("$README" "$USAGE" "$REF" "$CFG" "$EX")

t_begin "docs contract (req 1-9 + R1-R4)"

# ---- Diataxis structure ----
for f in "$README" "$USAGE" "$REF" "$CFG" "$EX"; do
  assert "doc exists: ${f##*/}" test -f "$f"
done
assert "README maps to usage.md"         file_has_str "$README" "(usage.md)"
assert "README maps to reference.md"     file_has_str "$README" "(reference.md)"
assert "README maps to configuration.md" file_has_str "$README" "(configuration.md)"
assert "README maps to examples.md"      file_has_str "$README" "(examples.md)"

# ---- Req 1: one dedicated Audit Engineer sub-agent ----
assert "reference names the Audit Engineer sub-agent (C1)" \
  file_has_str "$REF" "Audit Engineer sub-agent"
assert "usage: single dedicated Audit Engineer" \
  file_has_i "$USAGE" "single dedicated .*Audit Engineer"

# ---- Req 2: exactly ten waves across six concerns ----
assert "reference states exactly ten waves" file_has_str "$REF" "Exactly **ten** waves run"
assert "README states exactly ten"          file_has_str "$README" "exactly ten"
assert "concern: correctness"    file_has_i "$REF" "correctness"
assert "concern: memory safety"  file_has_i "$REF" "memory safety"
assert "concern: error handling" file_has_i "$REF" "error handling"
assert "concern: idiomatic Rust" file_has_i "$REF" "idiomatic rust"
assert "concern: test coverage"  file_has_i "$REF" "test coverage"
assert "concern: feature parity" file_has_i "$REF" "feature parity"
assert "parity references M1"    file_has_str "$REF" "M1"
assert "parity references M5"    file_has_str "$REF" "M5"

# ---- Req 3: umbrella issue + no snapshot docs ----
assert "reference defines umbrella issue title" file_has_str "$REF" "Ten-Wave Quality Audit"
assert "reference defines per-wave Status: COMPLETE" file_has_str "$REF" "Status: COMPLETE"
assert "reference: no committed snapshot documents" file_has_str "$REF" "no committed snapshot"
assert "README design principle: No snapshot docs"  file_has_str "$README" "No snapshot docs"

# ---- Req 4: PRs grouped per concern ----
assert "reference: PRs grouped per concern" file_has_i "$REF" "per concern"

# ---- Req 5: crusty binding proxy, explicit approval, loop ----
assert "reference: crusty is Ryan's proxy" file_has_str "$REF" "Ryan"
assert "reference: explicit approval statement" file_has_str "$REF" "explicit approval statement"
assert "reference: absence of comments is not approval" file_has_str "$REF" "Absence of comments"

# ---- Req 6: merge gate = CI green AND crusty; squash ----
assert "reference: CI green gate" file_has_i "$REF" "CI green"
assert "reference: crusty-approved gate" file_has_i "$REF" "crusty-approved"
assert "reference: squash merge command" file_has_str "$REF" "gh pr merge"
assert "reference: --squash" file_has_str "$REF" "--squash"
assert "reference: never merge a crusty-dissatisfied PR" \
  file_has_str "$REF" "Never merge a PR crusty is not satisfied"
assert "README: no unapproved PR is ever merged" \
  file_has_str "$README" "is ever merged"

# ---- Req 7: hermeticity, --locked/Cargo.lock, worktree cleanliness ----
assert "reference: --locked mentioned" file_has_str "$REF" "--locked"
assert "reference: Cargo.lock shipped with manifest changes" file_has_str "$REF" "Cargo.lock"
assert "reference: .claude/runtime ignored" file_has_str "$REF" ".claude/runtime/"
assert "configuration: .claude/runtime ignored" file_has_str "$CFG" ".claude/runtime/"
assert "reference: TS reference cloned out of tree" file_has_i "$REF" "out of tree"

# ---- Req 8: terminal states ----
assert "reference: Terminal states section" file_has_str "$REF" "Terminal states"
assert "reference: closed with a written rationale" file_has_i "$REF" "written rationale"

# ---- Req 9: done criteria ----
assert "reference: Done criteria" file_has_str "$REF" "Done criteria"
assert "reference: done = all ten waves + terminal PRs" file_has_i "$REF" "all ten waves"

# ---- R1: quality-audit label / deterministic discovery ----
for f in "${ALL[@]}"; do
  assert "label mentioned in ${f##*/}" file_has_str "$f" "quality-audit"
done
assert "usage creates the label"  file_has_str "$USAGE" "gh label create quality-audit"
assert "examples creates the label" file_has_str "$EX" "gh label create quality-audit"
assert "configuration documents the label" file_has_str "$CFG" "gh label create quality-audit"
# Closeout discovery is by label, not by brittle title match, and must be
# bounded so audits with >30 PRs are not silently truncated (--limit).
assert "usage closeout queries by label with --limit" \
  file_has_str "$USAGE" "gh pr list --label quality-audit --state all --limit 1000"
assert "reference closeout queries by label with --limit" \
  file_has_str "$REF" "gh pr list --label quality-audit --state all --limit 1000"
assert "examples closeout queries by label with --limit" \
  file_has_str "$EX" "gh pr list --label quality-audit --state all --limit 1000"

# ---- R2: bootstrap ("wave 0") disambiguation ----
for f in "${ALL[@]}"; do
  assert "audit(bootstrap) prefix in ${f##*/}" file_has_str "$f" "audit(bootstrap)"
  assert "'wave 0' concept in ${f##*/}" file_has "$f" '[Ww]ave 0'
done
assert "reference: bootstrap not counted in ten-wave ledger" \
  file_has_str "$REF" "does **not** count toward the ten-wave"
assert "examples: bootstrap branch name" file_has_str "$EX" "audit/bootstrap-gitignore"
assert "examples: umbrella body excludes bootstrap from the ten" \
  file_has_i "$EX" "NOT one of the ten waves"
# No stale bootstrap-as-wave-1 artifacts anywhere.
for f in "${ALL[@]}"; do
  refute "no stale 'audit(wave 1): hygiene' in ${f##*/}" \
    file_has_str "$f" "audit(wave 1): hygiene"
  refute "no stale 'hygiene-gitignore' branch in ${f##*/}" \
    file_has_str "$f" "hygiene-gitignore"
done

# ---- R3: C8 intentional-omission note ----
assert "reference notes C8 is intentionally reserved" \
  file_has_str "$REF" "C8 is intentionally reserved"
refute "no C8 row in the components table" file_has "$REF" '\| C8 \|'
# Sanity: the components that DO exist are present.
for c in C1 C2 C3 C4 C5 C6 C7 C9; do
  assert "components table has $c" file_has "$REF" "\\| $c \\|"
done

# ---- R4: --admin self-merge governance ----
for pair in "reference:$REF" "usage:$USAGE" "configuration:$CFG" "README:$README"; do
  name=${pair%%:*}; file=${pair#*:}
  assert "$name mentions --admin" file_has_str "$file" "--admin"
  assert "$name qualifies --admin with repo-admin governance" \
    file_has_str "$file" "repo-admin"
done
assert "reference has an explicit --admin governance callout" \
  file_has_str "$REF" '`--admin` governance'

# ---- Cross-doc links + heading anchors resolve ----
if command -v python3 >/dev/null 2>&1; then
  if python3 "$HERE/check_links.py"; then
    ok "all intra-repo doc links and anchors resolve"
  else
    nok "all intra-repo doc links and anchors resolve" "see check_links.py output above"
  fi
else
  nok "all intra-repo doc links and anchors resolve" "python3 not available to run check_links.py"
fi

t_end
