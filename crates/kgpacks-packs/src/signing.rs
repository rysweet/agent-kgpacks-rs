//! Ed25519 signing + verification of the `<name>.pack-release.json` release
//! index (WS7, issue #22).
//!
//! Ports the pack-signing half of the reference workstream as pure-Rust,
//! offline, fully-tested primitives. Signing operates on the **RAW serialized
//! bytes** of the release index, so it is deliberately *format-agnostic*: it
//! composes additively over the WS3 (#18) release-index schema whether or not
//! that schema has landed, because it never parses the index.
//!
//! The trust model is deliberately simple and fail-closed:
//!
//! * The signer holds an Ed25519 private key (a CI/release secret, never
//!   committed) and produces a **detached** signature over the raw index bytes.
//! * The signature travels next to the index as a `<name>.pack-release.json.sig`
//!   sidecar ([`PackIndexSignature`]).
//! * `pack pull` verifies the raw index bytes against a **trusted** public key —
//!   the one committed to the repo ([`trusted_release_public_key`]) — *before*
//!   parsing the index. Trust is anchored on that committed key, never on the
//!   `public_key` a sidecar happens to carry, so a signature from an untrusted
//!   key is rejected ([`verify_pack_index_signature`]).
//!
//! [`signature_plan`] is the pure pull-time policy: it maps the observed state
//! (`present`, `valid`) and the caller's flags (`require_signature`,
//! `no_verify`) onto a [`SignaturePlan`] the CLI acts on.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde_json::{Map, Value};

use crate::errors::{PacksError, Result};

/// Length in bytes of a raw Ed25519 public key.
pub const PUBLIC_KEY_LEN: usize = 32;

/// Length in bytes of a raw Ed25519 secret seed (RFC 8032).
pub const SECRET_SEED_LEN: usize = 32;

/// Length in bytes of an Ed25519 signature.
pub const SIGNATURE_LEN: usize = 64;

/// The signature algorithm identifier embedded in the sidecar envelope.
pub const SIGNATURE_ALGORITHM: &str = "ed25519";

/// The committed trusted-key file (base64 raw 32-byte public key + `#` header).
const TRUSTED_PUBLIC_KEY_FILE: &str = include_str!("../keys/pack-release-signing.pub");

/// The detached-signature sidecar filename for a pack's release index.
///
/// Mirrors [`crate::manifest::MANIFEST_FILENAME`]-style helpers: the index is
/// `<name>.pack-release.json`, so its signature is `<name>.pack-release.json.sig`.
pub fn pack_release_signature_filename(pack: &str) -> String {
    format!("{pack}.pack-release.json.sig")
}

/// An Ed25519 keypair that signs release indexes.
///
/// The secret seed is held in memory only. It is intentionally excluded from the
/// [`std::fmt::Debug`] output and must never be persisted to the repository —
/// the trusted *public* key is the only half that is committed.
#[derive(Clone)]
pub struct SigningKeyPair {
    signing_key: SigningKey,
}

impl SigningKeyPair {
    /// Generate a fresh keypair seeded from the operating-system CSPRNG.
    pub fn generate() -> Result<Self> {
        let mut seed = [0u8; SECRET_SEED_LEN];
        getrandom::getrandom(&mut seed)
            .map_err(|err| PacksError::Signature(format!("cannot read OS randomness: {err}")))?;
        Ok(Self::from_seed(&seed))
    }

    /// Construct from a 32-byte secret seed (RFC 8032 private key).
    pub fn from_seed(seed: &[u8; SECRET_SEED_LEN]) -> Self {
        Self {
            signing_key: SigningKey::from_bytes(seed),
        }
    }

    /// Construct from a base64 raw 32-byte secret seed.
    ///
    /// The **release/signing** counterpart to [`decode_public_key`]: the private
    /// seed is a CI/release secret supplied out-of-band (an env var or a key
    /// file), never committed. Surrounding whitespace is trimmed by
    /// [`decode_secret_seed`], so a seed read from a file with a trailing newline
    /// still loads. Fails — never panics — on non-base64 or wrong-length input.
    pub fn from_seed_base64(base64_seed: &str) -> Result<Self> {
        Ok(Self::from_seed(&decode_secret_seed(base64_seed)?))
    }

    /// The raw 32-byte public key.
    pub fn public_key_bytes(&self) -> [u8; PUBLIC_KEY_LEN] {
        self.signing_key.verifying_key().to_bytes()
    }

    /// The public key as standard base64 — the committed-key file format.
    pub fn public_key_base64(&self) -> String {
        BASE64.encode(self.public_key_bytes())
    }

    /// The raw 32-byte secret seed. Handle as a secret; never persist it.
    pub fn secret_seed_bytes(&self) -> [u8; SECRET_SEED_LEN] {
        self.signing_key.to_bytes()
    }

    /// The secret seed as standard base64 — the `--key-file` / env format a
    /// release signer supplies to `pack sign`. This is the private half: handle
    /// it as a secret and never persist it to the repository.
    pub fn secret_seed_base64(&self) -> String {
        BASE64.encode(self.secret_seed_bytes())
    }

    /// Produce a detached signature over the RAW release-index bytes.
    pub fn sign(&self, index_bytes: &[u8]) -> [u8; SIGNATURE_LEN] {
        self.signing_key.sign(index_bytes).to_bytes()
    }

    /// Build the [`PackIndexSignature`] sidecar for `index_bytes`.
    pub fn sign_index(&self, index_bytes: &[u8]) -> PackIndexSignature {
        PackIndexSignature {
            algorithm: SIGNATURE_ALGORITHM.to_string(),
            signature: self.sign(index_bytes).to_vec(),
            public_key: self.public_key_bytes().to_vec(),
        }
    }
}

impl std::fmt::Debug for SigningKeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never expose the secret seed.
        f.debug_struct("SigningKeyPair")
            .field("public_key_base64", &self.public_key_base64())
            .finish_non_exhaustive()
    }
}

/// Verify a detached signature over the RAW index bytes against a trusted key.
///
/// **Verify-before-parse**: this operates on the untrusted bytes exactly as
/// received and never interprets them. It returns `false` — never panics — for
/// every failure mode:
///
/// * a wrong-length signature or trusted-key slice,
/// * a structurally invalid public key,
/// * bytes that were tampered with after signing, or
/// * a signature produced by a key other than `trusted_pubkey`.
///
/// Uses `verify_strict` to reject the malleable/edge-case encodings that plain
/// `verify` would accept.
pub fn verify_pack_index_signature(index_bytes: &[u8], sig: &[u8], trusted_pubkey: &[u8]) -> bool {
    let Ok(sig_bytes) = <[u8; SIGNATURE_LEN]>::try_from(sig) else {
        return false;
    };
    let Ok(key_bytes) = <[u8; PUBLIC_KEY_LEN]>::try_from(trusted_pubkey) else {
        return false;
    };
    let Ok(verifying_key) = VerifyingKey::from_bytes(&key_bytes) else {
        return false;
    };
    let signature = Signature::from_bytes(&sig_bytes);
    verifying_key.verify_strict(index_bytes, &signature).is_ok()
}

/// A detached Ed25519 signature for a release index, stored beside it.
///
/// Serialized as the `<name>.pack-release.json.sig` JSON sidecar. The embedded
/// `public_key` is informational only (useful for key-rotation debugging);
/// verification trusts the caller-supplied trusted key, **not** this field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackIndexSignature {
    /// Signature algorithm identifier (always [`SIGNATURE_ALGORITHM`]).
    pub algorithm: String,
    /// The raw 64-byte Ed25519 signature.
    pub signature: Vec<u8>,
    /// The raw 32-byte public key of the signer (informational).
    pub public_key: Vec<u8>,
}

impl PackIndexSignature {
    /// Serialize to the canonical sidecar JSON object.
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert(
            "algorithm".to_string(),
            Value::String(self.algorithm.clone()),
        );
        map.insert(
            "signature".to_string(),
            Value::String(BASE64.encode(&self.signature)),
        );
        map.insert(
            "public_key".to_string(),
            Value::String(BASE64.encode(&self.public_key)),
        );
        Value::Object(map)
    }

    /// Serialize to a pretty JSON string for the on-disk sidecar.
    pub fn to_json_string(&self) -> Result<String> {
        serde_json::to_string_pretty(&self.to_value()).map_err(|err| {
            PacksError::Signature(format!("cannot serialize signature sidecar: {err}"))
        })
    }

    /// Parse and validate a sidecar from its JSON text.
    pub fn from_json_str(raw: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(raw).map_err(|err| {
            PacksError::Signature(format!("signature sidecar is not valid JSON: {err}"))
        })?;
        Self::from_value(&value)
    }

    /// Validate an arbitrary JSON value as a sidecar.
    ///
    /// Rejects a non-object, an unsupported algorithm, missing string fields, or
    /// base64 that does not decode to the exact Ed25519 lengths.
    pub fn from_value(value: &Value) -> Result<Self> {
        let obj = value.as_object().ok_or_else(|| {
            PacksError::Signature("signature sidecar must be a JSON object".to_string())
        })?;
        let algorithm = string_field(obj, "algorithm")?;
        if algorithm != SIGNATURE_ALGORITHM {
            return Err(PacksError::Signature(format!(
                "unsupported signature algorithm \"{algorithm}\" (expected \"{SIGNATURE_ALGORITHM}\")"
            )));
        }
        let signature = decode_fixed(&string_field(obj, "signature")?, SIGNATURE_LEN, "signature")?;
        let public_key = decode_fixed(
            &string_field(obj, "public_key")?,
            PUBLIC_KEY_LEN,
            "public_key",
        )?;
        Ok(Self {
            algorithm,
            signature,
            public_key,
        })
    }

    /// The signature as a fixed 64-byte array.
    pub fn signature_array(&self) -> Result<[u8; SIGNATURE_LEN]> {
        <[u8; SIGNATURE_LEN]>::try_from(self.signature.as_slice()).map_err(|_| {
            PacksError::Signature(format!(
                "signature must be {SIGNATURE_LEN} bytes, got {}",
                self.signature.len()
            ))
        })
    }
}

/// The observed signature state plus the caller's flags, fed to
/// [`signature_plan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignatureInputs {
    /// Whether a signature sidecar accompanies the index.
    pub present: bool,
    /// Whether that signature verified against the trusted key. Only meaningful
    /// when `present` is `true`.
    pub valid: bool,
    /// The `--require-signature` flag: an absent signature is then an error.
    pub require_signature: bool,
    /// The `--no-verify` flag: skip verification entirely.
    pub no_verify: bool,
}

/// The action `pack pull` takes for a given [`SignatureInputs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignaturePlan {
    /// A valid signature was verified; proceed on a trusted index.
    Verify,
    /// Fail closed: a present-but-invalid signature, or an absent signature
    /// under `--require-signature`.
    Fail,
    /// No signature, and none required: proceed on integrity (sha256) only.
    Warn,
    /// `--no-verify`: skip signature checking altogether.
    Skip,
}

/// Pull-time signature policy (pure).
///
/// Precedence, matching the issue spec:
/// * `--no-verify` wins over everything → [`SignaturePlan::Skip`].
/// * present + valid → [`SignaturePlan::Verify`].
/// * present + invalid → [`SignaturePlan::Fail`] (fail-closed).
/// * absent → [`SignaturePlan::Warn`], unless `--require-signature` →
///   [`SignaturePlan::Fail`].
///
/// The mutually-exclusive `--require-signature` + `--no-verify` combination is a
/// *usage* error caught earlier by [`validate_signature_flags`]; this function
/// stays total and simply honors `no_verify` if both are somehow set.
pub fn signature_plan(inputs: SignatureInputs) -> SignaturePlan {
    if inputs.no_verify {
        return SignaturePlan::Skip;
    }
    if inputs.present {
        if inputs.valid {
            SignaturePlan::Verify
        } else {
            SignaturePlan::Fail
        }
    } else if inputs.require_signature {
        SignaturePlan::Fail
    } else {
        SignaturePlan::Warn
    }
}

/// Reject the mutually-exclusive `--require-signature` + `--no-verify` pairing.
///
/// Requiring a signature while also asking to skip verification is contradictory,
/// so it is a usage error rather than a silently-resolved precedence.
pub fn validate_signature_flags(require_signature: bool, no_verify: bool) -> Result<()> {
    if require_signature && no_verify {
        return Err(PacksError::Signature(
            "`--require-signature` and `--no-verify` are mutually exclusive".to_string(),
        ));
    }
    Ok(())
}

/// Decode a base64 raw Ed25519 public key into its 32 bytes.
pub fn decode_public_key(base64_key: &str) -> Result<[u8; PUBLIC_KEY_LEN]> {
    let bytes = BASE64
        .decode(base64_key.trim())
        .map_err(|err| PacksError::Signature(format!("public key is not valid base64: {err}")))?;
    <[u8; PUBLIC_KEY_LEN]>::try_from(bytes.as_slice()).map_err(|_| {
        PacksError::Signature(format!(
            "public key must be {PUBLIC_KEY_LEN} bytes, got {}",
            bytes.len()
        ))
    })
}

/// Decode a base64 raw Ed25519 secret seed into its 32 bytes.
///
/// The **release/signing** counterpart to [`decode_public_key`]: this loads the
/// private seed a release signer supplies out-of-band (a CI secret, never
/// committed). Trims surrounding whitespace so a seed read from a file with a
/// trailing newline still decodes. Returns an error — never panics — on
/// non-base64 or wrong-length input, and never echoes the seed bytes.
pub fn decode_secret_seed(base64_seed: &str) -> Result<[u8; SECRET_SEED_LEN]> {
    let bytes = BASE64
        .decode(base64_seed.trim())
        .map_err(|err| PacksError::Signature(format!("secret seed is not valid base64: {err}")))?;
    <[u8; SECRET_SEED_LEN]>::try_from(bytes.as_slice()).map_err(|_| {
        PacksError::Signature(format!(
            "secret seed must be {SECRET_SEED_LEN} bytes, got {}",
            bytes.len()
        ))
    })
}

/// Parse a committed trusted-key file.
///
/// Skips `#` comment lines and blank lines, then decodes the first remaining
/// line as a base64 raw 32-byte Ed25519 public key.
pub fn parse_trusted_public_key(contents: &str) -> Result<[u8; PUBLIC_KEY_LEN]> {
    let line = contents
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .ok_or_else(|| {
            PacksError::Signature("trusted key file contains no key line".to_string())
        })?;
    decode_public_key(line)
}

/// The trusted release public key committed to the repository (raw 32 bytes).
///
/// Panics only if the committed `keys/pack-release-signing.pub` is malformed,
/// which is a build-time invariant guarded by a unit test.
pub fn trusted_release_public_key() -> [u8; PUBLIC_KEY_LEN] {
    parse_trusted_public_key(TRUSTED_PUBLIC_KEY_FILE)
        .expect("committed keys/pack-release-signing.pub must be a base64 Ed25519 public key")
}

fn string_field(obj: &Map<String, Value>, key: &str) -> Result<String> {
    obj.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            PacksError::Signature(format!("signature sidecar missing string field \"{key}\""))
        })
}

fn decode_fixed(base64_value: &str, expected: usize, field: &str) -> Result<Vec<u8>> {
    let bytes = BASE64.decode(base64_value).map_err(|err| {
        PacksError::Signature(format!(
            "sidecar field \"{field}\" is not valid base64: {err}"
        ))
    })?;
    if bytes.len() != expected {
        return Err(PacksError::Signature(format!(
            "sidecar field \"{field}\" must be {expected} bytes, got {}",
            bytes.len()
        )));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &[u8] = br#"{"name":"acme","format":"tar.gz-multipart-v1","version":"2025.6.0"}"#;

    fn fixed_pair() -> SigningKeyPair {
        SigningKeyPair::from_seed(&[7u8; SECRET_SEED_LEN])
    }

    #[test]
    fn generate_yields_distinct_keys() {
        let a = SigningKeyPair::generate().unwrap();
        let b = SigningKeyPair::generate().unwrap();
        assert_ne!(a.public_key_bytes(), b.public_key_bytes());
        assert_eq!(a.public_key_bytes().len(), PUBLIC_KEY_LEN);
    }

    #[test]
    fn from_seed_is_deterministic() {
        assert_eq!(
            fixed_pair().public_key_bytes(),
            fixed_pair().public_key_bytes()
        );
        assert_eq!(fixed_pair().sign(INDEX), fixed_pair().sign(INDEX));
    }

    #[test]
    fn sign_then_verify_round_trip() {
        let pair = fixed_pair();
        let sig = pair.sign(INDEX);
        assert!(verify_pack_index_signature(
            INDEX,
            &sig,
            &pair.public_key_bytes()
        ));
    }

    #[test]
    fn verify_rejects_tampered_index() {
        let pair = fixed_pair();
        let sig = pair.sign(INDEX);
        let mut tampered = INDEX.to_vec();
        // Flip one byte in the body.
        tampered[2] ^= 0x01;
        assert!(!verify_pack_index_signature(
            &tampered,
            &sig,
            &pair.public_key_bytes()
        ));
    }

    #[test]
    fn verify_rejects_untrusted_key() {
        let signer = fixed_pair();
        let sig = signer.sign(INDEX);
        let attacker = SigningKeyPair::from_seed(&[9u8; SECRET_SEED_LEN]);
        // Correct signature, but verified against the wrong (untrusted) key.
        assert!(!verify_pack_index_signature(
            INDEX,
            &sig,
            &attacker.public_key_bytes()
        ));
    }

    #[test]
    fn verify_rejects_malformed_lengths() {
        let pair = fixed_pair();
        let good = pair.sign(INDEX);
        let key = pair.public_key_bytes();
        // Short signature.
        assert!(!verify_pack_index_signature(INDEX, &good[..63], &key));
        // Short key.
        assert!(!verify_pack_index_signature(INDEX, &good, &key[..31]));
        // Empty inputs.
        assert!(!verify_pack_index_signature(INDEX, &[], &key));
        assert!(!verify_pack_index_signature(INDEX, &good, &[]));
    }

    #[test]
    fn sidecar_round_trips_through_json() {
        let pair = fixed_pair();
        let sidecar = pair.sign_index(INDEX);
        let json = sidecar.to_json_string().unwrap();
        let parsed = PackIndexSignature::from_json_str(&json).unwrap();
        assert_eq!(parsed, sidecar);
        assert_eq!(parsed.algorithm, SIGNATURE_ALGORITHM);
        // The parsed sidecar's signature verifies against the signer's key.
        assert!(verify_pack_index_signature(
            INDEX,
            &parsed.signature_array().unwrap(),
            &pair.public_key_bytes()
        ));
    }

    #[test]
    fn sidecar_rejects_bad_algorithm_and_lengths() {
        let bad_algo = serde_json::json!({
            "algorithm": "rsa",
            "signature": BASE64.encode([0u8; SIGNATURE_LEN]),
            "public_key": BASE64.encode([0u8; PUBLIC_KEY_LEN]),
        });
        assert!(PackIndexSignature::from_value(&bad_algo).is_err());

        let short_sig = serde_json::json!({
            "algorithm": SIGNATURE_ALGORITHM,
            "signature": BASE64.encode([0u8; 10]),
            "public_key": BASE64.encode([0u8; PUBLIC_KEY_LEN]),
        });
        assert!(PackIndexSignature::from_value(&short_sig).is_err());

        let not_object = serde_json::json!("nope");
        assert!(PackIndexSignature::from_value(&not_object).is_err());

        let missing = serde_json::json!({ "algorithm": SIGNATURE_ALGORITHM });
        assert!(PackIndexSignature::from_value(&missing).is_err());
    }

    #[test]
    fn signature_plan_covers_every_branch() {
        let plan = |present, valid, require, no_verify| {
            signature_plan(SignatureInputs {
                present,
                valid,
                require_signature: require,
                no_verify,
            })
        };
        // present + valid -> Verify.
        assert_eq!(plan(true, true, false, false), SignaturePlan::Verify);
        // present + invalid -> Fail (fail-closed), regardless of require.
        assert_eq!(plan(true, false, false, false), SignaturePlan::Fail);
        assert_eq!(plan(true, false, true, false), SignaturePlan::Fail);
        // absent -> Warn, unless required -> Fail.
        assert_eq!(plan(false, false, false, false), SignaturePlan::Warn);
        assert_eq!(plan(false, false, true, false), SignaturePlan::Fail);
        // --no-verify wins over everything -> Skip.
        assert_eq!(plan(true, true, false, true), SignaturePlan::Skip);
        assert_eq!(plan(true, false, false, true), SignaturePlan::Skip);
        assert_eq!(plan(false, false, true, true), SignaturePlan::Skip);
    }

    #[test]
    fn mutually_exclusive_flags_are_a_usage_error() {
        assert!(validate_signature_flags(true, true).is_err());
        assert!(validate_signature_flags(true, false).is_ok());
        assert!(validate_signature_flags(false, true).is_ok());
        assert!(validate_signature_flags(false, false).is_ok());
    }

    #[test]
    fn committed_trusted_key_is_well_formed() {
        let key = trusted_release_public_key();
        assert_eq!(key.len(), PUBLIC_KEY_LEN);
        // It must load as a real Ed25519 point.
        assert!(VerifyingKey::from_bytes(&key).is_ok());
    }

    #[test]
    fn parse_trusted_public_key_skips_comments() {
        let pair = fixed_pair();
        let file = format!("# header\n#  another\n\n{}\n", pair.public_key_base64());
        assert_eq!(
            parse_trusted_public_key(&file).unwrap(),
            pair.public_key_bytes()
        );
    }

    #[test]
    fn signature_filename_matches_index() {
        assert_eq!(
            pack_release_signature_filename("acme"),
            "acme.pack-release.json.sig"
        );
    }

    #[test]
    fn debug_does_not_leak_secret_seed() {
        let pair = SigningKeyPair::from_seed(&[42u8; SECRET_SEED_LEN]);
        let shown = format!("{pair:?}");
        assert!(shown.contains("public_key_base64"));
        // The base64 of the all-0x2a seed must not appear.
        assert!(!shown.contains(&BASE64.encode([42u8; SECRET_SEED_LEN])));
    }

    #[test]
    fn from_seed_base64_round_trips_a_signing_key() {
        let seed = [13u8; SECRET_SEED_LEN];
        let direct = SigningKeyPair::from_seed(&seed);
        // A base64 seed (as a release signer would supply) loads the same key.
        let loaded = SigningKeyPair::from_seed_base64(&BASE64.encode(seed)).unwrap();
        assert_eq!(loaded.public_key_bytes(), direct.public_key_bytes());
        // ...and a signature it produces verifies against that public key.
        let sig = loaded.sign(INDEX);
        assert!(verify_pack_index_signature(
            INDEX,
            &sig,
            &direct.public_key_bytes()
        ));
    }

    #[test]
    fn secret_seed_base64_round_trips_through_from_seed_base64() {
        let pair = fixed_pair();
        let exported = pair.secret_seed_base64();
        let reloaded = SigningKeyPair::from_seed_base64(&exported).unwrap();
        assert_eq!(reloaded.public_key_bytes(), pair.public_key_bytes());
        assert_eq!(reloaded.secret_seed_bytes(), pair.secret_seed_bytes());
    }

    #[test]
    fn from_seed_base64_tolerates_surrounding_whitespace() {
        let seed = [5u8; SECRET_SEED_LEN];
        // A seed read from a file often carries a trailing newline.
        let padded = format!("  {}\n", BASE64.encode(seed));
        let loaded = SigningKeyPair::from_seed_base64(&padded).unwrap();
        assert_eq!(
            loaded.public_key_bytes(),
            SigningKeyPair::from_seed(&seed).public_key_bytes()
        );
    }

    #[test]
    fn decode_secret_seed_rejects_bad_base64_and_lengths_without_leaking() {
        // Not base64 at all.
        assert!(decode_secret_seed("!!! not base64 !!!").is_err());
        // Valid base64 but the wrong length (16 bytes, not 32).
        let short = decode_secret_seed(&BASE64.encode([0u8; 16]));
        let err = short.unwrap_err().to_string();
        assert!(err.contains("32 bytes"), "err: {err}");
        assert!(err.contains("got 16"), "err: {err}");
        // The error text must never echo the (secret) seed bytes.
        let secret = BASE64.encode([7u8; 16]);
        let leaked = decode_secret_seed(&secret).unwrap_err().to_string();
        assert!(
            !leaked.contains(&secret),
            "seed leaked into error: {leaked}"
        );
    }

    #[test]
    fn a_signature_from_a_loaded_seed_matches_the_committed_trusted_key_only_for_that_seed() {
        // Signing with an arbitrary loaded seed produces a key that is NOT the
        // committed trusted key, so `pack pull` (which trusts the committed key)
        // would fail-closed — the invariant `pack sign` reports to the operator.
        let foreign =
            SigningKeyPair::from_seed_base64(&BASE64.encode([3u8; SECRET_SEED_LEN])).unwrap();
        assert_ne!(foreign.public_key_bytes(), trusted_release_public_key());
    }
}
