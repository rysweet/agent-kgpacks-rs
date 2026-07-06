#!/usr/bin/env bash
# WS7 (issue #22) outside-in cross-implementation signature harness.
#
# Signs a pack release index with Node's built-in Ed25519 (an INDEPENDENT
# implementation), then verifies + pulls it with the Rust `kgpacks pack pull`,
# asserting every branch of the fail-closed signature policy end to end through
# the real command surface. Prints PULL_SIGNATURE_OK on full agreement; any
# divergence exits non-zero.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

ORACLE="$ROOT/qa/pull-signature/sign_oracle.mjs"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# Build the CLI once so each pull invocation is fast and deterministic.
cargo build -q -p kgpacks-cli

kg() { cargo run -q -p kgpacks-cli -- "$@"; }
fail() {
  echo "PULL_SIGNATURE_DIVERGENCE: $1" >&2
  exit 1
}
# setup <dir> -> writes a Node-signed index+sidecar into <dir>, echoes pubkey.
setup() {
  mkdir -p "$1"
  node "$ORACLE" "$1" acme
}
# expect_fail <label> <cmd...> — the command MUST exit non-zero.
expect_fail() {
  local label="$1"
  shift
  if "$@" >/dev/null 2>&1; then
    fail "$label: expected failure but the pull succeeded"
  fi
}

# 1) Valid Node signature verifies under the Rust verifier (interop).
D="$TMP/valid"
PUB="$(setup "$D")"
out="$(kg --packs-dir "$D" pack pull acme --trusted-key "$PUB")" || fail "valid pull errored"
echo "$out" | grep -q '"verified": true' || fail "valid signature not verified"
echo "$out" | grep -q '"policy": "verify"' || fail "valid policy was not verify"

# 2) A signature from an untrusted key is rejected (fail-closed).
D="$TMP/wrongkey"
setup "$D" >/dev/null
WRONG="$(setup "$TMP/wrongkey_ref")" # a DIFFERENT keypair
expect_fail "untrusted-key" kg --packs-dir "$D" pack pull acme --trusted-key "$WRONG"

# 3) Tampering with the index after signing is detected.
D="$TMP/tamper"
PUB="$(setup "$D")"
node -e 'const fs=require("fs");const p=process.argv[1];fs.writeFileSync(p,fs.readFileSync(p).toString().replace("42","999"))' \
  "$D/acme.pack-release.json"
expect_fail "tamper" kg --packs-dir "$D" pack pull acme --trusted-key "$PUB"

# 4) An unsigned index is allowed (integrity-only warn) without --require-signature.
D="$TMP/unsigned"
PUB="$(setup "$D")"
rm -f "$D/acme.pack-release.json.sig"
out="$(kg --packs-dir "$D" pack pull acme --trusted-key "$PUB")" || fail "unsigned warn pull errored"
echo "$out" | grep -q '"policy": "warn"' || fail "unsigned policy was not warn"
echo "$out" | grep -q '"present": false' || fail "unsigned should report present=false"

# 5) --require-signature rejects the same unsigned index.
expect_fail "require-unsigned" kg --packs-dir "$D" pack pull acme --require-signature --trusted-key "$PUB"

# 6) --no-verify skips verification even for the unsigned index.
out="$(kg --packs-dir "$D" pack pull acme --no-verify)" || fail "no-verify pull errored"
echo "$out" | grep -q '"policy": "skip"' || fail "no-verify policy was not skip"

# 7) --require-signature together with --no-verify is a usage error.
expect_fail "mutually-exclusive" kg --packs-dir "$D" pack pull acme --require-signature --no-verify

# 8) The committed DEFAULT trusted key rejects a foreign (Node) signature.
D="$TMP/defaultkey"
setup "$D" >/dev/null
expect_fail "default-key-foreign-sig" kg --packs-dir "$D" pack pull acme

echo "PULL_SIGNATURE_OK"
