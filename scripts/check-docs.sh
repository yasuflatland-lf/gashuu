#!/usr/bin/env bash
# check-docs.sh — Documentation structure harness for gashuu.
#
# Enforces two invariants:
#   1. CLAUDE.md (L1 entry point) stays within MIN_L1_LINES..MAX_L1_LINES lines.
#   2. Every relative markdown link in CLAUDE.md and docs/**/*.md resolves
#      to a real file or directory (relative to the containing file's dir).
#
# Seam for testing: set CHECK_DOCS_ROOT to point at a fixture tree and the
# script runs against that tree instead of the real repo. Example:
#   CHECK_DOCS_ROOT=/tmp/fixture bash scripts/check-docs.sh
#
# Usage (normal):
#   bash scripts/check-docs.sh
#   mise run check-docs

set -euo pipefail

# ---------------------------------------------------------------------------
# Root resolution (overridable via CHECK_DOCS_ROOT for fixture-tree testing)
# ---------------------------------------------------------------------------
ROOT="${CHECK_DOCS_ROOT:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"}"
cd "$ROOT"

# ---------------------------------------------------------------------------
# Shared state
# ---------------------------------------------------------------------------
MIN_L1_LINES=10
MAX_L1_LINES=35
failures=0

pass() { echo "  OK  $*"; }
fail() { echo "  FAIL $*"; failures=$((failures + 1)); }

# ---------------------------------------------------------------------------
# CHECK 1 — L1 size cap: CLAUDE.md must be within MIN_L1_LINES..MAX_L1_LINES.
# awk 'END{print NR}' is robust to a missing trailing newline (unlike wc -l).
# ---------------------------------------------------------------------------
echo ""
echo "=== CHECK 1: CLAUDE.md line count (min $MIN_L1_LINES, max $MAX_L1_LINES) ==="

if [ ! -f "CLAUDE.md" ]; then
  fail "CLAUDE.md not found"
else
  line_count=$(awk 'END{print NR}' CLAUDE.md)
  if [ "$line_count" -lt "$MIN_L1_LINES" ]; then
    fail "CLAUDE.md is $line_count lines (minimum $MIN_L1_LINES; file may be empty or accidentally wiped)"
  elif [ "$line_count" -gt "$MAX_L1_LINES" ]; then
    fail "CLAUDE.md is $line_count lines (limit $MAX_L1_LINES)"
  else
    pass "CLAUDE.md is $line_count lines (>= $MIN_L1_LINES, <= $MAX_L1_LINES)"
  fi
fi

# ---------------------------------------------------------------------------
# CHECK 2 — Link integrity across CLAUDE.md and all docs/**/*.md.
# For each markdown link target ](target):
#   - Skip external URLs (http://, https://, mailto://) and protocol-relative.
#   - Strip trailing #anchor; skip pure in-page anchors (#section).
#   - Resolve relative to the containing file's directory.
#   - Fail if the resolved path does not exist (file or directory).
#
# Note: only inline-style links [text](target) are checked.  Reference-style
# links [x][ref], autolinks <url>, and link titles (file "title") are not
# recognised by the grep below and will be silently skipped.
# ---------------------------------------------------------------------------
echo ""
echo "=== CHECK 2: Markdown link integrity ==="

# Guard: docs/ directory must exist.
if [ ! -d "docs" ]; then
  fail "docs/ directory not found"
fi

# Build the file list: CLAUDE.md + every *.md under docs/ (follow symlinks).
file_list=()
[ -f "CLAUDE.md" ] && file_list+=("CLAUDE.md")
doc_md_count=0
while IFS= read -r -d '' md_file; do
  file_list+=("$md_file")
  doc_md_count=$((doc_md_count + 1))
done < <(find -L docs -type f -name "*.md" -print0 2>/dev/null)

if [ "${#file_list[@]}" -eq 0 ]; then
  fail "No markdown files found (CLAUDE.md or docs/*.md)"
fi

# Require at least one docs/*.md (the L2 layer must not vanish silently).
if [ "$doc_md_count" -eq 0 ]; then
  fail "no docs/*.md found (docs/ layer is empty)"
fi

for f in "${file_list[@]}"; do
  # Fail loudly on unreadable files rather than silently skipping their links.
  if [ ! -r "$f" ]; then
    fail "Cannot read $f"
    continue
  fi

  # Extract all ](target) occurrences from the file.
  # Only inline-style links are matched; see the known-limitation note above.
  while IFS= read -r raw_link; do
    # Strip leading ]( and trailing )
    target="${raw_link#\]\(}"
    target="${target%\)}"

    # Skip external / protocol links (http, https, mailto, and any ://)
    case "$target" in
      http://*|https://*|mailto:*|*://*) continue ;;
    esac

    # Strip trailing #anchor fragment
    target="${target%%#*}"

    # Skip pure in-page anchors (result is empty after stripping fragment)
    [ -z "$target" ] && continue

    # Resolve relative to the containing file's directory
    dir="$(dirname "$f")"
    resolved="$dir/$target"

    if [ ! -e "$resolved" ]; then
      fail "Broken link in $f: '$target' -> $resolved (not found)"
    fi
  done < <(grep -oE '\]\([^)]+\)' "$f" 2>/dev/null || true)
done

echo ""

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
if [ "$failures" -eq 0 ]; then
  echo "All checks passed."
  exit 0
else
  echo "$failures check(s) FAILED."
  exit 1
fi
