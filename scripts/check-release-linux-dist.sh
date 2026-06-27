#!/usr/bin/env bash
# check-release-linux-dist.sh - Linux release packaging guard.
#
# This checks the workflow packaging shape and documentation content
# without running the expensive Linux release build. It is the Linux
# counterpart to check-release-macos-dist.sh.

set -euo pipefail

ROOT="${CHECK_RELEASE_LINUX_ROOT:-"$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"}"
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

require_text() {
  local path="$1"
  local needle="$2"
  if [ ! -f "$path" ]; then
    fail "$path missing; cannot find: $needle"
    return
  fi

  # -e so needles that begin with '-' (e.g. "--output appimage", "-name '*.deb'")
  # are taken as the pattern, not parsed as grep options.
  if grep -Fq -e "$needle" "$path"; then
    pass "$path contains: $needle"
  else
    fail "$path missing: $needle"
  fi
}

workflow=".github/workflows/release.yml"
ci=".github/workflows/ci.yml"
readme="README.md"
cargo="crates/gashuu/Cargo.toml"
desktop="crates/gashuu/packaging/linux/gashuu.desktop"

echo ""
echo "=== CHECK 1: release workflow Linux build job ==="
require_text "$workflow" 'build-linux:'
require_text "$workflow" 'runs-on: ubuntu-22.04'
require_text "$workflow" 'cargo install cargo-deb --locked'
require_text "$workflow" 'cargo deb -p gashuu --no-build --output "gashuu-${TAG}-amd64.deb"'
require_text "$workflow" 'linuxdeploy --appdir AppDir'
require_text "$workflow" '--output appimage'
require_text "$workflow" 'APPIMAGE_EXTRACT_AND_RUN'
require_text "$workflow" 'name: linux-installers'

echo ""
echo "=== CHECK 2: publish job attaches Linux assets ==="
require_text "$workflow" "-name '*.deb'"
require_text "$workflow" "-name '*.AppImage'"
require_text "$workflow" 'build-linux]'

echo ""
echo "=== CHECK 3: .deb metadata and .desktop entry ==="
require_text "$cargo" '[package.metadata.deb]'
require_text "$cargo" 'depends = "$auto"'
require_file "$desktop"
require_text "$desktop" 'Exec=gashuu'
require_text "$desktop" 'Icon=gashuu'

echo ""
echo "=== CHECK 4: README Linux install docs and CI guard ==="
require_text "$readme" '### Linux release install'
require_text "$readme" 'gashuu-*-x86_64.AppImage'
require_text "$readme" 'sudo apt install ./gashuu-*-amd64.deb'
require_text "$ci" "bash scripts/check-release-linux-dist.sh"

echo ""
if [ "$failures" -eq 0 ]; then
  echo "All checks passed."
  exit 0
else
  echo "$failures check(s) FAILED."
  exit 1
fi
