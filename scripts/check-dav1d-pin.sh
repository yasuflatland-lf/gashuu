#!/usr/bin/env bash
# check-dav1d-pin.sh - dav1d version pin guard.
#
# Every from-source dav1d build (release.yml: macOS, Linux, Windows; ci.yml:
# Windows) must use ONE version, so the decoder CI tests is the decoder that
# ships. Asserts both workflows declare the same DAV1D_VERSION, that no job
# hard-codes a dav1d tag, and that Windows builds through the shared action.

set -euo pipefail

ROOT="${CHECK_DAV1D_PIN_ROOT:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"}"
cd "$ROOT"

failures=0
pass() { echo "  OK  $*"; }
fail() { echo "  FAIL $*"; failures=$((failures + 1)); }

release=".github/workflows/release.yml"
ci=".github/workflows/ci.yml"
action=".github/actions/build-dav1d-windows/action.yml"

pin_of() {
  sed -nE "s/^  DAV1D_VERSION: [\"']([^\"']+)[\"']$/\1/p" "$1"
}

echo ""
echo "=== CHECK 1: one DAV1D_VERSION across workflows ==="
rel_pin="$(pin_of "$release")"
ci_pin="$(pin_of "$ci")"
if [ -z "$rel_pin" ]; then fail "$release declares no workflow-level DAV1D_VERSION"; fi
if [ -z "$ci_pin" ]; then fail "$ci declares no workflow-level DAV1D_VERSION"; fi
if [ -n "$rel_pin" ] && [ "$rel_pin" = "$ci_pin" ]; then
  pass "DAV1D_VERSION=$rel_pin in both workflows"
elif [ -n "$rel_pin" ] && [ -n "$ci_pin" ]; then
  fail "DAV1D_VERSION differs: $release=$rel_pin, $ci=$ci_pin"
fi

echo ""
echo "=== CHECK 2: no hard-coded dav1d clone tag ==="
if grep -nE 'clone --branch [0-9]' "$release" "$ci" "$action"; then
  fail "a dav1d clone hard-codes its tag; use \$DAV1D_VERSION"
else
  pass "every dav1d clone uses the pinned variable"
fi

echo ""
echo "=== CHECK 3: Windows builds dav1d through the shared action ==="
for wf in "$release" "$ci"; do
  if grep -Fq 'uses: ./.github/actions/build-dav1d-windows' "$wf"; then
    pass "$wf uses the shared Windows dav1d action"
  else
    fail "$wf does not use ./.github/actions/build-dav1d-windows"
  fi
  if grep -Fq 'vcpkg install dav1d' "$wf"; then
    fail "$wf still installs dav1d through vcpkg"
  fi
done

echo ""
if [ "$failures" -eq 0 ]; then
  echo "All checks passed."
  exit 0
else
  echo "$failures check(s) FAILED."
  exit 1
fi
