//! `kgpacks-agent` — M1 placeholder agent (retained for not-yet-wired siblings).
//!
//! This is the original M1 scaffold [`Agent`]: a templated answer that echoes
//! the supplied retrieval context. It is retained unchanged so the not-yet-wired
//! sibling crates (`kgpacks-eval`, `kgpacks-ingestion`, `kgpacks-backend`, and
//! `kgpacks-query`'s `legacy::Retriever`) keep compiling.
//!
//! New code should use the real [`crate::CopilotAgent`] (graph-RAG synthesis
//! over the Copilot SDK via the injectable transport seam).

/// A graph-RAG agent that answers questions grounded in retrieved context
/// (M1 placeholder; superseded for real synthesis by [`crate::CopilotAgent`]).
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

    /// Placeholder answer (M1): echoes the model and supplied retrieval context.
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
