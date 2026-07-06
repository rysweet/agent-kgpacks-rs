// Reference oracle for the chunker cross-implementation parity harness.
//
// The `windowText` function below is copied VERBATIM from the canonical
// TypeScript reference `agent-kgpacks-ts/packages/ingestion/src/chunking.ts`
// (only the type-only `import` was dropped, and `.trim()` / `.slice()` are the
// stock JS string methods). Running it here on the shared corpus produces the
// golden output that the Rust `chunk_dump` example must reproduce byte-for-byte.
//
// Usage: node reference_oracle.mjs <corpus.tsv> [size] [overlap]
// Output: one chunk per line, `chunk_id \t escaped_content` (same escaping as
// the Rust example), so the two dumps can be diffed directly.

import { readFileSync } from 'node:fs';

// --- verbatim from chunking.ts -------------------------------------------------
/** Splits one piece of text into overlapping windows. Empty/blank text → `[]`. */
function windowText(text, size, overlap) {
  const trimmed = text.trim();
  if (trimmed.length === 0) {
    return [];
  }
  if (trimmed.length <= size) {
    return [trimmed];
  }
  const step = Math.max(1, size - overlap);
  const windows = [];
  for (let start = 0; start < trimmed.length; start += step) {
    windows.push(trimmed.slice(start, start + size));
    if (start + size >= trimmed.length) {
      break;
    }
  }
  return windows;
}
// -------------------------------------------------------------------------------

function escape(s) {
  let out = '';
  for (const c of s) {
    if (c === '\\') out += '\\\\';
    else if (c === '\t') out += '\\t';
    else if (c === '\n') out += '\\n';
    else if (c === '\r') out += '\\r';
    else out += c;
  }
  return out;
}

const [corpusPath, sizeArg, overlapArg] = process.argv.slice(2);
if (!corpusPath) {
  process.stderr.write('usage: reference_oracle.mjs <corpus.tsv> [size] [overlap]\n');
  process.exit(2);
}
const size = sizeArg !== undefined ? Number.parseInt(sizeArg, 10) : 512;
const overlap = overlapArg !== undefined ? Number.parseInt(overlapArg, 10) : 64;

const corpus = readFileSync(corpusPath, 'utf-8');
let out = '';
// Split on line boundaries the same way Rust's `str::lines()` does: break on
// `\n`, drop a trailing `\r`, and never yield a trailing empty record. This
// keeps the two parsers provably equivalent regardless of trailing newline or
// CRLF endings, so the diff reflects only chunker behavior.
for (const line of corpus.split('\n')) {
  const record = line.endsWith('\r') ? line.slice(0, -1) : line;
  if (record.length === 0) continue;
  // Mirror Rust's `splitn(3, '\t')`: title, section, then the remainder as
  // text (any further tabs stay inside the text field, not dropped).
  const parts = record.split('\t');
  const title = parts[0];
  const section = parts[1];
  const text = parts.slice(2).join('\t');
  const sectionIndex = Number.parseInt(section, 10);
  const windows = windowText(text, size, overlap);
  for (let i = 0; i < windows.length; i++) {
    out += `${title}#${sectionIndex}#${i}\t${escape(windows[i])}\n`;
  }
}
process.stdout.write(out);
