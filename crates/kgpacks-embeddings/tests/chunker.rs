//! Parity tests for the chunker (`bootstrap/src/embeddings/chunker.py`).
//!
//! The reference ships no `test_chunker.py`; these assert the documented
//! contract: empty input yields nothing, short text is a single chunk, long
//! text splits with overlap and prefers sentence boundaries, `chunk_id` is
//! `"{title}|s{section}|c{idx}"`, and `chunk_sections` concatenates per section.

use kgpacks_embeddings::{chunk_sections, chunk_text, chunk_text_with, Chunk};

#[test]
fn empty_or_whitespace_yields_no_chunks() {
    assert!(chunk_text("", "Title", 0).is_empty());
    assert!(chunk_text("   \n\t ", "Title", 0).is_empty());
}

#[test]
fn short_text_is_a_single_chunk_with_expected_id() {
    let chunks = chunk_text("Hello world.", "Python", 0);
    assert_eq!(chunks.len(), 1);
    assert_eq!(
        chunks[0],
        Chunk {
            chunk_id: "Python|s0|c0".to_string(),
            content: "Hello world.".to_string(),
            article_title: "Python".to_string(),
            section_index: 0,
            chunk_index: 0,
        }
    );
}

#[test]
fn short_text_is_stripped() {
    let chunks = chunk_text("  trimmed body  ", "T", 3);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].content, "trimmed body");
    assert_eq!(chunks[0].section_index, 3);
    assert_eq!(chunks[0].chunk_id, "T|s3|c0");
}

#[test]
fn long_text_splits_into_overlapping_chunks() {
    // 100 chars, no sentence boundaries: deterministic fixed-window splitting.
    let text: String = "abcdefghij".repeat(10);
    let chunks = chunk_text_with(&text, "T", 5, 20, 5);

    // start advances by chunk_size - overlap = 15 each step: 7 chunks.
    assert_eq!(chunks.len(), 7);
    assert_eq!(chunks[0].content.chars().count(), 20);
    assert_eq!(chunks[6].content.chars().count(), 10);

    // Sequential ids carrying the section index.
    for (i, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.chunk_id, format!("T|s5|c{i}"));
        assert_eq!(chunk.section_index, 5);
        assert_eq!(chunk.article_title, "T");
    }

    // The 5-char overlap: tail of chunk 0 equals head of chunk 1.
    let c0: Vec<char> = chunks[0].content.chars().collect();
    let c1: Vec<char> = chunks[1].content.chars().collect();
    assert_eq!(c0[15..20], c1[0..5]);
}

#[test]
fn long_text_prefers_a_sentence_boundary() {
    let text = "First sentence here. Second sentence follows. Third one.";
    let chunks = chunk_text_with(text, "T", 0, 25, 5);
    assert!(chunks.len() >= 2);
    // The first chunk breaks on the latest sentence boundary in the window,
    // so it ends with a full sentence rather than mid-word.
    assert!(chunks[0].content.starts_with("First sentence"));
    assert!(chunks[0].content.ends_with("follows."));
}

#[test]
fn chunk_sections_concatenates_per_section() {
    let sections = ["First section body.", "", "Third section body."];
    let chunks = chunk_sections(&sections, "Article");
    // The empty section contributes nothing; sections 0 and 2 each yield a chunk.
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].section_index, 0);
    assert_eq!(chunks[0].chunk_id, "Article|s0|c0");
    assert_eq!(chunks[1].section_index, 2);
    assert_eq!(chunks[1].chunk_id, "Article|s2|c0");
}

#[test]
#[should_panic(expected = "must be less than chunk_size")]
fn overlap_at_least_chunk_size_panics() {
    // Mirrors the reference `ValueError`: overlap must be < chunk_size.
    let _ = chunk_text_with(&"x".repeat(50), "T", 0, 10, 10);
}
