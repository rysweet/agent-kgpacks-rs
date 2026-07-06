# Ten-Wave Quality Audit — contract & acceptance tests

Executable specification for the audit process. Written test-first: the suite is
intentionally **red** until the audit is implemented and completed, then turns
**green**. It is standalone (bash + `git`/`gh` + `python3`) and is **not** wired
into the Rust `cargo test` gate, so it never slows CI's LadybugDB build.

## Run

```bash
docs/quality-audit/tests/run.sh              # all suites
docs/quality-audit/tests/run.sh --offline    # skip the live GitHub gate
docs/quality-audit/tests/run.sh test_docs_contract.sh   # one suite
```

Exit code is `0` only when every selected suite passes.

## Suites

| Suite | Contract | Network | Initial state |
| ----- | -------- | ------- | ------------- |
| `test_repo_hygiene.sh` | A3 / req 7 — `.claude/runtime/` git-ignored | offline | **fails** until the bootstrap hygiene PR lands |
| `test_docs_contract.sh` | req 1–9 + revisions R1–R4 (label, `wave 0`, C8 note, `--admin`) | offline | passes once docs encode the contract; guards against drift |
| `test_cargo_integrity.sh` | A8 / req 6–7 — `Cargo.lock` coverage + CI `--locked` gates | offline | passes on a consistent lockfile + CI |
| `test_audit_closeout.sh` | req 3,6,8,9 — live "done" definition | needs `gh` | **fails** until all 10 waves are `Status: COMPLETE` and every `quality-audit` PR is terminal |

`check_links.py` (used by the docs suite) verifies every intra-repo Markdown link
and `#anchor` resolves, using GitHub's heading-slug algorithm.

## Red → green

- `test_repo_hygiene.sh` goes green when the `audit(bootstrap)` PR adds
  `.claude/runtime/` to `.gitignore`.
- `test_audit_closeout.sh` goes green when the umbrella issue shows ten
  `Status: COMPLETE` wave comments, the bootstrap PR is merged, and no
  `quality-audit` PR is left open or otherwise non-terminal.

See the [process reference](../reference.md) for the full contract these tests encode.
