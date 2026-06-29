//! Contract tests for the `kgpacks-packs` SemVer 2.0 helpers.
//!
//! Ports `packages/packs/test/versioning.test.ts`: parsing, precedence
//! (including prerelease rules and ignored build metadata), sorting, and
//! error-on-invalid behavior.

use std::cmp::Ordering;

use kgpacks_packs::{
    compare_versions, is_valid_semver, latest_version, parse_version, sort_versions, ParsedVersion,
};

fn ids(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn parse_version_decomposes_a_full_semver_string() {
    let parsed = parse_version("1.4.2-rc.1+build.9").expect("valid semver");
    assert_eq!(
        parsed,
        ParsedVersion {
            major: 1,
            minor: 4,
            patch: 2,
            prerelease: ids(&["rc", "1"]),
            build: ids(&["build", "9"]),
        }
    );
}

#[test]
fn parse_version_handles_a_plain_version() {
    assert_eq!(
        parse_version("0.0.0").expect("valid"),
        ParsedVersion {
            major: 0,
            minor: 0,
            patch: 0,
            prerelease: Vec::new(),
            build: Vec::new(),
        }
    );
}

#[test]
fn parse_version_errors_on_invalid_input() {
    for bad in [
        "1.0", "1", "v1.0.0", "1.0.0-", "1.0.0+", "01.0.0", "1.2.3.4", "abc", "",
    ] {
        assert!(
            parse_version(bad).is_err(),
            "expected {bad:?} to be invalid"
        );
    }
}

#[test]
fn is_valid_semver_accepts_and_rejects_per_grammar() {
    for good in [
        "1.0.0",
        "0.0.0",
        "1.2.3-rc.1",
        "1.0.0-alpha+001",
        "1.0.0+build",
        "10.20.30",
    ] {
        assert!(is_valid_semver(good), "expected {good:?} to be valid");
    }
    for bad in [
        "1.0", "v1.0.0", "1.0.0.0", "1.0.0-", "01.0.0", "1.0.0-01", "",
    ] {
        assert!(!is_valid_semver(bad), "expected {bad:?} to be invalid");
    }
}

#[test]
fn compare_versions_orders_by_numeric_core() {
    assert_eq!(compare_versions("1.0.0", "2.0.0").unwrap(), Ordering::Less);
    assert_eq!(
        compare_versions("2.0.0", "1.0.0").unwrap(),
        Ordering::Greater
    );
    assert_eq!(compare_versions("1.2.0", "1.10.0").unwrap(), Ordering::Less);
    assert_eq!(compare_versions("1.0.9", "1.0.10").unwrap(), Ordering::Less);
    assert_eq!(compare_versions("1.2.3", "1.2.3").unwrap(), Ordering::Equal);
}

#[test]
fn compare_versions_ranks_prerelease_below_release() {
    assert_eq!(
        compare_versions("1.0.0-rc.1", "1.0.0").unwrap(),
        Ordering::Less
    );
    assert_eq!(
        compare_versions("1.0.0", "1.0.0-rc.1").unwrap(),
        Ordering::Greater
    );
}

#[test]
fn compare_versions_applies_prerelease_precedence_rules() {
    // numeric identifiers compared numerically (not lexically)
    assert_eq!(
        compare_versions("1.0.0-rc.2", "1.0.0-rc.10").unwrap(),
        Ordering::Less
    );
    // alphanumeric compared lexically
    assert_eq!(
        compare_versions("1.0.0-alpha", "1.0.0-beta").unwrap(),
        Ordering::Less
    );
    // numeric identifiers have lower precedence than alphanumeric
    assert_eq!(
        compare_versions("1.0.0-1", "1.0.0-alpha").unwrap(),
        Ordering::Less
    );
    // a larger set of identifiers wins when all preceding ones are equal
    assert_eq!(
        compare_versions("1.0.0-alpha", "1.0.0-alpha.1").unwrap(),
        Ordering::Less
    );
}

#[test]
fn compare_versions_ignores_build_metadata() {
    assert_eq!(
        compare_versions("1.0.0+build.1", "1.0.0+build.2").unwrap(),
        Ordering::Equal
    );
    assert_eq!(
        compare_versions("1.0.0-rc.1+a", "1.0.0-rc.1+b").unwrap(),
        Ordering::Equal
    );
}

#[test]
fn sort_versions_returns_ascending_without_mutating_input() {
    let input = ["1.2.0", "1.0.0", "1.1.0-rc.1", "1.1.0"];
    let sorted = sort_versions(&input).expect("all valid");
    assert_eq!(sorted, ids(&["1.0.0", "1.1.0-rc.1", "1.1.0", "1.2.0"]));
    // input slice is untouched (Rust borrows immutably).
    assert_eq!(input, ["1.2.0", "1.0.0", "1.1.0-rc.1", "1.1.0"]);
}

#[test]
fn sort_versions_honors_canonical_prerelease_ordering_chain() {
    let chain = [
        "1.0.0-alpha",
        "1.0.0-alpha.1",
        "1.0.0-alpha.beta",
        "1.0.0-beta",
        "1.0.0-beta.2",
        "1.0.0-beta.11",
        "1.0.0-rc.1",
        "1.0.0",
    ];
    let mut shuffled: Vec<&str> = chain.to_vec();
    shuffled.reverse();
    assert_eq!(sort_versions(&shuffled).expect("valid"), ids(&chain));
}

#[test]
fn sort_versions_errors_on_an_invalid_element() {
    assert!(sort_versions(&["1.0.0", "nope"]).is_err());
}

#[test]
fn latest_version_returns_the_highest_precedence_version() {
    assert_eq!(
        latest_version(&["1.0.0", "1.2.0", "1.1.0"]).unwrap(),
        Some("1.2.0".to_string())
    );
    assert_eq!(
        latest_version(&["1.0.0", "1.0.0-rc.1"]).unwrap(),
        Some("1.0.0".to_string())
    );
}

#[test]
fn latest_version_returns_none_for_an_empty_list() {
    assert_eq!(latest_version(&[]).unwrap(), None);
}

#[test]
fn latest_version_errors_on_an_invalid_element() {
    assert!(latest_version(&["1.0.0", "bad"]).is_err());
}
