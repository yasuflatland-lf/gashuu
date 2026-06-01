#!/usr/bin/env bash
# check-docs.sh — Documentation structure harness for gashuu.
#
# Enforces two invariants:
#   1. CLAUDE.md (L1 entry point) stays at or under MAX_L1_LINES lines.
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
MAX_L1_LINES=35
failures=0

pass() { echo "  OK  $*"; }
fail() { echo "  FAIL $*"; failures=$((failures + 1)); }

# ---------------------------------------------------------------------------
# CHECK 1 — L1 size cap: CLAUDE.md must not exceed MAX_L1_LINES lines.
# awk 'END{print NR}' is robust to a missing trailing newline (unlike wc -l).
# ---------------------------------------------------------------------------
echo ""
echo "=== CHECK 1: CLAUDE.md line count (max $MAX_L1_LINES) ==="

if [ ! -f "CLAUDE.md" ]; then
  fail "CLAUDE.md not found"
else
  line_count=$(awk 'END{print NR}' CLAUDE.md)
  if [ "$line_count" -gt "$MAX_L1_LINES" ]; then
    fail "CLAUDE.md is $line_count lines (limit $MAX_L1_LINES)"
  else
    pass "CLAUDE.md is $line_count lines (<= $MAX_L1_LINES)"
  fi
fi

# ---------------------------------------------------------------------------
# CHECK 2 — Link integrity across CLAUDE.md and all docs/**/*.md.
# For each markdown link target ](target):
#   - Skip external URLs (http://, https://, mailto://) and protocol-relative.
#   - Strip trailing #anchor; skip pure in-page anchors (#section).
#   - Resolve relative to the containing file's directory.
#   - Fail if the resolved path does not exist (file or directory).
# ---------------------------------------------------------------------------
echo ""
echo "=== CHECK 2: Markdown link integrity ==="

# Build the file list: CLAUDE.md + every *.md under docs/
file_list=()
[ -f "CLAUDE.md" ] && file_list+=("CLAUDE.md")
while IFS= read -r -d '' md_file; do
  file_list+=("$md_file")
done < <(find docs -type f -name "*.md" -print0 2>/dev/null)

if [ "${#file_list[@]}" -eq 0 ]; then
  fail "No markdown files found (CLAUDE.md or docs/*.md)"
fi

for f in "${file_list[@]}"; do
  # Extract all ](target) occurrences from the file
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
      fail "Broken link in $f: [$raw_link] -> $resolved (not found)"
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
