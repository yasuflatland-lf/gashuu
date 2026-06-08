#!/usr/bin/env bash
# check-release-macos-dist.sh - macOS release packaging guard.
#
# This checks the workflow packaging shape and documentation content
# without running the expensive macOS release build.

set -euo pipefail

ROOT="${CHECK_RELEASE_MACOS_ROOT:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"}"
cd "$ROOT"

failures=0
pass() { echo "  OK  $*"; }
fail() { echo "  FAIL $*"; failures=$((failures + 1)); }

require_file() {
  local path="$1"
  if [ -f "$path" ]; then
    pass "$path exists"
  else
    fail "$path missing"
  fi
}

require_executable() {
  local path="$1"
  if [ -x "$path" ]; then
    pass "$path is executable"
  else
    fail "$path is not executable"
  fi
}

require_text() {
  local path="$1"
  local needle="$2"
  if [ ! -f "$path" ]; then
    fail "$path missing; cannot find: $needle"
    return
  fi

  if grep -Fq "$needle" "$path"; then
    pass "$path contains: $needle"
  else
    fail "$path missing: $needle"
  fi
}

forbid_text() {
  local path="$1"
  local needle="$2"
  if [ ! -f "$path" ]; then
    fail "$path missing; cannot check forbidden text: $needle"
    return
  fi

  if grep -Fq "$needle" "$path"; then
    fail "$path contains forbidden text: $needle"
  else
    pass "$path does not contain forbidden text: $needle"
  fi
}

workflow=".github/workflows/release.yml"
ci=".github/workflows/ci.yml"
readme="README.md"

echo ""
echo "=== CHECK 1: release workflow macOS zip contents ==="
require_text "$workflow" 'STAGE="target/release/macos-dist/gashuu-${TAG}-macos-universal"'
require_text "$workflow" 'ditto "$APP" "$STAGE/gashuu.app"'
require_text "$workflow" '"$STAGE/README-macOS.txt"'
require_text "$workflow" 'This macOS build is currently unsigned and not notarized.'
require_text "$workflow" 'System Settings > Privacy & Security'
require_text "$workflow" 'Open Anyway'
require_text "$workflow" 'ditto -c -k --keepParent'
require_text "$workflow" '"$STAGE"'
require_text "$workflow" '"gashuu-${TAG}-macos-universal.zip"'
forbid_text "$workflow" 'cp "scripts/macos/Open gashuu.command"'

echo ""
echo "=== CHECK 2: docs and CI guard ==="
require_text "$readme" "macOS builds are currently unsigned and not notarized."
require_text "$readme" "System Settings"
require_text "$readme" "Open Anyway"
require_text "$readme" "temporary workaround until Developer ID signing"
require_text "$readme" "notarization are added"
require_text "$ci" "bash scripts/check-release-macos-dist.sh"

echo ""
if [ "$failures" -eq 0 ]; then
  echo "All checks passed."
  exit 0
else
  echo "$failures check(s) FAILED."
  exit 1
fi
