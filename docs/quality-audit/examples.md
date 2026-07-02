# Examples & tutorials

Worked examples for the Ten-Wave Quality Audit. These illustrate the process
described in the [Reference](reference.md) and [Usage](usage.md) guides.

All GitHub artifacts shown (issue/PR numbers, dates) are illustrative.

## Tutorial 1 — Bootstrap the audit

Set up the audit once before the wave loop begins.

```bash
# 1. Memory preference + auth.
export NODE_OPTIONS=--max-old-space-size=32768
gh auth status

# 2. Create the quality-audit label (idempotent) for deterministic discovery.
gh label create quality-audit \
  --description "Ten-Wave Quality Audit artifacts" --color 0E8A16 2>/dev/null || true

# 3. Open the umbrella tracking issue (labeled quality-audit).
gh issue create \
  --title "Ten-Wave Quality Audit" \
  --label quality-audit \
  --body-file - <<'EOF'
Living ledger for the ten-wave quality audit of agent-kgpacks-rs.
Each wave gets one comment (SEEK scope, VALIDATE results, FIX links, Status).
Concerns: correctness, memory-safety, error-handling, idiomatic-rust,
test-coverage/quality, parity(M1–M5).
The one-time bootstrap below is "wave 0" and is NOT one of the ten waves.

- [ ] Wave 1
- [ ] Wave 2
- [ ] Wave 3
- [ ] Wave 4
- [ ] Wave 5
- [ ] Wave 6
- [ ] Wave 7
- [ ] Wave 8
- [ ] Wave 9
- [ ] Wave 10
EOF

# 4. Housekeeping PR: ignore session artifacts (bootstrap / "wave 0", not a wave).
git switch -c audit/bootstrap-gitignore
printf '\n# Session / runtime artifacts (never committed)\n.claude/runtime/\n' >> .gitignore
git add .gitignore
git commit -m "chore: ignore .claude/runtime session artifacts"
git push -u origin audit/bootstrap-gitignore
gh pr create --fill --label quality-audit \
  --title "audit(bootstrap): hygiene — ignore session artifacts"

# 5. Clone the TS reference out of tree (never committed).
TS_REF="$(mktemp -d)/agent-kgpacks-ts"
git clone --depth 1 https://github.com/rysweet/agent-kgpacks-ts "$TS_REF"

# 6. Baseline gates.
cargo fmt --all -- --check
cargo build  --workspace --all-targets --locked
cargo test   --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## Tutorial 2 — Run a single wave (with findings)

A wave that surfaces an error-handling finding and delivers a fix PR.

```bash
# SEEK — the quality-audit skill scans all six concerns across crates/.
#   Finding: kgpacks-ingestion swallows a fetch error and returns Ok(()).
#   Concern: error-handling. Severity: high. Confirmed 3/3.

# FIX — one PR for the error-handling concern.
git switch -c audit/wave-3-error-handling
# ...edit crates/kgpacks-ingestion/src/processor.rs to propagate the error...
cargo fmt --all
cargo build  --workspace --all-targets --locked
cargo test   --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
git add crates/kgpacks-ingestion/src/processor.rs
git commit -m "fix(ingestion): propagate fetch error instead of returning Ok(())"
git push -u origin audit/wave-3-error-handling

gh pr create --title "audit(wave 3): error-handling — propagate fetch errors" \
  --label quality-audit \
  --body-file - <<'EOF'
## Findings (wave 3, error-handling)
- `processor.rs::process_one` discarded a `fetch()` error and returned `Ok(())`,
  masking a failed article (silent fallback / error swallowing). Confirmed 3/3.

## Fix
- Propagate the error with context; add a regression test asserting the error
  surfaces instead of a silent success.

## VALIDATE
- fmt / build(--locked) / test(--locked) / clippy(-D warnings): all pass.
EOF
```

Then run the crusty loop (Tutorial 4) and, once approved and CI is green, merge:

```bash
gh pr merge <number> --squash
```

Finally, log the wave (Tutorial 3).

## Tutorial 3 — Log a wave to the umbrella issue

Every wave — including empty ones — gets one comment.

### With findings

```bash
gh issue comment <umbrella-issue> --body-file - <<'EOF'
## Wave 3 — 2026-07-02

### SEEK scope
- Concerns scanned: correctness, memory-safety, error-handling, idiomatic-rust,
  test-coverage, parity(M1–M5)
- Crates/areas: kgpacks-ingestion, kgpacks-query
- Depth: deeper

### VALIDATE
- fmt: pass
- build (--locked): pass
- test (--locked): pass
- clippy (-D warnings): pass
- Confirmed findings (≥2/3 agreement): 1

### FIX
- error-handling: PR #42 — propagate fetch error instead of Ok(())   [crusty: approved]

### Parity gaps (non-blocking)
- none

Status: COMPLETE
EOF
```

### Zero-finding wave

A wave with no required fixes is complete once `SEEK` + `VALIDATE` are logged. No
PR is opened and no churn is manufactured.

```bash
gh issue comment <umbrella-issue> --body-file - <<'EOF'
## Wave 7 — 2026-07-02

### SEEK scope
- Concerns scanned: correctness, memory-safety, error-handling, idiomatic-rust,
  test-coverage, parity(M1–M5)
- Crates/areas: full workspace (crates/*)
- Depth: deepest

### VALIDATE
- fmt: pass
- build (--locked): pass
- test (--locked): pass
- clippy (-D warnings): pass
- Confirmed findings (≥2/3 agreement): none

### FIX
- No findings — no fix required.

### Parity gaps (non-blocking)
- none

Status: COMPLETE
EOF
```

## Tutorial 4 — The crusty review loop

Crusty is Ryan's binding proxy. It reviews the **PR diff + description** and must
issue an **explicit approval** before merge.

**Round 1 — crusty raises issues:**

```text
Skill(crusty-old-engineer) on PR #42:

Short framing:
  This propagates the error but leaves the caller's retry loop unchanged.

Key risks / sharp edges:
  - The new error type is untyped; callers can't distinguish transient from fatal.
  - No test asserts the error is surfaced — only that Ok is gone.

Recommended approach:
  Return a typed error. Add a test that asserts the specific error variant.

Verdict: NOT satisfied. Address the above and re-run.
```

**Address every issue, re-push, re-invoke:**

```bash
# ...add typed error variant + assertion test...
cargo test --workspace --locked
git commit -am "fix(ingestion): typed fetch error + assertion test"
git push
```

**Round 2 — crusty approves:**

```text
Skill(crusty-old-engineer) on PR #42:

The error is now typed and the test asserts the variant. Callers can branch on
transient vs fatal. No remaining concerns.

Verdict: SATISFIED. Approved.
```

Record the approval statement in the PR, confirm CI is green, then merge.

## Tutorial 5 — Close a PR with a rationale

If crusty cannot be satisfied (or the finding is withdrawn/superseded), the PR's
terminal state is **closed with a written rationale** — never merged.

```bash
gh pr close <number> --comment \
"Closing per crusty proxy review. The proposed refactor of the retriever cache
introduced a lifetime that would force an API break for marginal benefit; crusty
was not satisfied that the trade-off is warranted. The underlying finding is
downgraded to informational and tracked in #55 for a future milestone. No merge."
```

## Tutorial 6 — File a parity-gap issue (non-blocking)

Stub-crate parity gaps (`mcp` / `backend` / `eval`) are tracked, not merged.

```bash
gh issue create \
  --title "Parity gap: kgpacks-mcp does not yet expose pack queries (M5 follow-up)" \
  --label quality-audit \
  --body-file - <<'EOF'
Non-blocking parity gap surfaced by the ten-wave audit.

Reference: agent-kgpacks-ts `@kgpacks/mcp` exposes pack queries over MCP.
Rust: `kgpacks-mcp` is an intentional stub (documented M5 follow-up).

This is tracked, not a PR-blocking regression. Linked from the umbrella issue.
EOF
# Then link it from the umbrella issue's current wave comment.
```

## Tutorial 7 — Closeout

After wave 10, verify the audit is done.

```bash
# Every wave logged COMPLETE?
gh issue view <umbrella-issue> --comments | grep -c "Status: COMPLETE"   # expect 10

# Every audit PR terminal (merged or closed)? Query the label, not the title.
gh pr list --label quality-audit --state all

# Worktree clean; remove the out-of-tree TS clone.
git status --porcelain          # expect empty
rm -rf "$(dirname "$TS_REF")"
```

The audit is complete when all ten wave comments read `Status: COMPLETE` and
every audit PR is merged or closed with a rationale.
