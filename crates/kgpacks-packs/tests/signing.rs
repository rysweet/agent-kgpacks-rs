//! Integration tests for WS7 (issue #22) — Ed25519 signing + verification of the
//! release index, exercised through the public `kgpacks_packs` API boundary.

use kgpacks_packs::{
    parse_trusted_public_key, signature_plan, trusted_release_public_key, validate_signature_flags,
    verify_pack_index_signature, PackIndexSignature, SignatureInputs, SignaturePlan,
    SigningKeyPair, PUBLIC_KEY_LEN, SIGNATURE_ALGORITHM, SIGNATURE_LEN,
};

/// A representative serialized release index (raw bytes, schema-agnostic).
const INDEX: &[u8] =
    br#"{"name":"cve-2025.06","format":"tar.gz-multipart-v1","version":"2025.6.0","total_bytes":123}"#;

#[test]
fn keypair_generation_yields_valid_distinct_keypairs() {
    let a = SigningKeyPair::generate().expect("keypair a");
    let b = SigningKeyPair::generate().expect("keypair b");
    assert_eq!(a.public_key_bytes().len(), PUBLIC_KEY_LEN);
    assert_ne!(a.public_key_bytes(), b.public_key_bytes());
    // A freshly generated keypair round-trips a signature over the index.
    let sig = a.sign(INDEX);
    assert_eq!(sig.len(), SIGNATURE_LEN);
    assert!(verify_pack_index_signature(
        INDEX,
        &sig,
        &a.public_key_bytes()
    ));
}

#[test]
fn sign_verify_round_trip_through_the_sidecar() {
    let signer = SigningKeyPair::generate().expect("keypair");
    let sidecar = signer.sign_index(INDEX);
    assert_eq!(sidecar.algorithm, SIGNATURE_ALGORITHM);

    // Persist + reload the sidecar exactly as `pack pull` would.
    let json = sidecar.to_json_string().expect("serialize sidecar");
    let reloaded = PackIndexSignature::from_json_str(&json).expect("parse sidecar");
    assert_eq!(reloaded, sidecar);

    let sig = reloaded.signature_array().expect("64-byte signature");
    assert!(verify_pack_index_signature(
        INDEX,
        &sig,
        &signer.public_key_bytes()
    ));
}

#[test]
fn tamper_detection_fails_verification() {
    let signer = SigningKeyPair::generate().expect("keypair");
    let sig = signer.sign(INDEX);
    let key = signer.public_key_bytes();

    // Any single-byte mutation of the index must fail verification.
    for i in 0..INDEX.len() {
        let mut tampered = INDEX.to_vec();
        tampered[i] ^= 0xFF;
        assert!(
            !verify_pack_index_signature(&tampered, &sig, &key),
            "tamper at byte {i} was not detected"
        );
    }
    // The pristine bytes still verify.
    assert!(verify_pack_index_signature(INDEX, &sig, &key));
}

#[test]
fn wrong_key_and_malformed_inputs_are_rejected() {
    let signer = SigningKeyPair::generate().expect("signer");
    let attacker = SigningKeyPair::generate().expect("attacker");
    let sig = signer.sign(INDEX);

    // Correct signature verified against an untrusted key → rejected.
    assert!(!verify_pack_index_signature(
        INDEX,
        &sig,
        &attacker.public_key_bytes()
    ));
    // Wrong-length signature / key → rejected, never panics.
    assert!(!verify_pack_index_signature(
        INDEX,
        &sig[..10],
        &signer.public_key_bytes()
    ));
    assert!(!verify_pack_index_signature(
        INDEX,
        &sig,
        &signer.public_key_bytes()[..8]
    ));
    // An attacker-signed sidecar does not verify against the signer's key.
    let attacker_sidecar = attacker.sign_index(INDEX);
    assert!(!verify_pack_index_signature(
        INDEX,
        &attacker_sidecar.signature_array().unwrap(),
        &signer.public_key_bytes()
    ));
}

#[test]
fn signature_plan_all_branches_and_flag_exclusion() {
    let plan = |present, valid, require, no_verify| {
        signature_plan(SignatureInputs {
            present,
            valid,
            require_signature: require,
            no_verify,
        })
    };
    assert_eq!(plan(true, true, false, false), SignaturePlan::Verify);
    assert_eq!(plan(true, false, false, false), SignaturePlan::Fail);
    assert_eq!(plan(false, false, false, false), SignaturePlan::Warn);
    assert_eq!(plan(false, false, true, false), SignaturePlan::Fail);
    assert_eq!(plan(true, true, false, true), SignaturePlan::Skip);

    // `--require-signature` + `--no-verify` is a usage error.
    assert!(validate_signature_flags(true, true).is_err());
    assert!(validate_signature_flags(true, false).is_ok());
    assert!(validate_signature_flags(false, true).is_ok());
}

#[test]
fn committed_trusted_key_matches_its_file_and_is_a_valid_point() {
    // The exported accessor and the raw parser agree, and the key is usable:
    // signing with a key OTHER than the trusted one must not verify against it.
    let trusted = trusted_release_public_key();
    let file = include_str!("../keys/pack-release-signing.pub");
    assert_eq!(parse_trusted_public_key(file).unwrap(), trusted);

    let random = SigningKeyPair::generate().unwrap();
    let sig = random.sign(INDEX);
    assert!(!verify_pack_index_signature(INDEX, &sig, &trusted));
}
