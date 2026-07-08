//! End-to-end tests for `kgpacks pack sign` — the release-side (producing) half
//! of WS7 (issue #22), driven through the real command surface. `pack sign`
//! writes the detached `.sig` sidecar that `pack pull` verifies; the headline
//! test proves the round-trip: a signature produced by `pack sign` is ACCEPTED by
//! `pack pull --require-signature`, so the signing path is reachable end to end
//! rather than only through the library.
//!
//! Key material is supplied via `--key-file` (a base64 raw 32-byte seed) so the
//! tests never touch the process-global `KGPACKS_RELEASE_SIGNING_KEY`; the two
//! tests that DO exercise that env var serialize on `ENV_LOCK` to stay
//! deterministic under cargo's parallel test threads.

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use kgpacks_cli::run;
use kgpacks_packs::{pack_release_signature_filename, PackIndexSignature, SigningKeyPair};
use tempfile::tempdir;

const INDEX_JSON: &str =
    r#"{"name":"acme","format":"tar.gz-multipart-v1","version":"2025.6.0","total_bytes":42}"#;

/// Serializes the two tests that read/clear the `KGPACKS_RELEASE_SIGNING_KEY`
/// process-global env var (env vars are shared across cargo's test threads).
static ENV_LOCK: Mutex<()> = Mutex::new(());

const SIGNING_KEY_ENV: &str = "KGPACKS_RELEASE_SIGNING_KEY";

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// Lay down the release index (no sidecar) in `dir`.
fn write_index(dir: &Path, pack: &str) {
    fs::write(dir.join(format!("{pack}.pack-release.json")), INDEX_JSON).unwrap();
}

/// Write a base64 signing seed to `<dir>/key.b64` and return its path string.
fn write_key_file(dir: &Path, signer: &SigningKeyPair) -> String {
    let path = dir.join("key.b64");
    // A trailing newline mirrors how a real key file is written; the loader trims.
    fs::write(&path, format!("{}\n", signer.secret_seed_base64())).unwrap();
    path.to_str().unwrap().to_string()
}

#[test]
fn sign_produces_a_sidecar_that_pull_verifies() {
    let dir = tempdir().unwrap();
    let signer = SigningKeyPair::generate().unwrap();
    let pub_b64 = signer.public_key_base64();
    write_index(dir.path(), "acme");
    let key_file = write_key_file(dir.path(), &signer);
    let packs = dir.path().to_str().unwrap();

    // Sign, trusting the signer's own key so `matches_trusted_key` is true.
    let signed = run(&args(&[
        "--packs-dir",
        packs,
        "pack",
        "sign",
        "acme",
        "--key-file",
        &key_file,
        "--trusted-key",
        &pub_b64,
    ]))
    .expect("signing an existing index should succeed");
    assert!(signed.contains("\"command\": \"sign\""), "output: {signed}");
    assert!(signed.contains("\"status\": \"ok\""), "output: {signed}");
    assert!(
        signed.contains("\"matches_trusted_key\": true"),
        "output: {signed}"
    );
    assert!(
        signed.contains(&format!("\"public_key\": \"{pub_b64}\"")),
        "output: {signed}"
    );

    // The sidecar now exists next to the index and is a valid envelope.
    let sig_path = dir.path().join(pack_release_signature_filename("acme"));
    assert!(sig_path.exists(), "sidecar was not written");
    let sidecar = PackIndexSignature::from_json_str(&fs::read_to_string(&sig_path).unwrap())
        .expect("sidecar must be a valid signature envelope");
    assert_eq!(sidecar.algorithm, "ed25519");

    // Round-trip: `pack pull --require-signature` ACCEPTS the produced signature.
    let pulled = run(&args(&[
        "--packs-dir",
        packs,
        "pack",
        "pull",
        "acme",
        "--require-signature",
        "--trusted-key",
        &pub_b64,
    ]))
    .expect("pull must accept the signature that sign produced");
    assert!(pulled.contains("\"verified\": true"), "output: {pulled}");
    assert!(
        pulled.contains("\"policy\": \"verify\""),
        "output: {pulled}"
    );
}

#[test]
fn sign_writes_sidecar_next_to_index_by_default() {
    let dir = tempdir().unwrap();
    let signer = SigningKeyPair::generate().unwrap();
    write_index(dir.path(), "acme");
    let key_file = write_key_file(dir.path(), &signer);

    let out = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "sign",
        "acme",
        "--key-file",
        &key_file,
    ]))
    .expect("sign should succeed");
    let sig_path = dir.path().join(pack_release_signature_filename("acme"));
    assert!(
        out.contains(sig_path.to_str().unwrap()),
        "signature_file should report the default sidecar path: {out}"
    );
    assert!(sig_path.exists());
}

#[test]
fn sign_honors_the_out_flag() {
    let dir = tempdir().unwrap();
    let signer = SigningKeyPair::generate().unwrap();
    write_index(dir.path(), "acme");
    let key_file = write_key_file(dir.path(), &signer);
    let out_path = dir.path().join("custom.sig");

    let out = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "sign",
        "acme",
        "--key-file",
        &key_file,
        "--out",
        out_path.to_str().unwrap(),
    ]))
    .expect("sign should succeed");
    assert!(
        out.contains(out_path.to_str().unwrap()),
        "signature_file should report the --out path: {out}"
    );
    assert!(out_path.exists(), "sidecar should be written to --out path");
    // The default sidecar location must NOT be written when --out is given.
    assert!(!dir
        .path()
        .join(pack_release_signature_filename("acme"))
        .exists());
}

#[test]
fn sign_reports_a_mismatch_against_the_default_trusted_key() {
    let dir = tempdir().unwrap();
    let signer = SigningKeyPair::generate().unwrap();
    write_index(dir.path(), "acme");
    let key_file = write_key_file(dir.path(), &signer);

    // No --trusted-key: compares against the committed default trusted key, which
    // a randomly generated signer will (essentially never) match.
    let out = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "sign",
        "acme",
        "--key-file",
        &key_file,
    ]))
    .expect("sign should still succeed even on a trusted-key mismatch");
    assert!(
        out.contains("\"matches_trusted_key\": false"),
        "a foreign signer must not match the committed trusted key: {out}"
    );
}

#[test]
fn sign_errors_when_the_release_index_is_missing() {
    let dir = tempdir().unwrap();
    let signer = SigningKeyPair::generate().unwrap();
    let key_file = write_key_file(dir.path(), &signer);

    let err = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "sign",
        "ghost",
        "--key-file",
        &key_file,
    ]))
    .expect_err("a missing index is an error");
    assert!(err.contains("release index not found"), "err: {err}");
}

#[test]
fn sign_rejects_a_malformed_trusted_key_before_writing_any_sidecar() {
    let dir = tempdir().unwrap();
    let signer = SigningKeyPair::generate().unwrap();
    write_index(dir.path(), "acme");
    let key_file = write_key_file(dir.path(), &signer);

    // A malformed --trusted-key must fail the command BEFORE it produces a
    // sidecar, so a writing command never leaves a side effect on a bad argument.
    let err = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "sign",
        "acme",
        "--key-file",
        &key_file,
        "--trusted-key",
        "!!! not base64 !!!",
    ]))
    .expect_err("a malformed --trusted-key must be rejected");
    assert!(
        err.contains("base64") || err.contains("public key"),
        "err: {err}"
    );
    assert!(
        !dir.path()
            .join(pack_release_signature_filename("acme"))
            .exists(),
        "no sidecar should be written when --trusted-key is invalid"
    );
}

#[test]
fn sign_errors_on_a_malformed_key_file_without_leaking_it() {
    let dir = tempdir().unwrap();
    write_index(dir.path(), "acme");
    // A valid-base64 but wrong-length (16-byte) seed.
    let bad_seed = "AAAAAAAAAAAAAAAAAAAAAA==";
    let key_path = dir.path().join("bad.b64");
    fs::write(&key_path, bad_seed).unwrap();

    let err = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "sign",
        "acme",
        "--key-file",
        key_path.to_str().unwrap(),
    ]))
    .expect_err("a wrong-length seed must be rejected");
    assert!(err.contains("32 bytes"), "err: {err}");
    // The error must not echo the (secret) seed material.
    assert!(!err.contains(bad_seed), "seed leaked into error: {err}");
}

#[test]
fn sign_errors_when_the_key_file_is_unreadable() {
    let dir = tempdir().unwrap();
    write_index(dir.path(), "acme");

    let err = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "sign",
        "acme",
        "--key-file",
        "/nonexistent/key.b64",
    ]))
    .expect_err("an unreadable key file is an error");
    assert!(err.contains("cannot read signing key file"), "err: {err}");
}

#[test]
fn sign_rejects_a_traversal_pack_name() {
    let dir = tempdir().unwrap();
    let signer = SigningKeyPair::generate().unwrap();
    let key_file = write_key_file(dir.path(), &signer);

    let err = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "sign",
        "../etc",
        "--key-file",
        &key_file,
    ]))
    .expect_err("path traversal must be rejected");
    assert!(err.contains("invalid pack name"), "err: {err}");
}

#[test]
fn sign_rejects_an_unknown_flag() {
    let dir = tempdir().unwrap();
    let signer = SigningKeyPair::generate().unwrap();
    write_index(dir.path(), "acme");
    let key_file = write_key_file(dir.path(), &signer);

    let err = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "sign",
        "acme",
        "--key-file",
        &key_file,
        "--bogus",
    ]))
    .expect_err("an unknown flag must be a hard error for a writing command");
    assert!(err.contains("unknown flag"), "err: {err}");
}

#[test]
fn sign_reads_the_key_from_the_env_var_when_no_key_file() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = tempdir().unwrap();
    let signer = SigningKeyPair::generate().unwrap();
    let pub_b64 = signer.public_key_base64();
    write_index(dir.path(), "acme");

    std::env::set_var(SIGNING_KEY_ENV, signer.secret_seed_base64());
    let out = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "sign",
        "acme",
        "--trusted-key",
        &pub_b64,
    ]));
    std::env::remove_var(SIGNING_KEY_ENV);

    let out = out.expect("sign should read the key from the env var");
    assert!(out.contains("\"status\": \"ok\""), "output: {out}");
    assert!(
        out.contains("\"matches_trusted_key\": true"),
        "output: {out}"
    );
}

#[test]
fn sign_errors_when_no_key_is_provided() {
    let _guard = ENV_LOCK.lock().unwrap();
    // Ensure the env var is absent so the "no signing key" path is exercised.
    std::env::remove_var(SIGNING_KEY_ENV);
    let dir = tempdir().unwrap();
    write_index(dir.path(), "acme");

    let err = run(&args(&[
        "--packs-dir",
        dir.path().to_str().unwrap(),
        "pack",
        "sign",
        "acme",
    ]))
    .expect_err("signing without any key source must fail");
    assert!(err.contains("no signing key"), "err: {err}");
}
