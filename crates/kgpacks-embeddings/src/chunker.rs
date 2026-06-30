//! Text chunking for fine-grained retrieval.
//!
//! Rust port of `bootstrap/src/embeddings/chunker.py`. Splits section text into
//! overlapping chunks (~500 tokens / ~2000 chars) so vector search can target a
//! passage rather than a whole section. Overlap preserves context across chunk
//! boundaries, and chunks prefer to break on a sentence boundary.
//!
//! Indexing follows Python's character semantics (the reference slices the
//! `str` by character index), so this module operates on `Vec<char>` and is
//! UTF-8 safe for multi-byte text.

/// Default target chunk size in characters (~500 tokens), matching the reference.
pub const DEFAULT_CHUNK_SIZE: usize = 2000;

/// Default overlap between consecutive chunks in characters, matching the reference.
pub const DEFAULT_OVERLAP: usize = 400;

/// Sentence-ending boundary markers searched for when splitting a long chunk.
/// Each is a two-character `(punctuation, whitespace)` pair, mirroring the
/// reference's `(". ", "? ", "! ", ".\n", "?\n", "!\n")`.
const SENTENCE_BOUNDARIES: [[char; 2]; 6] = [
    ['.', ' '],
    ['?', ' '],
    ['!', ' '],
    ['.', '\n'],
    ['?', '\n'],
    ['!', '\n'],
];

/// A text chunk with the metadata needed for graph storage.
///
/// Mirrors the reference `Chunk` dataclass. `chunk_id` uses `|` as a separator
/// (forbidden in Wikipedia titles, safe for IDs): `"{title}|s{section}|c{idx}"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Stable identifier: `"{article_title}|s{section_index}|c{chunk_index}"`.
    pub chunk_id: String,
    /// Chunk text content (stripped of leading/trailing whitespace).
    pub content: String,
    /// Title of the article this chunk belongs to.
    pub article_title: String,
    /// Index of the source section within the article.
    pub section_index: usize,
    /// Index of this chunk within its section.
    pub chunk_index: usize,
}

/// Split `text` into overlapping chunks using the default size/overlap.
///
/// Short text (`<= chunk_size` characters) yields a single chunk; empty or
/// whitespace-only text yields no chunks.
pub fn chunk_text(text: &str, article_title: &str, section_index: usize) -> Vec<Chunk> {
    chunk_text_with(
        text,
        article_title,
        section_index,
        DEFAULT_CHUNK_SIZE,
        DEFAULT_OVERLAP,
    )
}

/// Split `text` into overlapping chunks of `chunk_size` characters with
/// `overlap` characters of overlap between consecutive chunks.
///
/// # Panics
///
/// Panics if `overlap >= chunk_size` (mirrors the reference `ValueError`); this
/// is a configuration error, not a runtime condition.
pub fn chunk_text_with(
    text: &str,
    article_title: &str,
    section_index: usize,
    chunk_size: usize,
    overlap: usize,
) -> Vec<Chunk> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    assert!(
        overlap < chunk_size,
        "overlap ({overlap}) must be less than chunk_size ({chunk_size})"
    );

    let chars: Vec<char> = trimmed.chars().collect();
    let len = chars.len();

    // Short text: single chunk.
    if len <= chunk_size {
        return vec![Chunk {
            chunk_id: format!("{article_title}|s{section_index}|c0"),
            content: trimmed.to_string(),
            article_title: article_title.to_string(),
            section_index,
            chunk_index: 0,
        }];
    }

    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut chunk_idx = 0usize;

    while start < len {
        // `end` is intentionally left unclamped (it may exceed `len`); the slice
        // is clamped separately, but the next `start` is computed from the
        // unclamped value, exactly as the reference does.
        let mut end = start + chunk_size;

        // Try to break at a sentence boundary within the back half of the window.
        if end < len {
            let search_end = (end + 200).min(len);
            let search_start = start + chunk_size / 2;
            if let Some(boundary) = best_sentence_boundary(&chars, search_start, search_end) {
                if boundary > start {
                    end = boundary + 1; // include the punctuation, exclude the space
                }
            }
        }

        let slice_end = end.min(len);
        let content: String = chars[start..slice_end].iter().collect();
        let content = content.trim();
        if !content.is_empty() {
            chunks.push(Chunk {
                chunk_id: format!("{article_title}|s{section_index}|c{chunk_idx}"),
                content: content.to_string(),
                article_title: article_title.to_string(),
                section_index,
                chunk_index: chunk_idx,
            });
            chunk_idx += 1;
        }

        start = end.saturating_sub(overlap);
        if start >= len {
            break;
        }
    }

    chunks
}

/// Find the highest index `b` in `[search_start, search_end)` where a
/// two-character sentence boundary begins (both characters within the window),
/// across all [`SENTENCE_BOUNDARIES`]. Returns `None` if none is found.
///
/// Mirrors taking the maximum of `str.rfind(punct, search_start, search_end)`
/// over every boundary marker.
fn best_sentence_boundary(chars: &[char], search_start: usize, search_end: usize) -> Option<usize> {
    if search_end == 0 {
        return None;
    }
    // The two-character marker must fit entirely inside the window.
    let last = search_end.saturating_sub(1);
    let mut best: Option<usize> = None;
    let mut b = search_start;
    while b < last {
        let pair = [chars[b], chars[b + 1]];
        if SENTENCE_BOUNDARIES.contains(&pair) {
            best = Some(b);
        }
        b += 1;
    }
    best
}

/// Chunk the `content` of every section of an article, concatenating the chunks
/// in section order. Section `i` produces chunks with `section_index == i`.
///
/// The reference accepts full section dicts but only reads each section's
/// `content`; this takes the section contents directly to keep the crate free
/// of any pipeline-specific section type.
pub fn chunk_sections(section_contents: &[&str], article_title: &str) -> Vec<Chunk> {
    chunk_sections_with(
        section_contents,
        article_title,
        DEFAULT_CHUNK_SIZE,
        DEFAULT_OVERLAP,
    )
}

/// [`chunk_sections`] with explicit `chunk_size` and `overlap`.
pub fn chunk_sections_with(
    section_contents: &[&str],
    article_title: &str,
    chunk_size: usize,
    overlap: usize,
) -> Vec<Chunk> {
    let mut all = Vec::new();
    for (i, content) in section_contents.iter().enumerate() {
        all.extend(chunk_text_with(
            content,
            article_title,
            i,
            chunk_size,
            overlap,
        ));
    }
    all
}
