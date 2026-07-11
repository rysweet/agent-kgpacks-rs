#!/usr/bin/env bash
set -euo pipefail

tmp=".qa-tmp/ws3-release-plan"
rm -rf "$tmp"
mkdir -p "$tmp/cve"

printf '%s\n' '{"name":"cve","version":"0.1.0","provenance":{"corpus":{"name":"cvelistV5","commit":"abc123","date":"2025-06-14"},"embedding":{"model":"Xenova/bge-base-en-v1.5","dimensions":768},"build":{"date":"2025-06-15T04:22:10Z","tool_version":"agent-kgpacks-rs@0.1.0"}}}' > "$tmp/cve/manifest.json"

cargo run --quiet --bin kgpacks -- --packs-dir "$tmp" pack release-plan cve --tag cve-2025.06
