#!/usr/bin/env python3
# File: docs/quality-audit/tests/check_links.py
#
# Verifies every intra-repo Markdown link in docs/quality-audit/*.md resolves:
#   - the target file exists (relative to the linking file), and
#   - any `#anchor` matches a heading in the target file, using GitHub's
#     heading-slug algorithm (github-slugger semantics).
#
# External links (http/https/mailto) are not fetched (offline-safe).
# Exit 0 if all links resolve, 1 otherwise. Emits TAP-ish lines.

import os
import re
import sys

DOCS_DIR = os.path.dirname(os.path.abspath(__file__))
DOCS_DIR = os.path.dirname(DOCS_DIR)  # parent: docs/quality-audit

LINK_RE = re.compile(r"(?<!\!)\[[^\]]+\]\(([^)]+)\)")
ATX_RE = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")


def strip_code_fences(lines):
    """Yield (lineno, text) for lines outside fenced code blocks."""
    fence = None
    for i, line in enumerate(lines, 1):
        stripped = line.lstrip()
        m = re.match(r"^(```+|~~~+)", stripped)
        if m:
            token = m.group(1)[0] * 3
            if fence is None:
                fence = token
            elif stripped.startswith(fence):
                fence = None
            continue
        if fence is None:
            yield i, line


def gh_slug(text):
    """Replicate github-slugger: lowercase, drop everything that is not a
    word char / space / hyphen, then spaces -> hyphens."""
    s = text.strip().lower()
    # Remove inline markdown emphasis/code markers so `Foo` -> foo.
    s = s.replace("`", "")
    s = re.sub(r"[^\w \-]", "", s, flags=re.UNICODE)
    s = s.replace(" ", "-")
    return s


def headings_slugs(path):
    slugs = {}
    counts = {}
    with open(path, encoding="utf-8") as fh:
        lines = fh.read().splitlines()
    for _, line in strip_code_fences(lines):
        m = ATX_RE.match(line)
        if not m:
            continue
        base = gh_slug(m.group(2))
        if base in counts:
            counts[base] += 1
            slug = f"{base}-{counts[base]}"
        else:
            counts[base] = 0
            slug = base
        slugs[slug] = True
    return slugs


def links_in(path):
    with open(path, encoding="utf-8") as fh:
        lines = fh.read().splitlines()
    out = []
    for lineno, line in strip_code_fences(lines):
        for m in LINK_RE.finditer(line):
            out.append((lineno, m.group(1)))
    return out


def main():
    md_files = sorted(
        os.path.join(DOCS_DIR, f)
        for f in os.listdir(DOCS_DIR)
        if f.endswith(".md")
    )
    if not md_files:
        print("not ok - no markdown files found in", DOCS_DIR)
        return 1

    slug_cache = {}
    failures = 0
    checks = 0

    for md in md_files:
        for lineno, target in links_in(md):
            if target.startswith(("http://", "https://", "mailto:", "tel:")):
                continue
            checks += 1
            rel = os.path.relpath(md, DOCS_DIR)

            filepart, _, anchor = target.partition("#")
            if filepart == "":
                target_file = md  # same-file anchor
            else:
                target_file = os.path.normpath(
                    os.path.join(os.path.dirname(md), filepart)
                )

            if not os.path.isfile(target_file):
                print(f"not ok - {rel}:{lineno} broken link -> {target} "
                      f"(missing file {target_file})")
                failures += 1
                continue

            if anchor:
                if target_file not in slug_cache:
                    slug_cache[target_file] = headings_slugs(target_file)
                if anchor not in slug_cache[target_file]:
                    print(f"not ok - {rel}:{lineno} broken anchor -> {target} "
                          f"(no heading '#{anchor}' in "
                          f"{os.path.relpath(target_file, DOCS_DIR)})")
                    failures += 1
                    continue

            print(f"ok - {rel}:{lineno} link resolves -> {target}")

    print(f"# link check: {checks - failures}/{checks} links resolve "
          f"({failures} broken)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
