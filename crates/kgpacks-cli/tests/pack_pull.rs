//! End-to-end tests for `kgpacks pack pull` — the WS7 (issue #22) signature
//! verification path over a release index, driven through the real command
//! surface. No network, no transport: the index + `.sig` sidecar are laid down
//! on disk exactly as a fetched release would appear.

use std::fs;
use std::path::Path;

use kgpacks_cli::run;
use kgpacks_packs::{pack_release_signature_filename, SigningKeyPair};
use tempfile::tempdir;

const INDEX_JSON: &str =
    r#"{"name":"acme","format":"tar.gz-multipart-v1","version":"2025.6.0","total_bytes":42}"#;

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// Write the release index, and optionally a signature sidecar, into `dir`.
fn write_index(dir: &Path, pack: &str, sidecar: Option<&str>) {
    fs::write(dir.join(format!("{pack}.pack-release.json")), INDEX_JSON).unwrap();
    if let Some(sig_json) = sidecar {
        fs::write(dir.join(pack_release_signature_filename(pack)), sig_json).unwrap();
    }
}

#[test]
fn pull_verifies_a_valid_signature() {
    let dir = tempdir().unwrap();
    let signer = SigningKeyPair::generate().unwrap();
    let sidecar = signer
        .sign_index(INDEX_JSON.as_bytes())
        .to_json_string()
        .unwrap();
    write_index(dir.path(), "acme", Some(&sidecar));

    let out = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "pull",
        "acme",
        "--trusted-key",
        &signer.public_key_base64(),
    ]))
    .expect("valid signature should pull");
    assert!(out.contains("\"verified\": true"), "output: {out}");
    assert!(out.contains("\"policy\": \"verify\""), "output: {out}");
    assert!(out.contains("\"status\": \"ok\""));
}

#[test]
fn pull_fails_closed_on_a_tampered_index() {
    let dir = tempdir().unwrap();
    let signer = SigningKeyPair::generate().unwrap();
    // Sign the pristine bytes, then tamper the index on disk after signing.
    let sidecar = signer
        .sign_index(INDEX_JSON.as_bytes())
        .to_json_string()
        .unwrap();
    write_index(dir.path(), "acme", Some(&sidecar));
    fs::write(
        dir.path().join("acme.pack-release.json"),
        INDEX_JSON.replace("42", "999"),
    )
    .unwrap();

    let err = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "pull",
        "acme",
        "--trusted-key",
        &signer.public_key_base64(),
    ]))
    .expect_err("tampered index must fail");
    assert!(err.contains("verification failed"), "err: {err}");
}

#[test]
fn pull_rejects_a_signature_from_an_untrusted_key() {
    let dir = tempdir().unwrap();
    let signer = SigningKeyPair::generate().unwrap();
    let attacker = SigningKeyPair::generate().unwrap();
    let sidecar = signer
        .sign_index(INDEX_JSON.as_bytes())
        .to_json_string()
        .unwrap();
    write_index(dir.path(), "acme", Some(&sidecar));

    // Trust a DIFFERENT key than the one that signed → fail-closed.
    let err = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "pull",
        "acme",
        "--trusted-key",
        &attacker.public_key_base64(),
    ]))
    .expect_err("untrusted signer must fail");
    assert!(err.contains("verification failed"), "err: {err}");
}

#[test]
fn pull_warns_when_unsigned_and_not_required() {
    let dir = tempdir().unwrap();
    write_index(dir.path(), "acme", None);

    let out = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "pull",
        "acme",
    ]))
    .expect("unsigned pull is allowed without --require-signature");
    assert!(out.contains("\"present\": false"), "output: {out}");
    assert!(out.contains("\"policy\": \"warn\""), "output: {out}");
    assert!(out.contains("\"verified\": false"));
}

#[test]
fn pull_rejects_unsigned_when_signature_required() {
    let dir = tempdir().unwrap();
    write_index(dir.path(), "acme", None);

    let err = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "pull",
        "acme",
        "--require-signature",
    ]))
    .expect_err("--require-signature must reject an unsigned index");
    assert!(err.contains("unsigned"), "err: {err}");
    assert!(err.contains("--require-signature"), "err: {err}");
}

#[test]
fn pull_skips_verification_with_no_verify() {
    let dir = tempdir().unwrap();
    // No sidecar at all; --no-verify still pulls.
    write_index(dir.path(), "acme", None);

    let out = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "pull",
        "acme",
        "--no-verify",
    ]))
    .expect("--no-verify skips signature checking");
    assert!(out.contains("\"policy\": \"skip\""), "output: {out}");
    assert!(out.contains("\"status\": \"ok\""));
}

#[test]
fn pull_rejects_mutually_exclusive_flags() {
    let dir = tempdir().unwrap();
    write_index(dir.path(), "acme", None);

    let err = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "pull",
        "acme",
        "--require-signature",
        "--no-verify",
    ]))
    .expect_err("mutually exclusive flags are a usage error");
    assert!(err.contains("mutually exclusive"), "err: {err}");
}

#[test]
fn pull_errors_when_the_release_index_is_missing() {
    let dir = tempdir().unwrap();
    let err = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "pull",
        "ghost",
    ]))
    .expect_err("a missing index is an error");
    assert!(err.contains("release index not found"), "err: {err}");
}

#[test]
fn pull_rejects_a_malformed_sidecar_fail_closed() {
    let dir = tempdir().unwrap();
    // A present-but-garbage sidecar counts as invalid (not absent), so the
    // policy fails closed rather than silently downgrading to integrity-only.
    write_index(dir.path(), "acme", Some("not json"));

    let err = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "pull",
        "acme",
    ]))
    .expect_err("a malformed sidecar must fail closed");
    assert!(err.contains("verification failed"), "err: {err}");
}

#[test]
fn pull_rejects_a_traversal_pack_name() {
    let dir = tempdir().unwrap();
    let err = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "pull",
        "../etc",
    ]))
    .expect_err("path traversal must be rejected");
    assert!(err.contains("invalid pack name"), "err: {err}");
}
