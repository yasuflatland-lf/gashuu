#!/usr/bin/env bash
# check-tokens.sh — Visual-token drift guard for gashuu's Slint UI.
#
# DESIGN.md ("Iteration / Agent Prompt Guide" rule 2) makes Theme.slint the
# single source of truth for color: components reference `Theme.<token>` and must
# NEVER paste raw hex inline. This script enforces that mechanically so the rule
# survives without per-PR vigilance.
#
# It scans crates/gashuu/ui/*.slint for raw color hex (#rgb..#rrggbbaa) and fails
# on any match, with one deliberate exclusion:
#   * Theme.slint — the single source of truth; raw hex is its whole job.
# The guard is unconditionally blocking for all other UI files. The allowlist
# scaffold used during the P0/P1 migration has been retired now that all
# components reference Theme.* tokens.
#
# Seam for testing: set CHECK_TOKENS_ROOT to point at a fixture tree and the
# script runs against that tree instead of the real repo. Example:
#   CHECK_TOKENS_ROOT=/tmp/fixture bash scripts/check-tokens.sh
#
# Usage (normal):
#   bash scripts/check-tokens.sh
#   mise run check-tokens

set -euo pipefail

# ---------------------------------------------------------------------------
# Root resolution (overridable via CHECK_TOKENS_ROOT for fixture-tree testing)
# ---------------------------------------------------------------------------
ROOT="${CHECK_TOKENS_ROOT:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"}"
cd "$ROOT"

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
UI_DIR="crates/gashuu/ui"
# Raw color hex: #rgb, #rgba, #rrggbb, #rrggbbaa.
HEX_RE='#[0-9a-fA-F]{3,8}'
# The single source of truth — raw hex here is intentional and always excluded.
SOURCE_OF_TRUTH="Theme.slint"

failures=0
pass() { echo "  OK  $*"; }
fail() { echo "  FAIL $*"; failures=$((failures + 1)); }

# ---------------------------------------------------------------------------
# Scan crates/gashuu/ui/*.slint for inline hex outside Theme.slint.
# ---------------------------------------------------------------------------
echo ""
echo "=== check-tokens: raw color hex outside $SOURCE_OF_TRUTH ==="

if [ ! -d "$UI_DIR" ]; then
  fail "$UI_DIR not found"
else
  scanned=0
  for f in "$UI_DIR"/*.slint; do
    # Guard the literal glob when no .slint file matches.
    [ -e "$f" ] || continue
    name="$(basename "$f")"

    # Theme.slint is the single source of truth — never scanned.
    [ "$name" = "$SOURCE_OF_TRUTH" ] && continue

    scanned=$((scanned + 1))
    # Report every offending file:line:match (-o so the column points at the hex).
    # exit 1 = no match (ok); >=2 = real grep error → fail
    matches="$(grep -onE "$HEX_RE" "$f")" || { rc=$?; [ "$rc" -eq 1 ] || fail "grep failed (exit $rc) on $f"; }
    if [ -n "$matches" ]; then
      while IFS= read -r m; do
        fail "raw hex in $f:$m — use a Theme.* token instead"
      done <<< "$matches"
    fi
  done

  if [ "$scanned" -eq 0 ]; then
    fail "no .slint files scanned in $UI_DIR (layer missing or all excluded?)"
  elif [ "$failures" -eq 0 ]; then
    pass "$scanned scanned .slint file(s) free of inline hex"
  fi
fi

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
