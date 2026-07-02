# Ten-Wave Quality Audit

The **Ten-Wave Quality Audit** is a repeatable, agent-driven quality process for
`agent-kgpacks-rs`. A single dedicated **Audit Engineer** sub-agent runs the
[amplihack `quality-audit`](https://github.com/rysweet/amplihack) skill through
**exactly ten** `SEEK → VALIDATE → FIX` waves, groups the resulting fixes into
pull requests, and gates every merge behind a binding proxy review from the
[`crusty-old-engineer`](reference.md#c3--crusty-old-engineer-proxy-gate)
skill acting as operator **Ryan's** stand-in reviewer.

The audit is a *living* process: all evidence lives in GitHub (one umbrella
tracking issue plus PR descriptions and issues). It never commits point-in-time
"snapshot" reports into the repository.

## What it covers

Each wave performs a fresh `SEEK` across all six concern areas:

1. **Correctness** — logic errors, wrong results, edge-case handling.
2. **Memory safety** — `unsafe` usage, aliasing, lifetimes, leaks, drop order.
3. **Error handling** — fail-closed behavior, no silent fallbacks or swallowed
   errors, no dropped `Result`s.
4. **Idiomatic Rust** — clippy cleanliness, ownership ergonomics, trait design,
   API shape.
5. **Test coverage / quality** — real assertions, hermetic tests, gap coverage.
6. **Feature parity (M1–M5)** — behavioral equivalence with the TypeScript
   reference [`rysweet/agent-kgpacks-ts`](https://github.com/rysweet/agent-kgpacks-ts).

## Documentation map

| Document | Purpose |
| -------- | ------- |
| [Usage](usage.md) | How to launch the audit and follow it end to end. |
| [Reference](reference.md) | The process contract: components, wave protocol, umbrella-issue schema, PR conventions, crusty loop, merge gate, terminal states. |
| [Configuration](configuration.md) | Environment, tokens, feature flags, and tunables. |
| [Examples & tutorials](examples.md) | Worked walkthroughs: a full run, a single wave, a crusty review loop, a zero-finding wave, closing a PR with rationale, filing a parity-gap issue. |
| [Tests](tests/README.md) | Executable contract & acceptance suite for the process (red until the audit is complete). |

## Non-negotiable gates

A pull request produced by the audit is merged **only** when **both** of the
following are true:

1. **CI is green** — `fmt → build → test → clippy -D warnings` plus the
   `--features copilot` step, all with `--locked`.
2. **Crusty is explicitly satisfied** — `crusty-old-engineer`, acting as Ryan's
   proxy, issues an explicit approval statement for the PR diff and description.

No PR that crusty is not satisfied with is ever merged. A PR that cannot reach
approval is **closed with a written rationale** instead. Self-merge via `--admin`
requires repo-admin rights and organizational acceptance of crusty's proxy
approval as a review substitute; otherwise the merge simply waits for a human, and
the two gates above are unchanged.

## Design principles

- **No snapshot docs.** Findings live in the umbrella issue and PRs, not in
  committed markdown reports.
- **Deterministic discovery.** Every audit artifact (umbrella issue, PRs,
  parity-gap issues) carries the `quality-audit` label, so closeout is verified by
  label query — not brittle title matching. The one-time bootstrap PR is tagged
  `audit(bootstrap)` ("wave 0") and is not one of the ten waves.
- **Worktree cleanliness.** `.claude/runtime/` and session artifacts are
  git-ignored; the TypeScript reference is shallow-cloned **out of tree**.
- **`--locked` integrity.** Any manifest change ships the regenerated
  `Cargo.lock` in the same PR.
- **Hermetic gates.** The default CI gates stay fully offline; networked work
  (the real Copilot transport) is isolated behind the `copilot` feature.

See the [Reference](reference.md) for the full contract.
