#!/usr/bin/env bash
# Regenerate the Noto Sans JP font and its accompanying files for Trunk.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
scratch=$(mktemp -d "${TMPDIR:-/tmp}/gashuu-web-font.XXXXXXXX")
trap 'rm -rf "$scratch"' EXIT

google_fonts_commit=66a36c8c94b1a5d992ee4e7f392fccfe4945767c
font_sha256=c2f3b4d463500a2ddcd3849cded1fceeb9fd6d1c32e6cbecd568453ba50fc68f
fonttools_version=4.62.1
upstream_url="https://raw.githubusercontent.com/google/fonts/$google_fonts_commit/ofl/notosansjp"
font_name=NotoSansJP-gashuu-subset.ttf
output_dir="$root/crates/gashuu/web/fonts"
max_bytes=$((1536 * 1024))

curl --fail --silent --show-error --location --globoff \
    "$upstream_url/NotoSansJP[wght].ttf" -o "$scratch/upstream.ttf"
curl --fail --silent --show-error --location \
    "$upstream_url/OFL.txt" -o "$scratch/OFL.txt"
printf '%s  %s\n' "$font_sha256" "$scratch/upstream.ttf" | shasum -a 256 -c -

venv="$scratch/venv"
if python3 -m pip --version >/dev/null 2>&1; then
    python3 -m venv "$venv"
    "$venv/bin/python" -m pip install --disable-pip-version-check --no-cache-dir \
        "fonttools==$fonttools_version"
else
    UV_CACHE_DIR="$scratch/uv-cache" uv venv --python python3 "$venv"
    UV_CACHE_DIR="$scratch/uv-cache" uv pip install --python "$venv/bin/python" \
        "fonttools==$fonttools_version"
fi

# Keep the font modification timestamp reproducible using the upstream commit time.
export SOURCE_DATE_EPOCH=1774471326
"$venv/bin/fonttools" varLib.instancer "$scratch/upstream.ttf" wght=400 \
    --update-name-table --output "$scratch/regular.ttf"

"$venv/bin/python" - "$root/crates/gashuu/i18n/ja/gashuu.ftl" \
    "$scratch/subset-codepoints.txt" <<'PY'
from pathlib import Path
import sys

catalog_path, codepoints_path = map(Path, sys.argv[1:])
ranges = [
    (0x0020, 0x007E),  # ASCII
    (0x3000, 0x303F),  # CJK Symbols and Punctuation
    (0x3040, 0x309F),  # Hiragana
    (0x30A0, 0x30FF),  # Katakana
    (0xFF00, 0xFFEF),  # Halfwidth and Fullwidth Forms
]
codepoints = {cp for first, last in ranges for cp in range(first, last + 1)}

# JIS X 0208 level-1 kanji use full EUC-JP rows through 0xCE.
level_one = {
    ord(bytes([row, column]).decode("euc_jp"))
    for row in range(0xB0, 0xD0)
    for column in range(0xA1, 0xD4 if row == 0xCF else 0xFF)
}
if len(level_one) != 2965:
    raise SystemExit("Expected 2965 JIS X 0208 level-1 kanji")
codepoints.update(level_one)
codepoints.update(map(ord, catalog_path.read_bytes().decode("utf-8")))

codepoints_path.write_text(
    "".join(f"U+{cp:04X}\n" for cp in sorted(codepoints)), encoding="ascii"
)
PY

"$venv/bin/pyftsubset" "$scratch/regular.ttf" \
    --unicodes-file="$scratch/subset-codepoints.txt" \
    --layout-features='*' --no-hinting --desubroutinize --flavor= \
    --output-file="$scratch/$font_name"

raw_bytes=$(wc -c < "$scratch/$font_name")
gzip_bytes=$(gzip -9 -n -c "$scratch/$font_name" | wc -c)
printf '%s: %d bytes raw, %d bytes gzip -9\n' "$font_name" "$raw_bytes" "$gzip_bytes"
if (( raw_bytes > max_bytes )); then
    printf 'Web font exceeds the 1.5 MiB budget (%d bytes).\n' "$max_bytes" >&2
    exit 1
fi

mkdir -p "$output_dir"
cp "$scratch/$font_name" "$scratch/OFL.txt" "$scratch/subset-codepoints.txt" "$output_dir/"
