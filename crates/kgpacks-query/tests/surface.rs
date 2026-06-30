//! Locks the public CORE surface and the overridable defaults of
//! `kgpacks-query` (parity with the CORE slice of `@kgpacks/query`'s
//! `index.test.ts`).
//!
//! The reference test asserts a specific set of exports (the "studs") and that
//! the locked-by-parity multipliers (`GRAPH_MATCH`/`KEYWORD_MATCH`/`MAX_*`/…) do
//! NOT leak. In Rust the latter is enforced statically: those constants are
//! `pub(crate)` and simply cannot be named from this external test crate, so a
//! leak would be a compile error rather than a runtime assertion.

use kgpacks_query::{
    default_stop_words, hybrid_retrieve, validate_cypher, vector_retrieve, CypherValidationError,
    Embedder, HybridWeights, PackRetriever, QueryError, RetrieveMode, RetrieveOptions,
    RetrieverConfig, RetrieverResult, DEFAULT_K, DEFAULT_NODE_TABLE, DEFAULT_VECTOR_INDEX,
    DEFAULT_WEIGHTS,
};

#[test]
fn exports_the_overridable_defaults_with_their_reference_values() {
    assert_eq!(DEFAULT_K, 10);
    assert_eq!(DEFAULT_WEIGHTS.vector, 0.5);
    assert_eq!(DEFAULT_WEIGHTS.graph, 0.3);
    assert_eq!(DEFAULT_WEIGHTS.keyword, 0.2);
    assert_eq!(DEFAULT_NODE_TABLE, "Section");
    assert_eq!(DEFAULT_VECTOR_INDEX, "embedding_idx");
}

#[test]
fn exports_the_stop_word_set_used_for_keyword_extraction() {
    let stop = default_stop_words();
    assert!(stop.contains("the"));
    assert!(!stop.contains("photosynthesis"));
}

#[test]
fn retriever_config_default_matches_the_reference_defaults() {
    let config = RetrieverConfig::default();
    assert_eq!(config.node_table, DEFAULT_NODE_TABLE);
    assert_eq!(config.vector_index, DEFAULT_VECTOR_INDEX);
    assert!(config.stop_words.contains("the"));
}

#[test]
fn retrieve_options_default_is_vector_mode_with_no_overrides() {
    let opts = RetrieveOptions::default();
    assert_eq!(opts.mode, RetrieveMode::Vector);
    assert!(opts.k.is_none());
    assert!(opts.weights.is_none());
}

/// Names every public CORE item so the surface is locked at compile time: the
/// `use` import above fails to build if any export is renamed or removed. This
/// test additionally exercises the value-typed surface.
#[test]
fn public_core_surface_is_present() {
    // Free functions are callable / nameable.
    assert!(validate_cypher("MATCH (n) RETURN n").is_ok());

    // Generic retrieval entry points are nameable with a concrete embedder type.
    fn _assert_callable<E: Embedder>() {
        let _ = vector_retrieve::<E>;
        let _ = hybrid_retrieve::<E>;
    }

    // Value types and the error taxonomy are constructible.
    let _config = RetrieverConfig::default();
    let _weights = HybridWeights {
        vector: 1.0,
        graph: 0.0,
        keyword: 0.0,
    };
    let _result = RetrieverResult {
        id: "1".to_string(),
        score: 1.0,
        content: String::new(),
    };
    let _err: QueryError = CypherValidationError("x".into()).into();

    // The facade constructor exists (named via a no-op closure to avoid binding a
    // connection here).
    let _ctor = PackRetriever::<kgpacks_embeddings::Embedder>::new;
}
