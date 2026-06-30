//! Shared OFFLINE test helpers for the `kgpacks-agent` parity suite.
//!
//! A configurable in-memory [`Transport`] mock, mirroring the `makeMockTransport`
//! helper of the reference `packages/agent/test/copilot-agent.test.ts`. The suite
//! injects this so it runs fully offline: it never opens a real Copilot session
//! and needs no network or credentials.
//!
//! `mod common;` is included by several test binaries; not every binary uses
//! every helper, so dead-code warnings are suppressed crate-locally here.
#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use kgpacks_agent::{
    Transport, TransportError, TransportOpenConfig, TransportResponse, TransportSession, Usage,
};

/// What `send()` returns (or errors) for a given prompt/timeout.
pub type Responder = Box<dyn Fn(&str, Option<u64>) -> Result<TransportResponse, TransportError>>;

/// Shared, interior-mutable state observed by the test after the agent has taken
/// ownership of the transport.
pub struct MockState {
    open_calls: Cell<usize>,
    shutdown_calls: Cell<usize>,
    close_calls: Cell<usize>,
    sends: RefCell<Vec<(String, Option<u64>)>>,
    opened_models: RefCell<Vec<String>>,
    open_error: RefCell<Option<String>>,
    close_fail_once: Cell<bool>,
    responder: RefCell<Responder>,
}

/// Build a self-consistent per-call [`Usage`].
pub fn usage(prompt: u64, completion: u64, reasoning: u64) -> Usage {
    Usage::new(prompt, completion, reasoning)
}

/// A handle to a mock transport: hand [`transport`](Mock::transport) to the
/// agent, then assert against the retained shared state.
#[derive(Clone)]
pub struct Mock {
    state: Rc<MockState>,
}

impl Default for Mock {
    fn default() -> Self {
        Self::new()
    }
}

impl Mock {
    /// A mock whose `send()` returns empty content with zero usage by default.
    pub fn new() -> Self {
        Self {
            state: Rc::new(MockState {
                open_calls: Cell::new(0),
                shutdown_calls: Cell::new(0),
                close_calls: Cell::new(0),
                sends: RefCell::new(Vec::new()),
                opened_models: RefCell::new(Vec::new()),
                open_error: RefCell::new(None),
                close_fail_once: Cell::new(false),
                responder: RefCell::new(Box::new(|_, _| {
                    Ok(TransportResponse {
                        content: String::new(),
                        usage: usage(0, 0, 0),
                    })
                })),
            }),
        }
    }

    /// An owned [`Transport`] backed by this mock's shared state.
    pub fn transport(&self) -> Box<dyn Transport> {
        Box::new(MockTransport {
            state: Rc::clone(&self.state),
        })
    }

    /// Make every `send()` return `content` with `usage`.
    pub fn respond_with(&self, content: &str, usage: Usage) {
        let content = content.to_string();
        self.set_responder(move |_, _| {
            Ok(TransportResponse {
                content: content.clone(),
                usage,
            })
        });
    }

    /// Drive `send()` with an explicit responder closure.
    pub fn set_responder<F>(&self, f: F)
    where
        F: Fn(&str, Option<u64>) -> Result<TransportResponse, TransportError> + 'static,
    {
        *self.state.responder.borrow_mut() = Box::new(f);
    }

    /// Return a fixed sequence of responses across successive `send()` calls
    /// (the last entry is repeated once exhausted).
    pub fn respond_sequence(&self, responses: Vec<TransportResponse>) {
        let idx = Cell::new(0usize);
        self.set_responder(move |_, _| {
            let i = idx.get();
            let last = responses.len().saturating_sub(1);
            let chosen = responses[i.min(last)].clone();
            idx.set(i + 1);
            Ok(chosen)
        });
    }

    /// Make `open()` fail with `message` (to exercise start() failure/redaction).
    pub fn fail_open(&self, message: &str) {
        *self.state.open_error.borrow_mut() = Some(message.to_string());
    }

    /// Make the next `close()` fail once (to exercise stop() leak-safety).
    pub fn fail_close_once(&self) {
        self.state.close_fail_once.set(true);
    }

    /// Number of `open()` calls.
    pub fn open_calls(&self) -> usize {
        self.state.open_calls.get()
    }

    /// Number of `shutdown()` calls.
    pub fn shutdown_calls(&self) -> usize {
        self.state.shutdown_calls.get()
    }

    /// Number of `close()` calls.
    pub fn close_calls(&self) -> usize {
        self.state.close_calls.get()
    }

    /// Number of `send()` calls.
    pub fn send_count(&self) -> usize {
        self.state.sends.borrow().len()
    }

    /// All prompts forwarded to `send()`, in order.
    pub fn prompts(&self) -> Vec<String> {
        self.state
            .sends
            .borrow()
            .iter()
            .map(|(p, _)| p.clone())
            .collect()
    }

    /// The model ids passed to each `open()` call, in order.
    pub fn opened_models(&self) -> Vec<String> {
        self.state.opened_models.borrow().clone()
    }

    /// The timeout forwarded to the most recent `send()`.
    pub fn last_timeout(&self) -> Option<u64> {
        self.state.sends.borrow().last().and_then(|(_, t)| *t)
    }
}

struct MockTransport {
    state: Rc<MockState>,
}

impl Transport for MockTransport {
    fn open(
        &self,
        config: &TransportOpenConfig,
    ) -> Result<Box<dyn TransportSession>, TransportError> {
        self.state.open_calls.set(self.state.open_calls.get() + 1);
        self.state
            .opened_models
            .borrow_mut()
            .push(config.model.clone());
        if let Some(message) = self.state.open_error.borrow().clone() {
            return Err(TransportError::new(message));
        }
        Ok(Box::new(MockSession {
            state: Rc::clone(&self.state),
        }))
    }

    fn shutdown(&self) -> Result<(), TransportError> {
        self.state
            .shutdown_calls
            .set(self.state.shutdown_calls.get() + 1);
        Ok(())
    }
}

struct MockSession {
    state: Rc<MockState>,
}

impl TransportSession for MockSession {
    fn send(
        &self,
        prompt: &str,
        timeout_ms: Option<u64>,
    ) -> Result<TransportResponse, TransportError> {
        self.state
            .sends
            .borrow_mut()
            .push((prompt.to_string(), timeout_ms));
        (self.state.responder.borrow())(prompt, timeout_ms)
    }

    fn close(&self) -> Result<(), TransportError> {
        self.state.close_calls.set(self.state.close_calls.get() + 1);
        if self.state.close_fail_once.get() {
            self.state.close_fail_once.set(false);
            return Err(TransportError::new("disconnect boom"));
        }
        Ok(())
    }
}
