#!/usr/bin/env bash
# check-release-macos-dist.sh - macOS release packaging guard.
#
# This checks the temporary unsigned macOS release helper and the workflow
# packaging shape without running the expensive macOS release build.

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

helper="scripts/macos/Open gashuu.command"
workflow=".github/workflows/release.yml"
ci=".github/workflows/ci.yml"
readme="README.md"

echo ""
echo "=== CHECK 1: macOS helper command ==="
require_file "$helper"
require_executable "$helper"
require_text "$helper" "#!/bin/sh"
require_text "$helper" 'APP="/Applications/gashuu.app"'
require_text "$helper" '/usr/bin/osascript'
require_text "$helper" '/usr/bin/xattr -dr com.apple.quarantine "$APP"'
require_text "$helper" '/usr/bin/open "$APP"'
require_text "$helper" 'gashuu.app was not found in /Applications'
forbid_text "$helper" "sudo"

echo ""
echo "=== CHECK 2: release workflow macOS zip contents ==="
require_text "$workflow" 'STAGE="target/release/macos-dist/gashuu-${TAG}-macos-universal"'
require_text "$workflow" 'ditto "$APP" "$STAGE/gashuu.app"'
require_text "$workflow" 'cp "scripts/macos/Open gashuu.command" "$STAGE/Open gashuu.command"'
require_text "$workflow" 'chmod +x "$STAGE/Open gashuu.command"'
require_text "$workflow" '"$STAGE/README-macOS.txt"'
require_text "$workflow" 'This macOS build is currently unsigned and not notarized.'
require_text "$workflow" 'ditto -c -k --keepParent'
require_text "$workflow" '"$STAGE"'
require_text "$workflow" '"gashuu-${TAG}-macos-universal.zip"'

echo ""
echo "=== CHECK 3: docs and CI guard ==="
require_text "$readme" "macOS builds are currently unsigned and not notarized."
require_text "$readme" "Open gashuu.command"
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
