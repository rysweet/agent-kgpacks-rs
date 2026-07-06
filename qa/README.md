# QA scenarios — `fetch-cve-corpus` CLI (issue #25)

[gadugi-test](https://www.npmjs.com/package/@gadugi/agentic-test) (`agentic-test`)
scenarios that exercise the built `kgpacks` binary's **deterministic, offline**
surfaces for the `fetch-cve-corpus` command — command discoverability and usage
documentation. They intentionally never trigger a real network download: the real
GitHub fetch path (release resolution, SSRF-guarded download, double-unzip, provenance)
is proven by the zero-I/O contract tests in
`crates/kgpacks-corpus/tests/cve_corpus.rs` with injected network/download/unzip seams,
and the argument-validation and feature-gate error paths are proven by the unit tests in
`crates/kgpacks-cli/src/lib.rs`.

## Run

```bash
# Build the default (offline) binary and put it on PATH.
cargo build -p kgpacks-cli
TARGET_DIR="$(cargo metadata --format-version 1 --no-deps \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')"
export PATH="$TARGET_DIR/debug:$PATH"

cd qa
gadugi-test validate -d scenarios --strict
gadugi-test run -d scenarios -c agentic-test.config.yaml
```

## Scenarios

| Scenario                       | Command                           | Asserts                                               |
| ------------------------------ | --------------------------------- | ----------------------------------------------------- |
| `fetch-cve-corpus-help-listed` | `kgpacks help`                    | Top-level help lists `fetch-cve-corpus`; exit 0.      |
| `fetch-cve-corpus-usage`       | `kgpacks fetch-cve-corpus --help` | Usage documents `--kind` + the `net` feature; exit 0. |
| `kgpacks-cli-smoke`            | `kgpacks version`                 | The binary runs and reports a version; exit 0.        |

Runtime output (`outputs/`, `reports/`, `logs/`) is git-ignored.
