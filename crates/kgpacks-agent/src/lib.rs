//! `kgpacks-agent` — graph-RAG agent.
//!
//! Rust port of `@kgpacks/agent`. In production the agent talks to the GitHub
//! Copilot SDK through the RustyClawd integration (`rustyclawd-core` /
//! `rustyclawd-tools`); that wiring lands in M5. The M1 scaffold returns a
//! templated answer that echoes the supplied retrieval context.

/// A graph-RAG agent that answers questions grounded in retrieved context.
#[derive(Debug, Clone)]
pub struct Agent {
    model: String,
}

impl Agent {
    /// Construct an agent bound to a model identifier.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
        }
    }

    /// The configured model identifier.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Placeholder answer (M1). Replaced by a Copilot-SDK call in M5.
    pub fn answer(&self, question: &str, context: &str) -> String {
        format!("[{}] {question} | {context}", self.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echoes_context() {
        let a = Agent::new("stub");
        let out = a.answer("q", "nodes=0");
        assert!(out.contains("nodes=0"));
        assert_eq!(a.model(), "stub");
    }
}
