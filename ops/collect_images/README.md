# collect_images

`collect_images.sh` downloads [Pepper&Carrot](https://www.peppercarrot.com)
webcomic episodes into one folder per episode — the exact shape gashuu's library
opens (a folder of images = a book). Use it to generate realistic local
sample/test data.

This is an **ops data-collection utility**, not part of the build or CI gates.

## Usage

```sh
# Episodes 1..10, Japanese, low-res (defaults)
ops/collect_images/collect_images.sh

# A range
ops/collect_images/collect_images.sh 1-5

# Individual episodes
ops/collect_images/collect_images.sh 1 3 7

# English, episodes 1..10
ops/collect_images/collect_images.sh -l en 1-10

# List the image URLs without downloading anything
ops/collect_images/collect_images.sh --dry-run 1-3
```

### Options

| Flag | Default | Meaning |
| --- | --- | --- |
| `EPISODES...` | `1-10` | Episode numbers; accepts `N` and inclusive ranges `N-M`, mixed |
| `-l, --lang CODE` | `ja` | Site language code (`ja`, `en`, `fr`, …) |
| `-o, --output DIR` | `ops/collect_images/output` | Output root (per-episode folders created under it; default is `<script-dir>/output`, independent of your cwd) |
| `-r, --resolution R` | `low-res` | `low-res` or `hi-res` |
| `--dry-run` | off | Print the image URLs and destinations, download nothing |
| `--force` | off | Re-download even if the file already exists (default: skip) |
| `-h, --help` | | Show usage |

Re-runs skip files already present, so an interrupted run resumes safely.

## Output layout

```
output/
  ep01/
    ja_Pepper-and-Carrot_by-David-Revoy_E01P00.jpg   # cover (page 0)
    ja_Pepper-and-Carrot_by-David-Revoy_E01P01.jpg
    ...
  ep02/
    ...
```

The `output/` directory is git-ignored — downloaded comics are not committed.

## How it resolves episodes

The script reads the **real** `webcomic/epNN_*.html` links from the language
index rather than hand-building titles, so the casing mismatch between the page
name (`ep01_Potion-of-flight`) and the image directory
(`ep01_Potion-of-Flight`) cannot break it. Comic pages are matched by the
`<lang>_Pepper-and-Carrot_by-David-Revoy_EnnPmm.jpg` filename pattern, which
also keeps the text-free `gfx-only` banner out.

For `--resolution hi-res`, the low-res URL embedded in the page is taken and its
`/low-res/` path segment is rewritten to `/hi-res/` (the filenames are identical
across resolutions). If the site ever changes that layout, hi-res downloads will
fail with "failed to download" rather than collecting the wrong file.

## License & attribution

Pepper&Carrot is created by **David Revoy** and released under
**[CC-BY 4.0](https://creativecommons.org/licenses/by/4.0/)**. The artwork is
free to use, share, and adapt **with attribution**. If you redistribute anything
collected here, retain: the creator credit (David Revoy), a link to the
[CC-BY 4.0 license](https://creativecommons.org/licenses/by/4.0/), a link back to
the original at <https://www.peppercarrot.com>, and — if you modified the work —
a note that changes were made. Please also be considerate of the server when
collecting many episodes.
