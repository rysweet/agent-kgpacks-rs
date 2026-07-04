//! Text chunking for fine-grained retrieval.
//!
//! Rust port of `agent-kgpacks-ts` `packages/ingestion/src/chunking.ts`. Splits
//! each section's text into overlapping, fixed-size character windows (default
//! 512 chars, 64 overlap) so vector search can target a passage rather than a
//! whole section. Overlap preserves context across window boundaries.
//!
//! Parity with the TypeScript reference is deliberate (the canonical write-side
//! reference for chunking is `agent-kgpacks-ts`, so a TS-written pack and a Rust
//! reader agree on chunk granularity and chunk ids):
//!
//! * Defaults: `size = 512`, `overlap = 64`.
//! * Chunk id format: `"{title}#{section}#{chunk}"`.
//! * Windowing: plain fixed-step slicing (`step = max(1, size - overlap)`), no
//!   sentence-boundary detection; the whole text is trimmed once (using the
//!   ECMAScript `String.prototype.trim()` code-point set, see
//!   [`is_js_trim_whitespace`]) and windows are emitted verbatim (never trimmed
//!   per-window), matching `windowText`.
//! * Forward progress is always guaranteed: `overlap` is clamped to `size - 1`
//!   so a caller-supplied `overlap >= size` degrades gracefully instead of
//!   panicking (mirrors TS `resolveOptions`).
//!
//! Indexing uses Unicode scalar values (`char`), so this module is UTF-8 safe
//! for multi-byte text and never slices on a non-character boundary. For
//! Basic-Multilingual-Plane text this coincides with the TS reference's
//! `String.prototype.slice` (UTF-16 code units); astral characters are treated
//! as single windowing units here, an intentional safety-preserving refinement.

/// Default target chunk size in characters, matching `agent-kgpacks-ts`.
pub const DEFAULT_CHUNK_SIZE: usize = 512;

/// Default overlap between consecutive chunks in characters, matching
/// `agent-kgpacks-ts`.
pub const DEFAULT_OVERLAP: usize = 64;

/// Returns `true` for exactly the code points that ECMAScript
/// `String.prototype.trim()` removes (its `WhiteSpace` ∪ `LineTerminator`
/// sets), so this port's leading/trailing trim matches `agent-kgpacks-ts`
/// `windowText`'s `text.trim()` byte-for-byte on the trimmed result — and,
/// because the trim result feeds every window offset, so do all downstream
/// chunk boundaries, contents, and ids.
///
/// This deliberately differs from Rust's `char::is_whitespace()` (the Unicode
/// `White_Space` property) in exactly the two BMP code points where the two
/// runtimes disagree:
///
/// * `U+FEFF` (BOM / zero-width no-break space): trimmed by JS, **not** by
///   `char::is_whitespace()` — so it is added here.
/// * `U+0085` (NEL / next line): trimmed by `char::is_whitespace()`, **not** by
///   JS — so it is excluded here.
///
/// Every other code point in the two sets already agrees.
fn is_js_trim_whitespace(c: char) -> bool {
    c == '\u{FEFF}' || (c.is_whitespace() && c != '\u{0085}')
}

/// A text chunk with the metadata needed for graph storage.
///
/// Mirrors the TS `Chunk`. `chunk_id` uses `#` as the separator:
/// `"{title}#{section}#{chunk}"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Stable identifier: `"{article_title}#{section_index}#{chunk_index}"`.
    pub chunk_id: String,
    /// Chunk text content (a verbatim window of the trimmed section text).
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
/// `chunk_size` is treated as at least `1`, and `overlap` is clamped to
/// `chunk_size - 1`, so the window always makes forward progress (mirrors the
/// TS `resolveOptions` clamp) — a degenerate `overlap >= chunk_size` degrades
/// gracefully instead of panicking.
pub fn chunk_text_with(
    text: &str,
    article_title: &str,
    section_index: usize,
    chunk_size: usize,
    overlap: usize,
) -> Vec<Chunk> {
    let trimmed = text.trim_matches(is_js_trim_whitespace);
    if trimmed.is_empty() {
        return Vec::new();
    }

    let chars: Vec<char> = trimmed.chars().collect();
    let len = chars.len();

    // Window dimensions: size >= 1, overlap < size (guarantees step >= 1).
    let size = chunk_size.max(1);
    let overlap = overlap.min(size - 1);

    // Short text: a single chunk holding the whole trimmed text.
    if len <= size {
        return vec![Chunk {
            chunk_id: format!("{article_title}#{section_index}#0"),
            content: trimmed.to_string(),
            article_title: article_title.to_string(),
            section_index,
            chunk_index: 0,
        }];
    }

    let step = size - overlap;
    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut chunk_idx = 0usize;

    loop {
        let end = (start + size).min(len);
        let content: String = chars[start..end].iter().collect();
        chunks.push(Chunk {
            chunk_id: format!("{article_title}#{section_index}#{chunk_idx}"),
            content,
            article_title: article_title.to_string(),
            section_index,
            chunk_index: chunk_idx,
        });
        // Stop once this window reaches the end of the text (matches
        // `windowText`'s `if (start + size >= length) break`).
        if start + size >= len {
            break;
        }
        start += step;
        chunk_idx += 1;
    }

    chunks
}

/// Chunk the `content` of every section of an article, concatenating the chunks
/// in section order. Section `i` produces chunks with `section_index == i`.
///
/// Mirrors TS `chunkArticle`, which reads each section's content directly; this
/// takes the section contents to keep the crate free of any pipeline-specific
/// section type.
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
