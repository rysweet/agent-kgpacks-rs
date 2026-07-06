//! Deterministic chunk dumper used by the cross-implementation parity harness
//! (`qa/chunker-parity/`).
//!
//! Reads a tab-separated corpus and emits one line per produced chunk so its
//! output can be diffed byte-for-byte against the `agent-kgpacks-ts`
//! `windowText` reference oracle.
//!
//! Usage:
//!
//! ```text
//! chunk_dump <corpus.tsv> [size] [overlap]
//! ```
//!
//! Corpus format: one section per non-empty line, `title \t section \t text`.
//! Output format: one chunk per line, `chunk_id \t escaped_content`, where the
//! content is escaped (`\` `\t` `\n` `\r`) so it stays on a single line and is
//! reproducible identically by the reference oracle.

use kgpacks_embeddings::{chunk_text_with, DEFAULT_CHUNK_SIZE, DEFAULT_OVERLAP};
use std::process::ExitCode;

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: chunk_dump <corpus.tsv> [size] [overlap]");
        return ExitCode::from(2);
    };
    let size: usize = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_CHUNK_SIZE);
    let overlap: usize = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_OVERLAP);

    let corpus = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("chunk_dump: cannot read {path}: {e}");
            return ExitCode::from(2);
        }
    };

    let mut out = String::new();
    for line in corpus.lines() {
        if line.is_empty() {
            continue;
        }
        let mut fields = line.splitn(3, '\t');
        let (Some(title), Some(section), Some(text)) =
            (fields.next(), fields.next(), fields.next())
        else {
            eprintln!("chunk_dump: malformed corpus line: {line:?}");
            return ExitCode::from(2);
        };
        let Ok(section_index) = section.parse::<usize>() else {
            eprintln!("chunk_dump: bad section index in line: {line:?}");
            return ExitCode::from(2);
        };
        for chunk in chunk_text_with(text, title, section_index, size, overlap) {
            out.push_str(&chunk.chunk_id);
            out.push('\t');
            out.push_str(&escape(&chunk.content));
            out.push('\n');
        }
    }
    print!("{out}");
    ExitCode::SUCCESS
}
