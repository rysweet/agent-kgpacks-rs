//! Offline parity tests for the token/usage accountant (`src/usage.rs`),
//! mirroring the reference `packages/agent/test/usage.test.ts`.

use kgpacks_agent::{Usage, UsageSnapshot, UsageTracker};

fn usage(prompt: u64, completion: u64, reasoning: u64) -> Usage {
    Usage::new(prompt, completion, reasoning)
}

fn snapshot(
    prompt: u64,
    completion: u64,
    reasoning: u64,
    total: u64,
    requests: u64,
) -> UsageSnapshot {
    UsageSnapshot {
        prompt_tokens: prompt,
        completion_tokens: completion,
        reasoning_tokens: reasoning,
        total_tokens: total,
        request_count: requests,
    }
}

#[test]
fn starts_at_zero_with_no_requests() {
    let tracker = UsageTracker::new();
    assert_eq!(tracker.snapshot(), snapshot(0, 0, 0, 0, 0));
}

#[test]
fn records_a_single_call() {
    let mut tracker = UsageTracker::new();
    tracker.record(&usage(10, 20, 5));
    assert_eq!(tracker.snapshot(), snapshot(10, 20, 5, 35, 1));
}

#[test]
fn accumulates_across_multiple_calls() {
    let mut tracker = UsageTracker::new();
    tracker.record(&usage(10, 20, 0));
    tracker.record(&usage(3, 4, 1));
    tracker.record(&usage(100, 200, 0));
    assert_eq!(tracker.snapshot(), snapshot(113, 224, 1, 338, 3));
}

#[test]
fn snapshot_is_an_independent_copy() {
    let mut tracker = UsageTracker::new();
    tracker.record(&usage(1, 1, 0));

    // Snapshots are value types: mutating one cannot affect the tracker.
    let mut snap = tracker.snapshot();
    snap.prompt_tokens = 9999;
    snap.request_count = 9999;
    assert_eq!(snap.prompt_tokens, 9999);

    assert_eq!(tracker.snapshot(), snapshot(1, 1, 0, 2, 1));
}

#[test]
fn successive_snapshots_are_equal_values() {
    let mut tracker = UsageTracker::new();
    tracker.record(&usage(2, 2, 0));
    let a = tracker.snapshot();
    let b = tracker.snapshot();
    assert_eq!(a, b);
}
