#!/usr/bin/env bash
# build-releases-mdx.sh -- regenerate the docs-site Release Notes page from CHANGELOG.md.
#
# Reads:  CHANGELOG.md (Keep a Changelog format with `## [X.Y.Z] - YYYY-MM-DD` headings)
# Writes: docs/src/content/docs/weftos/vision/releases.mdx
#
# The Unreleased section is dropped from the public page (it's staging only).
# Compare-link footnotes at the bottom of CHANGELOG.md are also dropped --
# they're not useful in the rendered site.
#
# Usage:
#   scripts/build-releases-mdx.sh           # regenerate
#   scripts/build-releases-mdx.sh --check   # exit non-zero if regen would change the file
#
# Wired from the docs-build flow so the Release Notes page never drifts.

set -euo pipefail

CHECK_MODE=0
if [ "${1:-}" = "--check" ]; then
    CHECK_MODE=1
fi

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CHANGELOG="${REPO_ROOT}/CHANGELOG.md"
OUTPUT="${REPO_ROOT}/docs/src/content/docs/weftos/vision/releases.mdx"

if [ ! -f "$CHANGELOG" ]; then
    echo "error: CHANGELOG.md not found at ${CHANGELOG}" >&2
    exit 1
fi

TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

python3 - "$CHANGELOG" > "$TMP" <<'PYEOF'
import re
import sys
from pathlib import Path

src = Path(sys.argv[1]).read_text()

# Drop the file header (lines before the first `## [` heading).
parts = re.split(r'^## \[', src, maxsplit=1, flags=re.MULTILINE)
if len(parts) != 2:
    sys.stderr.write("error: no '## [' headings found in CHANGELOG.md\n")
    sys.exit(2)
body = '## [' + parts[1]

# Drop the link-footnote block at the bottom: a contiguous run of
# `[X]: url` lines preceded by a blank line at end of file.
body = re.sub(r'\n+(?:^\[[^\]]+\]:\s+\S+\s*\n?)+\Z', '\n', body, flags=re.MULTILINE)

# Find every release section. Sections are introduced by
# `## [X.Y.Z] - YYYY-MM-DD`. Skip `## [Unreleased]`.
section_re = re.compile(
    r'^## \[(?P<ver>[^\]]+)\](?:\s*-\s*(?P<date>\d{4}-\d{2}-\d{2}))?\s*$',
    flags=re.MULTILINE,
)

matches = list(section_re.finditer(body))
if not matches:
    sys.stderr.write("error: no version sections matched\n")
    sys.exit(3)

# Slice each section out (start of this heading -> start of next, or EOF).
sections = []
for i, m in enumerate(matches):
    start = m.end()
    end = matches[i + 1].start() if i + 1 < len(matches) else len(body)
    text = body[start:end].strip('\n')
    sections.append((m.group('ver'), m.group('date'), text))

# Drop Unreleased.
sections = [s for s in sections if s[0].lower() != 'unreleased']

# Build the MDX.
out = []
out.append('---')
out.append('title: Release Notes')
out.append('description: Auto-generated from CHANGELOG.md. Complete WeftOS version history.')
out.append('---')
out.append('')
out.append('# Release Notes')
out.append('')
out.append('> This page is generated from `CHANGELOG.md` by')
out.append('> `scripts/build-releases-mdx.sh`. Edits made directly here will be')
out.append('> overwritten on the next docs build. To change a release entry, edit')
out.append('> `CHANGELOG.md` and re-run the script.')
out.append('')

for ver, date, text in sections:
    heading = f'## v{ver}'
    out.append(heading)
    out.append('')
    if date:
        out.append(f'*{date}*')
        out.append('')
    if text:
        out.append(text)
        out.append('')
    out.append('---')
    out.append('')

# Trim trailing separators.
while out and out[-1] in ('', '---'):
    out.pop()

print('\n'.join(out) + '\n', end='')
PYEOF

if [ "$CHECK_MODE" -eq 1 ]; then
    if ! diff -q "$OUTPUT" "$TMP" > /dev/null 2>&1; then
        echo "error: ${OUTPUT} is stale; re-run scripts/build-releases-mdx.sh" >&2
        diff -u "$OUTPUT" "$TMP" || true
        exit 1
    fi
    echo "ok: ${OUTPUT} matches CHANGELOG.md"
    exit 0
fi

mkdir -p "$(dirname "$OUTPUT")"
cp "$TMP" "$OUTPUT"
echo "regenerated ${OUTPUT}"
