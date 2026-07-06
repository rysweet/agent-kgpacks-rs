// WS7 (issue #22) cross-implementation reference signer.
//
// Signs a pack release index with Node's BUILT-IN Ed25519 (an implementation
// entirely independent of the Rust `ed25519-dalek` used by `kgpacks`), so the
// harness proves real interoperability: if a Node-produced detached signature
// verifies under the Rust `pack pull`, both sides agree on RFC 8032 Ed25519 over
// the raw index bytes.
//
// Usage: node sign_oracle.mjs <dir> <pack>
//   Writes <dir>/<pack>.pack-release.json and <dir>/<pack>.pack-release.json.sig,
//   and prints ONLY the standard-base64 raw 32-byte public key on stdout (the
//   trusted key to hand to `kgpacks pack pull --trusted-key`).

import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { generateKeyPairSync, sign as edSign } from "node:crypto";

const [dir, pack] = process.argv.slice(2);
if (!dir || !pack) {
  console.error("usage: sign_oracle.mjs <dir> <pack>");
  process.exit(2);
}

// A canonical, schema-agnostic release index. WS7 signs the RAW bytes, so the
// exact WS3 schema is irrelevant here — any well-formed JSON works.
const index = JSON.stringify({
  name: pack,
  format: "tar.gz-multipart-v1",
  version: "2025.6.0",
  total_bytes: 42,
});
const indexBytes = Buffer.from(index, "utf8");
writeFileSync(join(dir, `${pack}.pack-release.json`), indexBytes);

const { publicKey, privateKey } = generateKeyPairSync("ed25519");
// For Ed25519 the digest algorithm MUST be null; returns a 64-byte detached sig.
const signature = edSign(null, indexBytes, privateKey);
// Extract the raw 32-byte public key via its JWK `x` (base64url) component.
const rawPub = Buffer.from(publicKey.export({ format: "jwk" }).x, "base64url");

const sidecar = {
  algorithm: "ed25519",
  signature: signature.toString("base64"),
  public_key: rawPub.toString("base64"),
};
writeFileSync(
  join(dir, `${pack}.pack-release.json.sig`),
  JSON.stringify(sidecar, null, 2),
);

process.stdout.write(rawPub.toString("base64"));
