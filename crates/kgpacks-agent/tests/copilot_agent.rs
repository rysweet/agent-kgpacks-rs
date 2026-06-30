//! Offline behavioral parity tests for `CopilotAgent` (`src/copilot_agent.rs`),
//! mirroring the reference `packages/agent/test/copilot-agent.test.ts`.
//!
//! Every test injects a MOCK transport, so the suite runs fully offline. Because
//! model output is non-deterministic in production, these assert STRUCTURAL
//! parity only — valid shapes, `Vec<String>` results, fence-stripping, citation
//! derivation, usage accounting, lifecycle, and fail-closed error behavior.

mod common;

use common::{usage, Mock};
use kgpacks_agent::{
    ContextChunk, CopilotAgent, CopilotAgentOptions, ExpandQueryOptions, MultiQueryOptions,
    ProviderConfig, SeedArticleRequest, SynthesisRequest, TransportResponse,
    DEFAULT_SYNTHESIS_MODEL,
};

fn started(mock: &Mock, options: CopilotAgentOptions) -> CopilotAgent {
    let mut agent = CopilotAgent::with_transport(mock.transport(), options);
    agent.start().unwrap();
    agent
}

fn synth(question: &str, context: Vec<ContextChunk>) -> SynthesisRequest {
    SynthesisRequest {
        question: question.into(),
        context,
        ..SynthesisRequest::default()
    }
}

// ── Lifecycle ───────────────────────────────────────────────────────────────

#[test]
fn construction_is_side_effect_free() {
    let mock = Mock::new();
    let _agent = CopilotAgent::with_transport(mock.transport(), CopilotAgentOptions::default());
    assert_eq!(mock.open_calls(), 0);
}

#[test]
fn start_opens_one_session_pinned_to_the_default_model() {
    let mock = Mock::new();
    let _agent = started(&mock, CopilotAgentOptions::default());
    assert_eq!(mock.open_calls(), 1);
    assert_eq!(mock.opened_models(), [DEFAULT_SYNTHESIS_MODEL]);
}

#[test]
fn start_is_idempotent() {
    let mock = Mock::new();
    let mut agent = started(&mock, CopilotAgentOptions::default());
    agent.start().unwrap();
    assert_eq!(mock.open_calls(), 1);
}

#[test]
fn start_forwards_a_model_override() {
    let mock = Mock::new();
    let _agent = started(
        &mock,
        CopilotAgentOptions {
            model: Some("custom-pinned-model".into()),
            ..CopilotAgentOptions::default()
        },
    );
    assert_eq!(mock.opened_models(), ["custom-pinned-model"]);
}

#[test]
fn stop_closes_the_session_and_shuts_the_transport_down() {
    let mock = Mock::new();
    let mut agent = started(&mock, CopilotAgentOptions::default());
    agent.stop().unwrap();
    assert_eq!(mock.close_calls(), 1);
    assert_eq!(mock.shutdown_calls(), 1);
}

#[test]
fn stop_still_shuts_down_when_close_fails() {
    let mock = Mock::new();
    mock.fail_close_once();
    let mut agent = started(&mock, CopilotAgentOptions::default());

    let err = agent.stop().unwrap_err();
    assert!(err.is_transport());
    // shutdown() MUST run even though close() failed, or the client leaks.
    assert_eq!(mock.shutdown_calls(), 1);
}

#[test]
fn stop_is_idempotent() {
    let mock = Mock::new();
    let mut agent = started(&mock, CopilotAgentOptions::default());
    agent.stop().unwrap();
    agent.stop().unwrap();
    assert_eq!(mock.close_calls(), 1);
    assert_eq!(mock.shutdown_calls(), 1);
}

#[test]
fn stop_is_safe_when_start_was_never_called() {
    let mock = Mock::new();
    let mut agent = CopilotAgent::with_transport(mock.transport(), CopilotAgentOptions::default());
    assert!(agent.stop().is_ok());
}

// ── Not-started guard ───────────────────────────────────────────────────────

#[test]
fn every_operation_rejects_with_not_started_before_start() {
    let mock = Mock::new();
    let agent = CopilotAgent::with_transport(mock.transport(), CopilotAgentOptions::default());

    assert!(agent
        .synthesize_answer(&synth("q", vec![]))
        .unwrap_err()
        .is_not_started());
    assert!(agent
        .expand_query("q", ExpandQueryOptions::default())
        .unwrap_err()
        .is_not_started());
    assert!(agent
        .multi_query("q", MultiQueryOptions::default())
        .unwrap_err()
        .is_not_started());
    assert!(agent
        .identify_seed_articles(&SeedArticleRequest {
            topic: "t".into(),
            candidates: vec!["a".into()],
            limit: None,
        })
        .unwrap_err()
        .is_not_started());
    assert_eq!(mock.send_count(), 0);
}

#[test]
fn operations_reject_after_stop() {
    let mock = Mock::new();
    let mut agent = started(&mock, CopilotAgentOptions::default());
    agent.stop().unwrap();
    assert!(agent
        .expand_query("q", ExpandQueryOptions::default())
        .unwrap_err()
        .is_not_started());
}

#[test]
fn get_usage_is_callable_before_start_and_reports_zeros() {
    let mock = Mock::new();
    let agent = CopilotAgent::with_transport(mock.transport(), CopilotAgentOptions::default());
    let snap = agent.get_usage();
    assert_eq!(snap.total_tokens, 0);
    assert_eq!(snap.request_count, 0);
}

// ── synthesizeAnswer ────────────────────────────────────────────────────────

#[test]
fn synthesize_returns_answer_metadata_and_usage() {
    let mock = Mock::new();
    mock.respond_with("Synthesized grounded answer.", usage(10, 20, 5));
    let agent = started(&mock, CopilotAgentOptions::default());

    let result = agent
        .synthesize_answer(&synth(
            "How does HNSW work?",
            vec![ContextChunk::new(
                "doc:1",
                "HNSW builds a navigable small-world graph.",
            )],
        ))
        .unwrap();

    assert_eq!(result.answer, "Synthesized grounded answer.");
    assert_eq!(result.metadata.model, DEFAULT_SYNTHESIS_MODEL);
    assert_eq!(result.usage, usage(10, 20, 5));
}

#[test]
fn synthesize_derives_cited_ids_in_first_appearance_order() {
    let mock = Mock::new();
    mock.respond_with(
        "According to doc:2 and also doc:1, HNSW is layered.",
        usage(1, 1, 0),
    );
    let agent = started(&mock, CopilotAgentOptions::default());

    let result = agent
        .synthesize_answer(&synth(
            "q",
            vec![
                ContextChunk::new("doc:1", "a"),
                ContextChunk::new("doc:2", "b"),
                ContextChunk::new("doc:3", "c"),
            ],
        ))
        .unwrap();

    assert_eq!(result.metadata.cited_ids, ["doc:2", "doc:1"]);
}

#[test]
fn synthesize_yields_empty_cited_ids_when_nothing_referenced() {
    let mock = Mock::new();
    mock.respond_with("An answer that cites nothing.", usage(1, 1, 0));
    let agent = started(&mock, CopilotAgentOptions::default());

    let result = agent
        .synthesize_answer(&synth("q", vec![ContextChunk::new("doc:1", "a")]))
        .unwrap();
    assert!(result.metadata.cited_ids.is_empty());
}

#[test]
fn synthesize_yields_empty_cited_ids_when_context_is_empty() {
    let mock = Mock::new();
    mock.respond_with("doc:1 mentioned but not in context", usage(1, 1, 0));
    let agent = started(&mock, CopilotAgentOptions::default());

    let result = agent.synthesize_answer(&synth("q", vec![])).unwrap();
    assert!(result.metadata.cited_ids.is_empty());
}

#[test]
fn synthesize_empty_context_default_instructs_refusal() {
    let mock = Mock::new();
    mock.respond_with("No grounding available.", usage(1, 1, 0));
    let agent = started(&mock, CopilotAgentOptions::default());

    agent
        .synthesize_answer(&synth("What is CVE-2025-0074?", vec![]))
        .unwrap();

    let prompt = &mock.prompts()[0];
    assert!(prompt.contains("lacks grounding"));
    assert!(!prompt.contains("from your own knowledge"));
}

#[test]
fn synthesize_closed_book_empty_context_asks_own_knowledge() {
    let mock = Mock::new();
    mock.respond_with("It is a use-after-free in Android.", usage(1, 1, 0));
    let agent = started(&mock, CopilotAgentOptions::default());

    let result = agent
        .synthesize_answer(&SynthesisRequest {
            question: "What is CVE-2025-0074?".into(),
            context: vec![],
            closed_book: true,
            ..SynthesisRequest::default()
        })
        .unwrap();

    let prompt = &mock.prompts()[0];
    assert!(prompt.contains("Answer the question from your own knowledge"));
    assert!(!prompt.contains("lacks grounding"));
    assert_eq!(result.answer, "It is a use-after-free in Android.");
}

#[test]
fn synthesize_does_not_match_an_id_as_a_prefix_of_a_longer_id() {
    let mock = Mock::new();
    mock.respond_with("Only Topic#10 is relevant here.", usage(1, 1, 0));
    let agent = started(&mock, CopilotAgentOptions::default());

    let result = agent
        .synthesize_answer(&synth(
            "q",
            vec![
                ContextChunk::new("Topic#1", "a"),
                ContextChunk::new("Topic#10", "b"),
            ],
        ))
        .unwrap();
    assert_eq!(result.metadata.cited_ids, ["Topic#10"]);
}

#[test]
fn synthesize_errors_on_empty_model_content() {
    let mock = Mock::new();
    mock.respond_with("", usage(1, 0, 0));
    let agent = started(&mock, CopilotAgentOptions::default());

    let err = agent.synthesize_answer(&synth("q", vec![])).unwrap_err();
    assert!(err.is_response_format());
}

// ── expandQuery / multiQuery ────────────────────────────────────────────────

#[test]
fn expand_query_returns_vec_from_fenced_json_array() {
    let mock = Mock::new();
    mock.respond_with(
        "```json\n[\"vector database parity\",\"embedding retrieval equivalence\"]\n```",
        usage(5, 5, 0),
    );
    let agent = started(&mock, CopilotAgentOptions::default());

    assert_eq!(
        agent
            .expand_query("vector db parity", ExpandQueryOptions::default())
            .unwrap(),
        ["vector database parity", "embedding retrieval equivalence"]
    );
}

#[test]
fn multi_query_returns_vec_from_bare_json_array() {
    let mock = Mock::new();
    mock.respond_with("[\"a\",\"b\",\"c\"]", usage(5, 5, 0));
    let agent = started(&mock, CopilotAgentOptions::default());

    assert_eq!(
        agent
            .multi_query(
                "how to install a pack",
                MultiQueryOptions {
                    count: Some(3),
                    timeout_ms: None
                }
            )
            .unwrap(),
        ["a", "b", "c"]
    );
}

#[test]
fn expand_query_errors_when_not_a_json_array() {
    let mock = Mock::new();
    mock.respond_with("{\"not\":\"an array\"}", usage(5, 5, 0));
    let agent = started(&mock, CopilotAgentOptions::default());
    assert!(agent
        .expand_query("q", ExpandQueryOptions::default())
        .unwrap_err()
        .is_response_format());
}

#[test]
fn expand_query_errors_when_array_contains_non_strings() {
    let mock = Mock::new();
    mock.respond_with("[1, 2, 3]", usage(5, 5, 0));
    let agent = started(&mock, CopilotAgentOptions::default());
    assert!(agent
        .expand_query("q", ExpandQueryOptions::default())
        .unwrap_err()
        .is_response_format());
}

// ── identifySeedArticles ────────────────────────────────────────────────────

#[test]
fn seed_articles_returns_selected_titles_from_fenced_array() {
    let mock = Mock::new();
    mock.respond_with("```json\n[\"Kùzu\",\"HNSW\"]\n```", usage(8, 4, 0));
    let agent = started(&mock, CopilotAgentOptions::default());

    let seeds = agent
        .identify_seed_articles(&SeedArticleRequest {
            topic: "graph databases".into(),
            candidates: vec![
                "Kùzu".into(),
                "HNSW".into(),
                "Cypher".into(),
                "Apache Arrow".into(),
            ],
            limit: None,
        })
        .unwrap();
    assert_eq!(seeds, ["Kùzu", "HNSW"]);
}

#[test]
fn seed_articles_errors_on_a_non_array_response() {
    let mock = Mock::new();
    mock.respond_with("\"just a string\"", usage(8, 4, 0));
    let agent = started(&mock, CopilotAgentOptions::default());
    assert!(agent
        .identify_seed_articles(&SeedArticleRequest {
            topic: "t".into(),
            candidates: vec!["a".into()],
            limit: None,
        })
        .unwrap_err()
        .is_response_format());
}

#[test]
fn seed_articles_enforces_the_optional_limit_cap() {
    let mock = Mock::new();
    mock.respond_with("[\"A\",\"B\",\"C\",\"D\"]", usage(8, 4, 0));
    let agent = started(&mock, CopilotAgentOptions::default());

    let seeds = agent
        .identify_seed_articles(&SeedArticleRequest {
            topic: "t".into(),
            candidates: vec!["A".into(), "B".into(), "C".into(), "D".into()],
            limit: Some(2),
        })
        .unwrap();
    assert_eq!(seeds, ["A", "B"]);
}

// ── Usage accounting ────────────────────────────────────────────────────────

#[test]
fn accumulates_usage_and_request_count_across_calls() {
    let mock = Mock::new();
    mock.respond_sequence(vec![
        TransportResponse {
            content: "first answer".into(),
            usage: usage(10, 20, 0),
        },
        TransportResponse {
            content: "[\"a\",\"b\"]".into(),
            usage: usage(3, 4, 1),
        },
    ]);
    let agent = started(&mock, CopilotAgentOptions::default());

    agent.synthesize_answer(&synth("q", vec![])).unwrap();
    agent
        .expand_query("q", ExpandQueryOptions::default())
        .unwrap();

    let snap = agent.get_usage();
    assert_eq!(snap.prompt_tokens, 13);
    assert_eq!(snap.completion_tokens, 24);
    assert_eq!(snap.reasoning_tokens, 1);
    assert_eq!(snap.total_tokens, 38);
    assert_eq!(snap.request_count, 2);
}

#[test]
fn per_call_usage_is_not_cumulative() {
    let mock = Mock::new();
    mock.respond_sequence(vec![
        TransportResponse {
            content: "first".into(),
            usage: usage(10, 20, 0),
        },
        TransportResponse {
            content: "second".into(),
            usage: usage(1, 2, 0),
        },
    ]);
    let agent = started(&mock, CopilotAgentOptions::default());

    agent.synthesize_answer(&synth("q", vec![])).unwrap();
    let second = agent.synthesize_answer(&synth("q", vec![])).unwrap();

    assert_eq!(second.usage, usage(1, 2, 0));
    assert_eq!(agent.get_usage().request_count, 2);
    assert_eq!(agent.get_usage().total_tokens, 33);
}

#[test]
fn get_usage_returns_a_copy_that_cannot_mutate_internal_state() {
    let mock = Mock::new();
    mock.respond_with("x", usage(5, 5, 0));
    let agent = started(&mock, CopilotAgentOptions::default());
    agent.synthesize_answer(&synth("q", vec![])).unwrap();

    let mut snap = agent.get_usage();
    snap.total_tokens = 99999;
    assert_eq!(snap.total_tokens, 99999);
    assert_eq!(agent.get_usage().total_tokens, 10);
}

#[test]
fn accrues_usage_even_when_response_fails_validation() {
    let mock = Mock::new();
    mock.respond_with("not-json-garbage", usage(5, 5, 0));
    let agent = started(&mock, CopilotAgentOptions::default());

    assert!(agent
        .expand_query("q", ExpandQueryOptions::default())
        .unwrap_err()
        .is_response_format());
    let snap = agent.get_usage();
    assert_eq!(snap.prompt_tokens, 5);
    assert_eq!(snap.completion_tokens, 5);
    assert_eq!(snap.request_count, 1);
}

// ── Timeout forwarding ──────────────────────────────────────────────────────

#[test]
fn forwards_the_constructor_default_timeout() {
    let mock = Mock::new();
    mock.respond_with("answer", usage(1, 1, 0));
    let agent = started(
        &mock,
        CopilotAgentOptions {
            timeout_ms: Some(1234),
            ..CopilotAgentOptions::default()
        },
    );

    agent.synthesize_answer(&synth("q", vec![])).unwrap();
    assert_eq!(mock.last_timeout(), Some(1234));
}

#[test]
fn a_per_call_timeout_override_takes_precedence() {
    let mock = Mock::new();
    mock.respond_with("[\"a\"]", usage(1, 1, 0));
    let agent = started(
        &mock,
        CopilotAgentOptions {
            timeout_ms: Some(1234),
            ..CopilotAgentOptions::default()
        },
    );

    agent
        .expand_query(
            "q",
            ExpandQueryOptions {
                count: None,
                timeout_ms: Some(999),
            },
        )
        .unwrap();
    assert_eq!(mock.last_timeout(), Some(999));
}

// ── Error model: transport failures, redaction ──────────────────────────────

#[test]
fn wraps_a_send_failure_as_transport_error() {
    let mock = Mock::new();
    mock.set_responder(|_, _| Err(kgpacks_agent::TransportError::new("socket hang up")));
    let agent = started(&mock, CopilotAgentOptions::default());

    let err = agent.synthesize_answer(&synth("q", vec![])).unwrap_err();
    assert!(err.is_transport());
}

#[test]
fn wraps_a_start_failure_as_transport_error() {
    let mock = Mock::new();
    mock.fail_open("client failed to start");
    let mut agent = CopilotAgent::with_transport(mock.transport(), CopilotAgentOptions::default());
    assert!(agent.start().unwrap_err().is_transport());
}

#[test]
fn redacts_the_byok_secret_from_a_surfaced_transport_error() {
    let secret = "sk-secret-XYZ-should-never-leak";
    let mock = Mock::new();
    mock.fail_open(&format!("connection failed: apiKey={secret}"));
    let mut agent = CopilotAgent::with_transport(
        mock.transport(),
        CopilotAgentOptions {
            provider: Some(ProviderConfig {
                api_key: Some(secret.into()),
                ..ProviderConfig::default()
            }),
            ..CopilotAgentOptions::default()
        },
    );

    let err = agent.start().unwrap_err();
    assert!(err.is_transport());
    assert!(!err.to_string().contains(secret));
}

#[test]
fn response_format_error_carries_a_size_capped_raw_content() {
    let huge = "x".repeat(200_000);
    let mock = Mock::new();
    mock.respond_with(&huge, usage(1, 1, 0));
    let agent = started(&mock, CopilotAgentOptions::default());

    let err = agent
        .expand_query("q", ExpandQueryOptions::default())
        .unwrap_err();
    assert!(err.is_response_format());
    let raw = err.raw_content().unwrap();
    assert!(raw.len() < huge.len());
}

#[test]
fn all_agent_error_types_are_one_catchable_type() {
    use kgpacks_agent::AgentError;
    assert!(AgentError::not_started().is_not_started());
    assert!(AgentError::transport("x").is_transport());
    assert!(AgentError::response_format("x", "raw").is_response_format());
}
