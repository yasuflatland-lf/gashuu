#!/usr/bin/env bash
# check-tokens.sh — Visual-token drift guard for gashuu's Slint UI.
#
# DESIGN.md ("Iteration / Agent Prompt Guide" rule 2) makes Theme.slint the
# single source of truth for color: components reference `Theme.<token>` and must
# NEVER paste raw hex inline. This script enforces that mechanically so the rule
# survives without per-PR vigilance.
#
# It scans crates/gashuu/ui/*.slint for raw color hex (#rgb..#rrggbbaa) and fails
# on any match, with two deliberate exclusions:
#   * Theme.slint        — the single source of truth; raw hex is its whole job.
#   * ALLOWLIST (below)  — files not yet migrated to Theme.* tokens. P0 only fixes
#                          the two hard violations; P1 migrates these wholesale and
#                          deletes them from the array, at which point the guard
#                          becomes blocking everywhere. Until then they are reported
#                          as a visible WARN (never silently skipped).
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
# Files not yet migrated to Theme.* tokens. P1 removes these (shrinks to empty),
# then the guard is blocking for the whole UI.
ALLOWLIST=(
  ThumbnailStrip.slint
  SettingsDialog.slint
  FirstRunGuide.slint
)

failures=0
pass() { echo "  OK  $*"; }
fail() { echo "  FAIL $*"; failures=$((failures + 1)); }

is_allowlisted() {
  local name="$1"
  local entry
  for entry in "${ALLOWLIST[@]}"; do
    [ "$name" = "$entry" ] && return 0
  done
  return 1
}

# Count raw-hex matches in a file (0 if none). `|| true` keeps the no-match
# grep exit (1) from tripping `set -e`/`pipefail`.
hex_count() { { grep -oE "$HEX_RE" "$1" || true; } | grep -c . || true; }

# ---------------------------------------------------------------------------
# Scan crates/gashuu/ui/*.slint for inline hex outside Theme.slint + allowlist.
# ---------------------------------------------------------------------------
echo ""
echo "=== check-tokens: raw color hex outside $SOURCE_OF_TRUTH ==="

if [ ! -d "$UI_DIR" ]; then
  fail "$UI_DIR not found"
else
  scanned=0
  pending=()
  for f in "$UI_DIR"/*.slint; do
    # Guard the literal glob when no .slint file matches.
    [ -e "$f" ] || continue
    name="$(basename "$f")"

    # Theme.slint is the single source of truth — never scanned.
    [ "$name" = "$SOURCE_OF_TRUTH" ] && continue

    # Allowlisted files are not yet migrated; remember them for the WARN below.
    if is_allowlisted "$name"; then
      [ "$(hex_count "$f")" -gt 0 ] && pending+=("$name")
      continue
    fi

    scanned=$((scanned + 1))
    # Report every offending file:line:match (-o so the column points at the hex).
    matches="$(grep -onE "$HEX_RE" "$f" || true)"
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

  # No silent caps: surface the still-excluded files so the debt stays visible
  # (Diana Mounter — a guard you can't see the holes in isn't a guard).
  if [ "${#pending[@]}" -gt 0 ]; then
    echo ""
    echo "WARN: ${#pending[@]} file(s) excluded from the hex guard, pending P1 token migration:"
    for name in "${pending[@]}"; do
      echo "  - $name ($(hex_count "$UI_DIR/$name") raw hex)"
    done
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
