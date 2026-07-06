//! Parity tests for the chunker against `agent-kgpacks-ts`
//! (`packages/ingestion/src/chunking.ts` + `test/chunking.test.ts`).
//!
//! These assert the TS contract: empty input yields nothing, short text is a
//! single verbatim chunk, long text splits into fixed-size overlapping windows
//! (no sentence-boundary logic), `chunk_id` is `"{title}#{section}#{idx}"`,
//! `overlap >= size` still makes forward progress, and `chunk_sections`
//! concatenates per section skipping empty ones.

use kgpacks_embeddings::{
    chunk_sections, chunk_text, chunk_text_with, Chunk, DEFAULT_CHUNK_SIZE, DEFAULT_OVERLAP,
};

#[test]
fn defaults_match_agent_kgpacks_ts() {
    // TS `DEFAULT_SIZE = 512`, `DEFAULT_OVERLAP = 64`.
    assert_eq!(DEFAULT_CHUNK_SIZE, 512);
    assert_eq!(DEFAULT_OVERLAP, 64);
}

#[test]
fn empty_or_whitespace_yields_no_chunks() {
    // TS `windowText('   ') => []`.
    assert!(chunk_text("", "Title", 0).is_empty());
    assert!(chunk_text("   \n\t ", "Title", 0).is_empty());
}

#[test]
fn short_text_is_a_single_chunk_with_expected_id() {
    // TS: `windowText('short', 100, 10) => ['short']`, id `Title#0#0`.
    let chunks = chunk_text("Hello world.", "Python", 0);
    assert_eq!(chunks.len(), 1);
    assert_eq!(
        chunks[0],
        Chunk {
            chunk_id: "Python#0#0".to_string(),
            content: "Hello world.".to_string(),
            article_title: "Python".to_string(),
            section_index: 0,
            chunk_index: 0,
        }
    );
}

#[test]
fn short_text_is_trimmed_as_a_whole() {
    // TS trims the whole text once before windowing (`text.trim()`).
    let chunks = chunk_text("  trimmed body  ", "T", 3);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].content, "trimmed body");
    assert_eq!(chunks[0].section_index, 3);
    assert_eq!(chunks[0].chunk_id, "T#3#0");
}

#[test]
fn produces_overlapping_windows_that_cover_the_whole_text() {
    // Mirrors the TS `windowText('abcdefghij', 4, 1)` case (step = 3).
    let chunks = chunk_text_with("abcdefghij", "Topic", 0, 4, 1);
    let contents: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
    assert_eq!(contents, vec!["abcd", "defg", "ghij"]);
    let ids: Vec<&str> = chunks.iter().map(|c| c.chunk_id.as_str()).collect();
    assert_eq!(ids, vec!["Topic#0#0", "Topic#0#1", "Topic#0#2"]);
    // The last window reaches the end of the text.
    assert!(chunks.last().unwrap().content.ends_with('j'));
}

#[test]
fn windows_are_fixed_size_not_sentence_aware() {
    // Unlike the former Python-lineage chunker, no boundary snapping: each
    // window is exactly `size` chars (except the last), sliced verbatim.
    let text = "First sentence here. Second sentence follows. Third one.";
    let chunks = chunk_text_with(text, "T", 0, 25, 5);
    assert!(chunks.len() >= 2);
    // The first window is the raw first 25 characters, mid-sentence.
    assert_eq!(chunks[0].content.chars().count(), 25);
    assert_eq!(chunks[0].content, "First sentence here. Seco");
}

#[test]
fn interior_whitespace_in_a_window_is_preserved() {
    // TS never trims individual windows, only the whole text once.
    let text = "aa      bb"; // len 10, interior run of spaces
    let chunks = chunk_text_with(text, "T", 0, 4, 1); // step 3
    assert_eq!(chunks[1].content, "    "); // window [3..7) is all spaces, kept
}

#[test]
fn long_text_splits_into_overlapping_windows_with_sequential_ids() {
    // 100 chars, size 20, overlap 5 => step 15 => 7 windows.
    let text: String = "abcdefghij".repeat(10);
    let chunks = chunk_text_with(&text, "T", 5, 20, 5);

    assert_eq!(chunks.len(), 7);
    assert_eq!(chunks[0].content.chars().count(), 20);
    assert_eq!(chunks[6].content.chars().count(), 10);

    for (i, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.chunk_id, format!("T#5#{i}"));
        assert_eq!(chunk.section_index, 5);
        assert_eq!(chunk.article_title, "T");
    }

    // The 5-char overlap: tail of window 0 equals head of window 1.
    let c0: Vec<char> = chunks[0].content.chars().collect();
    let c1: Vec<char> = chunks[1].content.chars().collect();
    assert_eq!(c0[15..20], c1[0..5]);
}

#[test]
fn overlap_at_least_size_still_makes_forward_progress() {
    // TS: `windowText('abcdef', 3, 99)` -> clamps overlap, always progresses.
    let chunks = chunk_text_with("abcdef", "T", 0, 3, 99);
    assert!(!chunks.is_empty());
    assert!(chunks.len() <= 6);
    // Overlap clamped to size - 1 = 2 => step 1.
    assert_eq!(chunks[0].content, "abc");
    assert_eq!(chunks[1].content, "bcd");
}

#[test]
fn chunk_size_zero_degrades_gracefully() {
    // Defensive: a zero window size is clamped to 1 instead of underflowing.
    let chunks = chunk_text_with("abc", "T", 0, 0, 0);
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0].content, "a");
    assert_eq!(chunks[2].chunk_id, "T#0#2");
}

#[test]
fn multibyte_text_is_sliced_on_character_boundaries() {
    // Cyrillic (2-byte UTF-8) must window by scalar value, never panicking.
    let text = "абвгдеёжзи"; // 10 chars
    let chunks = chunk_text_with(text, "T", 0, 4, 1); // step 3
    let contents: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
    assert_eq!(contents, vec!["абвг", "гдеё", "ёжзи"]);
}

#[test]
fn astral_scalars_are_windowed_by_char_not_utf16_units() {
    // Documented intentional refinement over the TS reference: the port windows
    // by Unicode scalar value (`char`), so an astral code point (U+1F600, which
    // is two UTF-16 code units in TS `String.prototype.slice`) counts as a
    // single windowing unit here. This keeps slicing UTF-8 safe (never panics
    // on a code-point boundary). Six emoji, size 4, overlap 1 => step 3.
    let text = "😀".repeat(6);
    let chunks = chunk_text_with(&text, "T", 0, 4, 1);
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].content.chars().count(), 4);
    assert_eq!(chunks[1].content.chars().count(), 3);
    assert_eq!(chunks[0].chunk_id, "T#0#0");
    assert_eq!(chunks[1].chunk_id, "T#0#1");
}

#[test]
fn trims_the_ecmascript_whitespace_set_not_rust_whitespace() {
    // U+FEFF (BOM) is stripped by JS `.trim()` but not by Rust `str::trim()`;
    // it must be stripped here so a TS-written pack and this reader agree.
    let chunks = chunk_text("\u{FEFF}Hello world.\u{FEFF}", "T", 0);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].content, "Hello world.");

    // U+0085 (NEL) is stripped by Rust `str::trim()` but NOT by JS `.trim()`;
    // it must be preserved here to match TS.
    let chunks = chunk_text("\u{0085}abc\u{0085}", "T", 0);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].content, "\u{0085}abc\u{0085}");

    // An all-U+FEFF string trims to empty (parity with JS), yielding no chunks.
    assert!(chunk_text("\u{FEFF}\u{FEFF}", "T", 0).is_empty());
}

#[test]
fn chunk_sections_concatenates_per_section_and_skips_empty() {
    // Mirrors TS `chunkArticle` skipping empty sections while preserving index.
    let sections = ["First section body.", "", "Third section body."];
    let chunks = chunk_sections(&sections, "Article");
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].section_index, 0);
    assert_eq!(chunks[0].chunk_id, "Article#0#0");
    assert_eq!(chunks[1].section_index, 2);
    assert_eq!(chunks[1].chunk_id, "Article#2#0");
}
