# In-memory browser spike (#651)

From the repository root, use Python 3.9 or later with Pillow 10.1 or later installed in the active environment. Build before generating the books because Trunk replaces `dist/`. Record the actual byte sizes printed by the generator.

```sh
cd crates/gashuu
mise exec cargo:trunk@0.22.1 -- trunk build --release
python3 -I ../../spike/make_sample_cbz.py --pages 32 --out dist/sample-50.cbz
python3 -I ../../spike/make_sample_cbz.py --pages 190 --out dist/sample-300.cbz
python3 -I -m http.server 8080 --bind 0.0.0.0 --directory dist
```

The static server uses plain HTTP without COOP or COEP headers. Use `http://localhost:8080/?book=sample-50.cbz` on desktop and replace `localhost` with the server's LAN IP on iPad. Repeat with `?book=sample-300.cbz`.

1. Open the browser console, filter for `GASHUU_METRIC`, and hard-reload. Record `boot_ms`, `fetch_ms`, `open_ms`, and `first_page_ms`. Exclude the initial page's `read_ms` and `decode_ms` from the turn samples.
2. Start on page 1 in single-page mode. Send Next 30 times and then Prev 10 times, waiting for each page to render. Use Right and Left on the focused canvas, or tap the right and left page zones on iPad. Use the first 30 `turn_ms` lines as misses and the last 10 as hits. Each miss has `read_ms` and `decode_ms` lines before its `turn_ms`. `decode_ms` measures from the ZIP read's return to the cache dispatch's return, including cache overhead. Compute p50, p95, and max in milliseconds for each series.
3. After the final page renders, evaluate `window.gashuuLogWasmHeap()` in the console and record `wasm_heap_bytes`. Measure peak tab memory separately in Chrome Task Manager or Safari Web Inspector, using USB for iPad.

Each metric is a single console line, for example `GASHUU_METRIC turn_ms=72.500 book=sample-50.cbz`. Use the `book` field to identify the run.

In Chrome, paste this before the first turn, then evaluate `spikeLongTasks.disconnect(); spikeLongTaskCount` after the last turn.

```js
let spikeLongTaskCount = 0;
const spikeTurnStart = performance.now();
const spikeLongTasks = new PerformanceObserver(list => {
  spikeLongTaskCount += list.getEntries().filter(entry => entry.startTime >= spikeTurnStart && entry.duration > 50).length;
});
spikeLongTasks.observe({ type: 'longtask', buffered: true });
```

Run these once from `crates/gashuu` to get the raw, gzip, and Brotli Wasm sizes in bytes.

```sh
wc -c dist/*.wasm
gzip -9 -c dist/*.wasm | wc -c
brotli -q 11 -c dist/*.wasm | wc -c
```

For the Library text-input check, press Up or Escape from the Viewer, then click the search field. Press Down from the carousel to return to the Viewer.
