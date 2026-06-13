#!/usr/bin/env bash
# collect_images.sh — Fetch Pepper&Carrot webcomic episodes into per-episode folders.
#
# Pepper&Carrot (https://www.peppercarrot.com) is a free/libre webcomic by
# David Revoy, released under CC-BY 4.0. This ops tool downloads the comic page
# images for a range of episodes and lays them out one folder per episode, which
# is exactly the shape gashuu's library opens (a folder of images = a book).
# It is a data-collection utility for local testing/sample data — NOT part of the
# build or CI gates.
#
# How it works (robust by design):
#   1. Fetch the language index once and read the real "webcomic/epNN_*.html"
#      links from it — we never hand-build episode titles, so the casing
#      mismatch between the page name (ep01_Potion-of-flight) and the image
#      directory (ep01_Potion-of-Flight) cannot bite us.
#   2. For each requested episode, fetch its page and extract the comic-page
#      image URLs (low-res "<lang>_Pepper-and-Carrot_by-David-Revoy_EnnPmm.jpg").
#      The pattern naturally excludes the text-free "gfx-only" banner image.
#   3. Download each image into output/epNN/, skipping files already present so
#      re-runs resume rather than re-fetch.
#
# Testing seam: set COLLECT_IMAGES_BASE_URL to point the index/page fetches at a
# fixture host instead of the live site (mirrors the CHECK_*_ROOT seam used by
# the other scripts/). Combine with --dry-run for a side-effect-free check.
#
# Usage:
#   ops/collect_images/collect_images.sh                 # episodes 1..10, ja, low-res
#   ops/collect_images/collect_images.sh 1-5             # a range
#   ops/collect_images/collect_images.sh 1 3 7           # individual episodes
#   ops/collect_images/collect_images.sh -l en 1-10      # English
#   ops/collect_images/collect_images.sh --dry-run 1-3   # list URLs, download nothing
#
set -euo pipefail

# ---------------------------------------------------------------------------
# Defaults (most overridable via flags / env; SLEEP_BETWEEN and USER_AGENT are
# edit-here knobs)
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BASE_URL="${COLLECT_IMAGES_BASE_URL:-https://www.peppercarrot.com}"
LANG_CODE="ja"
RESOLUTION="low-res"
OUTPUT_ROOT="$SCRIPT_DIR/output"
DRY_RUN=0
FORCE=0
USER_AGENT="gashuu-collect-images/1.0 (+https://github.com/yasuflatland-lf/gashuu)"
SLEEP_BETWEEN="0.3"   # be polite to the server between downloads

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
err()  { echo "error: $*" >&2; }
note() { echo "$*"; }

usage() {
  cat <<'EOF'
collect_images.sh — fetch Pepper&Carrot episodes into per-episode folders.

Usage:
  collect_images.sh [options] [EPISODES...]

  EPISODES               Episode numbers; plain "7" and ranges "1-10" may be
                         mixed. Defaults to 1-10 when omitted.

Options:
  -l, --lang CODE        Site language code (default: ja)
  -o, --output DIR       Output root (default: <script-dir>/output)
  -r, --resolution R     low-res (default) or hi-res
      --dry-run          List image URLs and destinations; download nothing
      --force            Re-download even if the file already exists
  -h, --help             Show this help

See ops/collect_images/README.md for details and license/attribution notes.
EOF
}

# Expand episode CLI tokens into a sorted, unique list of integers.
# Accepts plain numbers ("7") and inclusive ranges ("1-10").
# Returns non-zero (and prints to stderr) on any malformed token. The `return`s
# live in the function body — not a pipe subshell — so they reach the caller.
expand_episodes() {
  local token start end n out=""
  for token in "$@"; do
    if [[ "$token" =~ ^[0-9]+$ ]]; then
      out+="$token"$'\n'
    elif [[ "$token" =~ ^([0-9]+)-([0-9]+)$ ]]; then
      start="${BASH_REMATCH[1]}"
      end="${BASH_REMATCH[2]}"
      if [ "$start" -gt "$end" ]; then
        err "invalid range '$token' (start > end)"
        return 1
      fi
      for ((n = start; n <= end; n++)); do out+="$n"$'\n'; done
    else
      err "invalid episode token '$token' (expected N or N-M)"
      return 1
    fi
  done
  printf '%s' "$out" | sort -n -u
}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------
positional=()
while [ "$#" -gt 0 ]; do
  case "$1" in
    -l|--lang)        LANG_CODE="${2:?--lang needs a value}"; shift 2 ;;
    -o|--output)      OUTPUT_ROOT="${2:?--output needs a value}"; shift 2 ;;
    -r|--resolution)  RESOLUTION="${2:?--resolution needs a value}"; shift 2 ;;
    --dry-run)        DRY_RUN=1; shift ;;
    --force)          FORCE=1; shift ;;
    -h|--help)        usage; exit 0 ;;
    --)               shift; while [ "$#" -gt 0 ]; do positional+=("$1"); shift; done ;;
    -*)               err "unknown option '$1'"; exit 2 ;;
    *)                positional+=("$1"); shift ;;
  esac
done

case "$RESOLUTION" in
  low-res|hi-res) ;;
  *) err "invalid --resolution '$RESOLUTION' (expected low-res or hi-res)"; exit 2 ;;
esac

# Default episode set: 1..10.
if [ "${#positional[@]}" -eq 0 ]; then
  positional=(1-10)
fi

episodes="$(expand_episodes "${positional[@]}")" || exit 2
[ -n "$episodes" ] || { err "no episodes to process"; exit 2; }

# ---------------------------------------------------------------------------
# Fetch the language index once and read its real episode links.
# ---------------------------------------------------------------------------
# Note the index page lives under "webcomics" (plural); the per-episode links it
# contains are "webcomic/..." (singular, matched at line ~163). Do not unify them.
index_url="$BASE_URL/$LANG_CODE/webcomics/peppercarrot.html"
note "Fetching index: $index_url"
index_html="$(curl -fsSL -A "$USER_AGENT" "$index_url")" || {
  err "could not fetch index ($index_url)"
  exit 1
}
# Links look like: webcomic/ep01_Potion-of-Flight.html
index_links="$(printf '%s' "$index_html" \
  | grep -oE "webcomic/ep[0-9]+_[^\"' ]+\.html" | sort -u || true)"
if [ -z "$index_links" ]; then
  err "no episode links found at $index_url (site layout changed?)"
  exit 1
fi

# ---------------------------------------------------------------------------
# Per-episode collection
# ---------------------------------------------------------------------------
total_downloaded=0
total_skipped=0
total_failed=0
missing_episodes=()

while IFS= read -r ep; do
  [ -n "$ep" ] || continue
  pad="$(printf '%02d' "$ep")"

  # Resolve this episode's page link from the index (real link, real casing).
  link="$(printf '%s\n' "$index_links" | grep -E "^webcomic/ep${pad}_" | head -1 || true)"
  if [ -z "$link" ]; then
    err "episode $pad not found in index — skipping"
    missing_episodes+=("$pad")
    total_failed=$((total_failed + 1))
    continue
  fi

  page_url="$BASE_URL/$LANG_CODE/$link"
  note ""
  note "=== Episode $pad ==="
  note "Page: $page_url"

  page_html="$(curl -fsSL -A "$USER_AGENT" "$page_url")" || {
    err "could not fetch episode page ($page_url) — skipping"
    total_failed=$((total_failed + 1))
    continue
  }

  # Extract the comic-page images. We always read the low-res image URLs the page
  # embeds, then (for hi-res) rewrite the directory segment — the filenames are
  # identical across resolutions. The "<lang>_Pepper-and-Carrot..._EnnPmm.jpg"
  # pattern keeps the text-free "gfx-only" banner out.
  image_urls="$(printf '%s' "$page_html" \
    | grep -oE "https?://[^\"' )]+/0_sources/[^\"' )]+/low-res/${LANG_CODE}_Pepper-and-Carrot_by-David-Revoy_E[0-9]+P[0-9]+\.jpg" \
    | sort -u || true)"
  if [ "$RESOLUTION" = "hi-res" ]; then
    image_urls="$(printf '%s\n' "$image_urls" | sed 's#/low-res/#/hi-res/#')"
  fi

  if [ -z "$image_urls" ]; then
    err "no comic-page images found for episode $pad (lang '$LANG_CODE'?) — skipping"
    total_failed=$((total_failed + 1))
    continue
  fi

  ep_dir="$OUTPUT_ROOT/ep$pad"
  # Treat a dir-creation failure like any other per-episode error (count + skip)
  # so the run still reaches its summary rather than aborting mid-collection.
  if [ "$DRY_RUN" -eq 0 ] && ! mkdir -p "$ep_dir"; then
    err "could not create output dir $ep_dir — skipping episode $pad"
    total_failed=$((total_failed + 1))
    continue
  fi

  while IFS= read -r img; do
    [ -n "$img" ] || continue
    fname="$(basename "$img")"
    dest="$ep_dir/$fname"

    if [ "$DRY_RUN" -eq 1 ]; then
      note "  [dry-run] $img -> $dest"
      continue
    fi

    if [ "$FORCE" -eq 0 ] && [ -f "$dest" ]; then
      note "  skip (exists): $fname"
      total_skipped=$((total_skipped + 1))
      continue
    fi

    note "  get: $fname"
    # `curl -f` rejects HTTP errors, but a 200 with an empty/truncated body still
    # exits 0; require a non-empty file (`-s`) so a corrupt download is counted as
    # a failure instead of silently promoted to a 0-byte "image".
    if curl -fsSL -A "$USER_AGENT" --retry 3 --retry-delay 2 -o "$dest.part" "$img" \
       && [ -s "$dest.part" ]; then
      mv -f "$dest.part" "$dest"
      total_downloaded=$((total_downloaded + 1))
      # Be polite between requests. Guarded so a `sleep` that rejects a fractional
      # interval can never abort the run under `set -e`.
      sleep "$SLEEP_BETWEEN" 2>/dev/null || true
    else
      rm -f "$dest.part"
      err "failed to download $img"
      total_failed=$((total_failed + 1))
    fi
  done <<< "$image_urls"
done <<< "$episodes"

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
note ""
if [ "$DRY_RUN" -eq 1 ]; then
  note "Dry run complete (no files written)."
else
  note "Done. downloaded=$total_downloaded skipped=$total_skipped failed=$total_failed"
  note "Output: $OUTPUT_ROOT"
fi

if [ "${#missing_episodes[@]}" -gt 0 ]; then
  err "episodes not present in index: ${missing_episodes[*]}"
fi

[ "$total_failed" -eq 0 ]
