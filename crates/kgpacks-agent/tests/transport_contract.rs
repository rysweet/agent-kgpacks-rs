//! Offline contract tests for the [`Transport`] seam, mirroring the reference
//! `packages/agent/test/transport.contract.test.ts`.
//!
//! The Copilot SDK is wrapped behind a narrow, injectable [`Transport`] so the
//! rest of the suite runs offline against a mock and never opens a real session.
//! This file pins the structural contract any `Transport` must honour
//! (demonstrated with the shared in-memory mock), and — with the `copilot`
//! feature — that constructing the real adapter is side-effect-free.

mod common;

use common::{usage, Mock};
use kgpacks_agent::TransportOpenConfig;

#[test]
fn open_resolves_to_a_session_exposing_send_and_close() {
    let mock = Mock::new();
    let transport = mock.transport();
    // A session value that satisfies the trait is returned; `send`/`close` are
    // exercised below.
    let _session = transport
        .open(&TransportOpenConfig {
            model: "pinned-model".into(),
            provider: None,
        })
        .unwrap();
    assert_eq!(mock.open_calls(), 1);
}

#[test]
fn send_resolves_with_content_plus_a_four_field_usage() {
    let mock = Mock::new();
    mock.respond_with("hello world", usage(12, 7, 0));
    let transport = mock.transport();

    let session = transport
        .open(&TransportOpenConfig {
            model: "pinned-model".into(),
            provider: None,
        })
        .unwrap();
    let res = session.send("a prompt", Some(5_000)).unwrap();

    assert_eq!(res.content, "hello world");
    assert_eq!(res.usage, usage(12, 7, 0));
    assert_eq!(res.usage.prompt_tokens, 12);
    assert_eq!(res.usage.completion_tokens, 7);
    assert_eq!(res.usage.reasoning_tokens, 0);
    assert_eq!(res.usage.total_tokens, 19);
}

#[test]
fn close_and_shutdown_resolve() {
    let mock = Mock::new();
    let transport = mock.transport();
    let session = transport
        .open(&TransportOpenConfig {
            model: "pinned-model".into(),
            provider: None,
        })
        .unwrap();
    assert!(session.close().is_ok());
    assert!(transport.shutdown().is_ok());
}

#[cfg(feature = "copilot")]
#[test]
fn real_copilot_transport_constructs_without_side_effects() {
    // Construction must be lazy: no client/subprocess until `open()`. So this is
    // safe to run offline with no Copilot auth.
    use kgpacks_agent::copilot_transport;
    let transport = copilot_transport();
    // It satisfies the Transport shape; we never call `open()` (which would need
    // credentials), only assert it is constructible and `shutdown()` is a no-op.
    let t: &dyn kgpacks_agent::Transport = &transport;
    assert!(t.shutdown().is_ok());
}
