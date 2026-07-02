# Usage

This guide describes how the Ten-Wave Quality Audit runs end to end. The audit is
executed by a single dedicated **Audit Engineer** sub-agent; a human operator (or
an orchestrating agent) launches it once and then follows progress through GitHub.

For the process contract behind these steps, see the [Reference](reference.md).
For tunables, see [Configuration](configuration.md).

## Prerequisites

- A working checkout of `agent-kgpacks-rs` with a stable Rust toolchain
  (`rustup`, `rustfmt`, `clippy`) and a C++ toolchain + **CMake** (for the
  bundled LadybugDB engine). On Debian/Ubuntu:
  `sudo apt-get install -y cmake build-essential`.
- `gh` authenticated with `repo` + `workflow` scope for `rysweet/agent-kgpacks-rs`.
- Read access to the reference `rysweet/agent-kgpacks-ts`.
- `NODE_OPTIONS=--max-old-space-size=32768` exported (saved operator preference;
  see [Configuration](configuration.md#node_options)).
- The amplihack `quality-audit` and `crusty-old-engineer` skills available.

## Launching the audit

The audit is started by spawning the Audit Engineer sub-agent and handing it the
end-to-end mandate. The engineer owns everything from bootstrap to closeout.

```text
Spawn one Audit Engineer sub-agent that will:
  1. Bootstrap the audit (umbrella issue, .gitignore hygiene PR, TS reference clone,
     baseline gates).
  2. Run exactly 10 SEEK → VALIDATE → FIX waves via Skill(quality-audit).
  3. Group fixes into PRs per concern and drive each PR through the crusty loop.
  4. Merge only when CI is green AND crusty is explicitly satisfied.
  5. Close out when all 10 waves are logged and every PR is in a terminal state.
```

You do not micro-manage the engineer. You observe the umbrella issue and the PR
queue.

## The lifecycle

### 1. Bootstrap (once)

The engineer performs one-time setup:

- **Creates the `quality-audit` GitHub label** (idempotent) so every audit
  artifact is discoverable by label rather than by title substring:

  ```bash
  gh label create quality-audit \
    --description "Ten-Wave Quality Audit artifacts" --color 0E8A16 2>/dev/null || true
  ```
- **Opens the umbrella issue** titled `Ten-Wave Quality Audit`, labeled
  `quality-audit` (see
  [Reference → Umbrella issue](reference.md#umbrella-tracking-issue-c4)). This
  is the living ledger for all ten waves.
- **Opens a housekeeping PR** that adds `.claude/runtime/` and other session
  artifacts to `.gitignore` (counts under the correctness/hygiene concern). This
  bootstrap PR is titled `audit(bootstrap): …` (conceptually *wave 0*) and does
  **not** count toward the ten waves; it carries the `quality-audit` label.
- **Shallow-clones the TypeScript reference** out of tree:

  ```bash
  git clone --depth 1 https://github.com/rysweet/agent-kgpacks-ts \
    "$(mktemp -d)/agent-kgpacks-ts"
  ```

  The clone is never committed and lives outside the repository worktree.
- **Establishes a baseline** by running every gate:

  ```bash
  cargo fmt --all -- --check
  cargo build  --workspace --all-targets --locked
  cargo test   --workspace --locked
  cargo clippy --workspace --all-targets --locked -- -D warnings
  # Mirror CI's copilot-feature step so local pre-flight matches CI exactly:
  cargo clippy -p kgpacks-agent --all-targets --features copilot --locked -- -D warnings
  cargo test   -p kgpacks-agent --features copilot --locked
  ```

### 2. Wave loop (repeat 10 times)

For each wave `w` in `1..=10`:

1. **SEEK** — a fresh scan across all six concerns (correctness, memory safety,
   error handling, idiomatic Rust, test coverage/quality, and M1–M5 parity with
   the TS reference). Later waves prioritize residual and newly-surfaced issues.
2. **VALIDATE** — run the gates locally and confirm each finding with the
   `quality-audit` multi-agent validators (≥2/3 agreement). Log the wave to the
   umbrella issue.
3. **FIX → PR** — group confirmed findings **per concern** (small/coupled
   findings may share one PR) and open the PR (titled `audit(wave <N>): …`,
   labeled `quality-audit`). Manifest edits ship the regenerated `Cargo.lock` in
   the same PR. Pre-flight all gates locally before pushing.
4. **Crusty loop** — invoke `crusty-old-engineer` as Ryan's proxy on the PR,
   address **every** issue it raises, and re-invoke until it is **explicitly
   satisfied** (or close the PR with a written rationale).
5. **Merge gate** — once CI is green **and** crusty is satisfied, self-merge:

   ```bash
   gh pr merge <number> --squash            # add --admin only if required-human-review blocks
   ```

   `--admin` requires repo-admin rights and org acceptance of crusty's proxy
   approval as a review substitute; otherwise the merge waits for a human (the
   CI-green **and** crusty-approved gate is unchanged). See
   [Reference → Merge gate](reference.md#merge-gate).
6. **Parity-gap tracking** — file non-blocking issues for stub-crate gaps
   (`kgpacks-mcp`, `kgpacks-backend`, `kgpacks-eval`), label them `quality-audit`,
   and link them from the umbrella issue.
7. **Log `Status: COMPLETE`** for wave `w` on the umbrella issue.

A wave with **no required fixes** is complete as soon as its `SEEK` scope and
`VALIDATE` results are logged — no churn is manufactured to fill a wave.

### 3. Closeout (once)

The engineer verifies that:

- All ten waves are logged complete on the umbrella issue.
- Every audit PR reached a terminal state — *(merged + crusty-approved + CI
  green)* **or** *(closed + written rationale)*.
- The worktree is clean and the out-of-tree TS clone is removed.

## Following progress

- **Umbrella issue** — read the per-wave comments for `SEEK` scope, `VALIDATE`
  results, `FIX` PR links, and `Status`.
- **Pull requests** — each PR description carries its findings and the crusty
  approval statement.
- **Parity-gap issues** — linked from the umbrella issue; these are tracked, not
  merge-blocking.

## Verifying completion

The audit is **done** when every wave comment on the umbrella issue reads
`Status: COMPLETE` and every audit PR is either merged or closed with a rationale.

```bash
# List all audit PRs and their state (deterministic: query the label, not the title):
gh pr list --label quality-audit --state all

# Restrict to the ten waves only (excludes the audit(bootstrap) PR):
gh pr list --label quality-audit --search "audit(wave" --state all

# Open the umbrella issue:
gh issue list --label quality-audit --search "Ten-Wave Quality Audit in:title"
```
