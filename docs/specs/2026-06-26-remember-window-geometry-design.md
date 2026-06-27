# Remember and restore the window size + position across launches

- Date: 2026-06-26
- Branch: `feat/remember-window-geometry`
- Status: Brainstorm-approved; not yet implemented

## Problem

The viewer window opens at a fixed boot size every launch. `ViewerWindow.slint`
hard-codes `preferred-width: 900px` / `preferred-height: 1200px` (with a
`min-width: 480px` / `min-height: 600px` floor); nothing reads or writes the
runtime window geometry. `Settings` carries no window fields, and there is no
`window().set_size()` / `set_position()` call anywhere. So a user who resizes or
moves the window has to redo it on every start.

Goal: on exit, remember the window's **size and position**; on the next launch,
restore both — while never leaving the window stranded off-screen when the
monitor layout has changed.

## Decision

Persist the window geometry in `Settings` and restore it at startup. Two product
choices were settled during brainstorming:

1. **Scope = size + position** (not size-only, not + maximized state).
2. **Off-screen fallback = drop the position, keep the size, center on the
   primary monitor.** If the saved position no longer lands on any monitor
   (external display removed, resolution lowered), the saved position is ignored
   and the window is centered; the size is still restored.

## Data model (gashuu-core)

A new value type carries the geometry; `Settings` gains one optional field.

```rust
// crates/gashuu-core/src/window_geometry.rs (new)
pub struct WindowGeometry { pub width: u32, pub height: u32, pub x: i32, pub y: i32 }
pub struct Rect          { pub x: i32, pub y: i32, pub width: u32, pub height: u32 }
```

```rust
// settings.rs
#[serde(default, skip_serializing_if = "Option::is_none")]
pub window: Option<WindowGeometry>,
```

- **Fresh install → `None`** → no `set_size` call → Slint's existing 900×1200
  boot default applies. The feature only ever changes behavior once a geometry
  has been captured.
- **Integer fields (`u32`/`i32`)** so `Settings` keeps its `#[derive(Eq)]`. A
  float field would drop `Eq` and break the many `assert_eq!(settings, …)` tests.
- **`skip_serializing_if = "Option::is_none"`** so a `None` window is omitted
  from `settings.json` entirely. Consequently `Settings::default()` serializes
  unchanged and the existing `default_settings_json_snapshot` insta test needs
  no update.
- **No schema version bump.** This is an additive `#[serde(default)]` field, the
  same forward/backward-compatible field-add pattern used for `fit_mode`,
  `seen_guide`, `language`, and `allow_rar_archives`.

### Why physical pixels, not logical

winit's monitor geometry and Slint's `window().size()` / `position()` are all in
**physical** pixels. Storing physical lets every calculation (visibility,
centering) stay in one coordinate space with zero scale-factor conversion —
which is the main source of multi-DPI bugs. The only cost is that changing the
display's DPI scale between sessions makes the restored window a different
apparent size; that is a rare edge case, and the off-screen clamp already
absorbs the related "resolution changed" case.

### Pure, testable core logic

UI-independent decisions live in `window_geometry.rs` as pure functions with unit
tests (no live event loop needed):

- `clamped_size() -> (u32, u32)` — a sanity floor (≥ the 480×600 logical
  minimum) guarding against zero/garbage stored values. The *exact* minimum is
  re-enforced by Slint's own `min-width` / `min-height` when the size is applied.
- `is_position_visible(&[Rect]) -> bool` — true when the title-bar grab point
  (top-center: `x + width/2`, `y + TITLE_BAR_GRAB`) lies inside some monitor
  rectangle. This is the off-screen test.
- `center_in(primary: Rect, size: (u32, u32)) -> (i32, i32)` — top-left for a
  window centered on the primary monitor.

## Restore flow (gashuu, before `ui.run()`)

When `settings.window` is `Some`:

1. `ui.window().set_size(PhysicalSize::new(w, h))` using `clamped_size()`.
2. Gather monitors via the winit accessor (`with_winit_window` →
   `available_monitors()`, `primary_monitor()`), converting each to a core `Rect`.
3. If `is_position_visible(&rects)` → `set_position(PhysicalPosition::new(x, y))`.
   Otherwise drop the saved position and `set_position(center_in(primary, size))`.

If the build is not winit-backed the accessor degrades to a no-op (same pattern
as drag-drop), so the size is still restored and the position is simply left to
the OS.

## Capture flow (gashuu, after `ui.run()`)

Immediately **before** the existing `settings.borrow().save()` in `main.rs`
(currently `main.rs:298`), read the live geometry and store it:

- `ui.window().size()` (`PhysicalSize`) and `ui.window().position()`
  (`PhysicalPosition`) → `settings.window = Some(WindowGeometry { … })`.
- The existing exit-time `save()` writes it out — no new save site.

The `ui` handle is still alive through the exit sequence (it is used by
`covers.flush_counts` and `write_back_position` after `run()` returns), so
Slint's cached final geometry is readable there.

**Why read-at-exit rather than live tracking:** `on_winit_window_event` is a
single-callback slot already owned by drag-drop (`drag_drop.rs:94`); registering
a second one would overwrite it. Reading once at exit needs **no** winit event
handler and so cannot collide with drag-drop. This is the one behavior to verify
manually (below).

## Files changed

- `crates/gashuu-core/src/window_geometry.rs` (new) — `WindowGeometry`, `Rect`,
  the three pure functions, and their unit tests.
- `crates/gashuu-core/src/settings.rs` — the `window` field; clamp it in
  `normalize`; extend the `non_default_settings()` fixture with a `Some(window)`
  value (distinct from the `None` default, so round-trip tests catch a dropped
  field); add round-trip / serde-default tests.
- `crates/gashuu-core/src/lib.rs` — `pub` re-export of the new module.
- `crates/gashuu/src/window_state.rs` (new) — UI glue: monitor collection,
  restore, and capture, kept out of `main.rs`.
- `crates/gashuu/src/main.rs` — call restore before `ui.run()`, capture after it.

Estimated ≈300 production LOC — within the ≤1000-LOC PR guideline.

## Non-goals / constraints

- **No maximized/fullscreen state.** A window maximized at exit reopens at that
  maximized *size* (a normal window), not in the maximized state.
- **No live position tracking.** Geometry is read once at exit; no per-move/
  per-resize churn and no extra winit handler.
- **Core stays headless** — `window_geometry.rs` defines its own `Rect`; no
  `winit` / `slint` dependency crosses into `gashuu-core`. The winit→`Rect`
  conversion happens entirely in the UI crate.

## Verification

- Gates (all must be green): `mise exec -- cargo fmt --check` ·
  `mise exec -- cargo clippy --workspace --all-targets -- -D warnings` ·
  `mise exec -- cargo nextest run --workspace --profile ci`.
- Unit tests: size clamp, position-visible true/false against synthetic monitor
  rects, centering math, and `Settings` round-trip with a populated `window`.
- Manual (the read-at-exit risk): resize + move the window, quit, relaunch →
  the window reopens at the same size and position. Then move it onto a
  secondary monitor, quit, disconnect that monitor, relaunch → the window
  reopens at the same size, centered on the primary monitor (not off-screen).
  If `size()` / `position()` return stale/zero after `run()` on any platform,
  fall back to live tracking (size via `on_resized`, position via a `Moved`
  branch folded into the existing drag-drop winit handler).

## Amendment (2026-06-27): apply geometry after the window is created

Implementation/review found that winit 0.30 creates the OS window lazily — it does
not exist until the event loop spins. Applying geometry before `ui.run()` therefore
fails: `with_winit_window` returns `None` (so the monitor list is empty and the
position is never restored), and `set_size` is treated as a logical size at scale
1.0 (wrong physical size on HiDPI). The restore is now armed before `ui.run()` but
DEFERRED via `slint::Timer::single_shot(Duration::ZERO, …)`, which fires on the first
event-loop turn once the window exists (with a bounded re-arm guard on
`has_winit_window()`). Capture at exit is unchanged.
