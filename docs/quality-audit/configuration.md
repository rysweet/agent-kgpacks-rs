# Configuration

The Ten-Wave Quality Audit is configured through environment variables, `gh`
authentication, Cargo features, and the `quality-audit` skill's tunables. This
page lists every configurable input and its default.

For the process contract, see the [Reference](reference.md).

## Environment

### NODE_OPTIONS

The audit runs under the operator's saved memory preference:

```bash
export NODE_OPTIONS=--max-old-space-size=32768
```

This is a saved preference. To change it, edit `~/.amplihack/config`
(`/home/azureuser/.amplihack/config`).

### GitHub authentication

The engineer reuses the existing least-privilege `gh` authentication. Required
scopes:

| Scope | Why |
| ----- | --- |
| `repo` | Open/update/close issues and PRs; merge. |
| `workflow` | Read CI status; touch `.github/workflows/` if a finding targets CI. |

```bash
gh auth status          # verify auth (does NOT print the token)
```

Never run `gh auth token` in this workflow — it prints the raw credential to
stdout, risking capture in logs or transcripts. `gh auth status` is sufficient to
confirm authentication.

Transient auth/rate-limit failures are retried once, then surfaced visibly (never
swallowed).

## Cargo features

| Feature | Default | Effect |
| ------- | ------- | ------ |
| _(none)_ | on | Fully offline, hermetic default gates (`fmt/build/test/clippy`). |
| `copilot` | off | Compiles + lints + smoke-tests the real RustyClawd / Copilot transport in `kgpacks-agent`. Fetches the SHA-pinned git dependency. Exercised in a dedicated CI step. |

The default gates never enable `copilot`, keeping them offline. The audit's
VALIDATE step mirrors CI:

```bash
cargo fmt --all -- --check
cargo build  --workspace --all-targets --locked
cargo test   --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
# copilot-feature parity (matches the dedicated CI step):
cargo clippy -p kgpacks-agent --all-targets --features copilot --locked -- -D warnings
cargo test   -p kgpacks-agent --features copilot --locked
```

## TypeScript reference clone

The M1–M5 parity source of truth is a shallow clone of the reference, made
**out of tree** and never committed:

```bash
TS_REF="$(mktemp -d)/agent-kgpacks-ts"
git clone --depth 1 https://github.com/rysweet/agent-kgpacks-ts "$TS_REF"
```

The clone is removed at closeout. Nothing under it is ever executed as a script.

## Git hygiene

The bootstrap housekeeping PR ensures agent/session artifacts are ignored:

```gitignore
# Session / runtime artifacts (never committed)
.claude/runtime/
```

Staging is always explicit (`git add <path>`), never `git add -A`.

## `quality-audit` skill tunables

The per-wave engine (`Skill(quality-audit)`) accepts the following inputs. For
this audit they are set to span the whole workspace and all six concerns.

| Input | Default | Audit setting | Description |
| ----- | ------- | ------------- | ----------- |
| `target_path` | `src/amplihack` | `crates/` | Directory to audit (the whole Cargo workspace). |
| `repo_path` | `.` | `.` | Repository root. |
| `min_cycles` | `3` | see note | Minimum internal cycles per invocation. |
| `max_cycles` | `6` | see note | Maximum internal cycles (safety valve). |
| `validation_threshold` | `2` | `2` | Validators (of 3) that must agree to confirm a finding. |
| `severity_threshold` | `medium` | `medium` | Minimum severity to report. |
| `module_loc_limit` | `300` | `300` | Flag modules exceeding this LOC. |
| `fix_all_per_cycle` | `true` | `true` | Fix ALL confirmed findings before completing a wave. |
| `categories` | (all) | (all) | The six concerns map onto all categories (see [Reference → Concern areas](reference.md#concern-areas)). |

> **Note on waves vs. cycles.** The audit runs **exactly ten** outer *waves*
> (`SEEK → VALIDATE → FIX`). The `min_cycles`/`max_cycles` inputs govern the
> `quality-audit` skill's *internal* iteration within a single wave invocation
> and are independent of the ten-wave count.

### Available categories

`security`, `reliability`, `dead_code`, `silent_fallbacks`, `error_swallowing`,
`result_dropping`, `shell_anti_patterns`, `silent_truncation`,
`async_anti_patterns`, `config_divergence`, `validation_gaps`,
`health_observability`, `retry_anti_patterns`, `structural`, `hardcoded_limits`,
`test_gaps`, `doc_gaps`, `documentation`.

### Core settings (environment)

| Variable | Default | Description |
| -------- | ------- | ----------- |
| `AUDIT_PARALLEL_LIMIT` | `8` | Max concurrent worktrees the skill may spawn. |
| `AUDIT_PR_SCAN_DAYS` | `30` | Days of recent PRs scanned for false-positive suppression. |
| `AUDIT_AUTO_CLOSE_THRESHOLD` | `90` | Confidence % to auto-close a matched issue. |
| `AUDIT_TAG_THRESHOLD` | `70` | Confidence % to tag a match for verification. |
| `AUDIT_ENABLE_VALIDATION` | `true` | Enable post-audit PR validation. |

## Labels

The audit uses a single GitHub label, `quality-audit`, applied to the umbrella
issue, every audit PR, and every parity-gap issue. It makes discovery and closeout
deterministic (label queries instead of brittle title-substring matches).

```bash
# Idempotent creation during bootstrap:
gh label create quality-audit \
  --description "Ten-Wave Quality Audit artifacts" --color 0E8A16 2>/dev/null || true
```

The ten wave PRs additionally use the title prefix `audit(wave <N>):`; the one-time
bootstrap/hygiene PR uses `audit(bootstrap):` (conceptually *wave 0*) so it is not
miscounted as a wave. Creating and applying the label needs only the existing
`repo` scope.

## Merge configuration

| Setting | Value |
| ------- | ----- |
| Merge method | `gh pr merge <n> --squash` |
| `--admin` | Only to bypass required-human-review, and only after CI-green **and** crusty-approved. Requires repo-admin rights **and** org acceptance of crusty's proxy approval as a review substitute; otherwise the merge waits for a human. |
| Approval authority | `crusty-old-engineer` explicit approval (Ryan's binding proxy). |

## CI reference

CI (`.github/workflows/ci.yml`) is left unchanged unless a finding targets it. It
runs `fmt → build → test → clippy -D warnings` plus the `copilot`-feature
build/lint/test, all under `--locked`, with every Action pinned to a commit SHA.
Expect ~22 minutes per run (the LadybugDB C++ engine builds from source).
