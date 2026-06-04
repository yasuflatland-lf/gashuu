---
version: alpha
name: gashuu-design-system
description: >
  gashuu is a quiet, dark, immersive cross-platform manga reader (Rust + Slint). The
  artwork leads and the chrome recedes: a near-black reading canvas, a cover-flow library
  carousel, and an auto-hiding page scrubber. A single blue accent does all the interactive
  work; a green tint marks "read". Type is minimal and calm — the page and cover imagery
  carry the visual weight. Rendering target is Slint (not the web); tokens below map to a
  single Slint `global Theme`, and CSS-only concepts (gradients, blur) are noted where the
  Slint equivalent differs.

colors:
  accent: "#5b8cff"
  accent-glow: "rgba(91,140,255,0.25)"
  # Glass surfaces — Slint 1.x has NO backdrop-blur; these only approximate "glass"
  # (no real blur — the look is translucent fill + rim + top highlight + drop shadow).
  glass-fill: "#10141fd1"      # surface-float at ~82% alpha — translucent glass-pill fill
  glass-border: "#2f3850b3"    # hairline-float at ~70% alpha — the hairline rim
  glass-highlight: "#dde5f51f" # text-high at ~12% alpha — 1px top inner highlight rim
  glass-sheen-top: "#1a2030d1" # stage-top at ~82% alpha — top stop of the settings panel's fill gradient
  shadow-popover: "#00000080"  # Theme.shadow-popover — popover/panel drop-shadow ink (50% black)
  success: "#41c98a"
  canvas: "#0b0e15"
  surface: "#0e1118"
  surface-raised: "#161b27"
  surface-float: "#10141f"
  surface-sunken: "#0d1019"
  stage-top: "#1a2030"
  hairline: "#262c3a"
  hairline-float: "#2f3850"
  track: "#2a3243"
  track-prog: "#333c4f"
  chip: "#222a3a"
  text: "#ffffff"
  text-high: "#dde5f5"
  text-mid: "#cdd8ef"
  text-muted: "#9fb0cc"
  text-dim: "#7c8bab"
  text-faint: "#67748f"
  win-close: "#ff5f57"
  win-min: "#febc2e"
  win-max: "#28c840"

typography:
  ui-title:
    fontFamily: "system-ui, -apple-system, Segoe UI, sans-serif"
    fontSize: 15px
    fontWeight: 700
    lineHeight: 1.3
  ui-body:
    fontFamily: "system-ui, -apple-system, Segoe UI, sans-serif"
    fontSize: 13px
    fontWeight: 400
    lineHeight: 1.5
  ui-label:
    fontFamily: "system-ui, -apple-system, Segoe UI, sans-serif"
    fontSize: 12px
    fontWeight: 500
    lineHeight: 1.4
  ui-micro:
    fontFamily: "system-ui, -apple-system, Segoe UI, sans-serif"
    fontSize: 11px
    fontWeight: 400
    lineHeight: 1.4
  numeric:
    fontFamily: "system-ui, -apple-system, Segoe UI, sans-serif"
    fontSize: 13px
    fontWeight: 600
    lineHeight: 1.3
    fontVariantNumeric: tabular-nums

rounded:
  xs: 3px
  sm: 6px
  md: 8px
  lg: 10px
  pill: 9999px
  full: 9999px

spacing:
  xxs: 4px
  xs: 6px
  sm: 8px
  md: 10px
  lg: 14px
  xl: 18px
  xxl: 22px
  huge: 26px

elevation:
  flat: "none"
  card: "0 8px 22px rgba(0,0,0,0.55)"
  page: "0 6px 18px rgba(0,0,0,0.50)"
  float: "0 10px 30px rgba(0,0,0,0.55)"
  focus-glow: "0 0 0 4px {colors.accent-glow}"

components:
  cover:
    rounded: "{rounded.sm}"
    shadow: "{elevation.card}"
    focusOutline: "3px solid {colors.accent}"
    focusOffset: 3px
    sideOpacity: 0.45
  progress-bar:
    height: 4px
    rounded: 2px
    trackColor: "{colors.track-prog}"
    fillColor: "{colors.accent}"
    fillColorDone: "{colors.success}"
  chip:
    backgroundColor: "{colors.chip}"
    textColor: "{colors.text-mid}"
    typography: "{typography.ui-label}"
    rounded: "{rounded.pill}"
    padding: 3px 10px
  scrubber-track:
    height: 6px
    backgroundColor: "{colors.track}"
    fillColor: "{colors.accent}"
    rounded: "{rounded.xs}"
  scrubber-knob:
    size: 16px
    sizeActive: 20px
    backgroundColor: "{colors.text}"
    rounded: "{rounded.full}"
    glow: "{elevation.focus-glow}"
  preview-popover:
    backgroundColor: "{colors.surface-float}"
    border: "1px solid {colors.hairline-float}"
    rounded: "{rounded.md}"
    shadow: "{elevation.float}"
    padding: "{spacing.sm}"
  title-bar:
    backgroundColor: "{colors.surface-raised}"
    borderBottom: "1px solid {colors.hairline}"
    textColor: "{colors.text-muted}"
    typography: "{typography.ui-label}"
    padding: 8px 14px
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.text}"
    typography: "{typography.ui-label}"
    rounded: "{rounded.md}"
    padding: 8px 16px
  page-image:
    rounded: "{rounded.xs}"
    shadow: "{elevation.page}"
  window:
    backgroundColor: "{colors.surface}"
    border: "1px solid {colors.hairline}"
    rounded: "{rounded.lg}"
  settings-panel:
    width: 360px              # Theme.settings-w
    height: content-hug       # header + body + footer; clamps to the window (Marcotte). φ outline dropped 2026-06-04 — φ moved into component proportions (toggle track, spacing ladder)
    rounded: 21px             # Theme.settings-radius = nav-pill-radius (shares NavBar's glass corner language)
    labelColumn: 132px        # Theme.settings-label-col (fixed; longest label ≈ 100px + slack, never wraps/elides)
    controlSeam: "labelColumn + {spacing.lg}"   # Theme.settings-control-x — fill controls START here; every control ENDS at the right rail
    rowHeight: 34px           # Theme.settings-row-h (= nav-capsule); the 30px control atom centers within it
    controlHeight: 30px       # Theme.settings-control-h — the control atom; centers within rowHeight
    rowGap: "{spacing.lg}"    # 14px ≈ Fib 13 — row pitch 48px ≈ controlHeight × φ
    sectionGap: "{spacing.xxl}" # 22px ≈ Fib 21; also the caption→footer-hairline gap
    segmentRadius: "{rounded.md}"          # Theme.radius-md capsule; cells = Theme.seg-cell-radius (md − 4px, concentric)
    toggleTrack: "controlHeight × φ"       # Theme.toggle-track-w ≈ 48.5×30 — Apple's 51×31 switch proportion; knob inset 2px
    scrollIndicatorWidth: 3px # Theme.settings-scroll-indicator-w (self-drawn, not a std scrollbar)
    dropdownWidth: 140px      # Theme.settings-dropdown-w (fixed so the pull-down capsule doesn't resize across languages)
    dropdownChevron: 10px     # Theme.settings-dropdown-chevron (the pull-down's chevron glyph square)
    sheenTop: "{colors.glass-sheen-top}"  # top stop of the panel fill gradient
    fill: "{colors.glass-fill}"           # bottom stop of the panel fill gradient
    border: "1px solid {colors.glass-border}"
    highlight: "{colors.glass-highlight}" # 1px top inner highlight
    shadow: "0 8px 22px {colors.shadow-popover}"  # ONE drop shadow; no second shadow, no nested glass
  shortcuts-overlay:
    width: 360px              # Theme.settings-w — REUSED, not a new token
    height: 466px             # Theme.shortcuts-h; fixed, smaller than the settings panel's content-hug height so it reads as a smaller modal stacked over settings
    rounded: 21px             # Theme.settings-radius — REUSED
    # All glass tokens reused from settings-panel above (sheenTop/fill/border/highlight/shadow); no second glass set.
---

## Overview

gashuu is a reading instrument, not a dashboard. Its design language is built on a single
idea: **the artwork is the interface, and everything else gets out of the way.** Two screens
carry the whole app — a **Library** (cover-flow carousel) and a **Viewer** (the page spread) —
and the chrome between them is deliberately thin and quiet.

The canvas is near-black (`{colors.canvas}` `#0b0e15`), not pure black: a faint blue-grey cast
keeps it from reading as a void and lets the page art sit warmly on top. Surfaces step up in a
tight, low-contrast ladder — canvas → window → raised chrome → floating popover — separated by
1px hairlines and soft shadows rather than bright dividers. There is **one accent**, a confident
blue (`{colors.accent}` `#5b8cff`), used for every interactive signal: the focused cover's ring,
the scrubber fill, the reading-progress fill, the primary button. A single green
(`{colors.success}` `#41c98a`) means one thing only — a book is fully read.

Type is intentionally muted. There is no display tier and no custom typeface; the system font
in four quiet sizes labels the chrome and counts the pages, and page numbers use tabular figures
so they don't jitter while scrubbing. The expressive surface is the cover and the page — the UI
is the dark frame around them.

**Key Characteristics:**
- Near-black reading canvas (`{colors.canvas}`) with a low-contrast surface ladder; depth via soft shadows + hairlines, never bright borders.
- A single blue accent (`{colors.accent}`) for ALL interactive state; a single green (`{colors.success}`) reserved for "read".
- Auto-hiding viewer chrome — page counter, library affordance, and scrubber appear on intent and fade away, so the page reads edge-to-edge.
- Cover-flow library: the focused book is large and ringed; neighbors recede via scale + reduced opacity (`{components.cover.sideOpacity}`).
- Reading progress is ambient — a thin bar under every cover, never a number the reader must hunt for.
- Quiet system-font type, tabular numerals for counts; the imagery, not the type, is the brand.

---

## Colors

> Source: the approved brainstorm mockups (cover-flow carousel, viewer scrubber, empty state).

### Accent
- **Accent** (`{colors.accent}` — `#5b8cff`): The only interactive color. Focus ring on the centered cover, scrubber fill, progress fill, primary button. If an element is interactive or "where you are", it is this blue.
- **Accent Glow** (`{colors.accent-glow}` — `rgba(91,140,255,0.25)`): The system's one "glow", appearing in two places — a 4px soft halo around the white scrubber knob, and the hover/press glow on the Library nav capsules (`components.nav-bar`). It is the SAME accent hue in both, not a second accent.
- **Success** (`{colors.success}` — `#41c98a`): Reserved exclusively for a fully-read book's progress bar. Never used for general UI.

### Surface (the ladder)
- **Canvas** (`{colors.canvas}` — `#0b0e15`): The viewer reading background; the deepest surface.
- **Surface** (`{colors.surface}` — `#0e1118`): The window body.
- **Surface Raised** (`{colors.surface-raised}` — `#161b27`): Title-bar / chrome strip.
- **Surface Float** (`{colors.surface-float}` — `#10141f`): Floating elements — the scrubber preview popover.
- **Surface Sunken** (`{colors.surface-sunken}` — `#0d1019`): The empty-library panel.
- **Stage Top** (`{colors.stage-top}` — `#1a2030`): The top of the carousel stage's vertical gradient (fades to `{colors.canvas}` at the bottom). In Slint, render as a `@linear-gradient`, not a radial.

### Hairlines & Tracks
- **Hairline** (`{colors.hairline}` — `#262c3a`): 1px chrome borders and dividers.
- **Hairline Float** (`{colors.hairline-float}` — `#2f3850`): 1px border on the floating popover.
- **Track** (`{colors.track}` — `#2a3243`): The scrubber rail.
- **Track Prog** (`{colors.track-prog}` — `#333c4f`): The unfilled portion of a progress bar.
- **Chip** (`{colors.chip}` — `#222a3a`): Pill/chip background (page counter, hints).

### Text (high → faint)
- **Text** (`{colors.text}` — `#ffffff`): Focused book title, primary emphasis.
- **Text High** (`{colors.text-high}` — `#dde5f5`): Strong reading text on dark.
- **Text Mid** (`{colors.text-mid}` — `#cdd8ef`): Chip text, secondary labels.
- **Text Muted** (`{colors.text-muted}` — `#9fb0cc`): Chrome labels, window title.
- **Text Dim** (`{colors.text-dim}` — `#7c8bab`): Hints (key reference lines).
- **Text Faint** (`{colors.text-faint}` — `#67748f`): Footnotes / least-important helper text.

### Window controls (platform chrome — not brand)
`{colors.win-close}` `#ff5f57` / `{colors.win-min}` `#febc2e` / `{colors.win-max}` `#28c840` are the
traffic-light dots shown in mockups. They represent OS window decorations and are **not part of
the gashuu palette** — do not reuse these hues anywhere in the UI.

---

## Typography

### Font Family
gashuu ships **no custom typeface**. UI text uses the platform **system font** (Slint's default
font; effectively San Francisco / Segoe UI / system-ui). The reading content is raster imagery,
so the font's only job is to label chrome quietly and count pages legibly. There is no display
tier, no mono, no second family.

### Hierarchy

| Token | Size | Weight | Line Height | Use |
|---|---|---|---|---|
| `{typography.ui-title}` | 15px | 700 | 1.3 | Focused book / current book title |
| `{typography.ui-body}` | 13px | 400 | 1.5 | General secondary text |
| `{typography.ui-label}` | 12px | 500 | 1.4 | Chips, chrome labels, button text |
| `{typography.ui-micro}` | 11px | 400 | 1.4 | Hints, captions, footnotes |
| `{typography.numeric}` | 13px | 600 | 1.3 | Page counters (`142 / 340`) — **tabular figures** |

### Principles
- **Quiet by default.** UI type is `{colors.text-muted}` or dimmer unless it names the focused/current item, which steps up to `{colors.text}`.
- **Tabular numerals for counts.** Page numbers must not reflow while scrubbing — set `font-variant-numeric: tabular-nums` (Slint: ensure the chosen font renders monospaced digits, or pad).
- **No display headlines.** The largest type is a 15px title. Emphasis comes from the cover art, not big letters.
- **Sentence case.** No all-caps in chrome; gashuu is calm, not loud.

---

## Layout

### Two screens
- **Library** — a horizontally-centered **cover-flow** row on a full-width stage (`{colors.stage-top}` → `{colors.canvas}` vertical gradient), with the focused book's meta centered below.
- **Viewer** — a centered page spread (single or double) on `{colors.canvas}`, with chrome absolutely positioned at the edges (top-left library affordance, top-right page counter, bottom scrubber) and **auto-hidden** during reading.

### Spacing System
- **Base unit**: 8px, with denser sub-steps for chrome.
- **Tokens**: `{spacing.xxs}` 4 · `{spacing.xs}` 6 · `{spacing.sm}` 8 · `{spacing.md}` 10 · `{spacing.lg}` 14 · `{spacing.xl}` 18 · `{spacing.xxl}` 22 · `{spacing.huge}` 26.
- Cover-flow gap ≈ `{spacing.lg}` 14px; chrome inset ≈ `{spacing.xl}`–`{spacing.huge}` from the window edge.

### Grid & Container
- No fixed content grid. The **carousel centers** on the focused cover; the **viewer centers** the spread. Both are width-responsive (see Responsive Behavior).
- Floating chrome (counter, library pill, scrubber preview) is positioned relative to the window edges / the knob, not a grid.

### Whitespace Philosophy
Whitespace is the dark canvas itself. The reader's eye should rest on art; empty space is the
quiet near-black around the page, not padded panels. Chrome claims the minimum and yields it back.

---

## Elevation & Depth

A flat, low-contrast world lifted by **soft shadows + 1px hairlines** — never by bright fills.

| Level | Token | Treatment | Use |
|---|---|---|---|
| 0 | `{elevation.flat}` | Flat | Canvas and window body |
| 1 | — (hairline) | 1px `{colors.hairline}`, no shadow | Title bar / chrome strip; progress tracks |
| 2 | `{elevation.card}` | `0 8px 22px rgba(0,0,0,.55)` | Book covers in the carousel |
| 2 | `{elevation.page}` | `0 6px 18px rgba(0,0,0,.50)` | Page images in the viewer |
| 3 | `{elevation.float}` | `0 10px 30px rgba(0,0,0,.55)` | Scrubber preview popover & window |
| focus | `{elevation.focus-glow}` | `0 0 0 4px {colors.accent-glow}` | Scrubber knob ring |

The **focused cover** reads as elevated not by shadow alone but by **scale + full opacity + the
accent ring**, while neighbors drop to `{components.cover.sideOpacity}` 0.45 and slightly smaller
scale. No blur effects; depth is shadow, scale, and opacity only.

> **Slint note:** shadows map to `Rectangle`'s `drop-shadow-blur` / `drop-shadow-color` /
> `drop-shadow-offset-*`. There is no CSS `box-shadow` spread; approximate the soft look with
> blur ≈ 18–22px and a high-alpha black. Reduced-opacity neighbors use `opacity:`.

---

## Shapes

### Border Radius Scale

| Token | Value | Use |
|---|---|---|
| `{rounded.xs}` | 3px | Page images, scrubber rail, thumbnail cells |
| `{rounded.sm}` | 6px | Book covers |
| `{rounded.md}` | 8px | Preview popover, primary button |
| `{rounded.lg}` | 10px | Window frame |
| `{rounded.pill}` | 9999px | Chips / page-counter pills |
| `{rounded.full}` | 9999px | Scrubber knob (circle) |

### Imagery Geometry
Covers and pages lead with their native aspect ratio; gashuu never crops art to a fixed frame
(letterbox/pillarbox instead). The three thumbnail surfaces keep **consistent rounding**: page
images and thumbnail cells at `{rounded.xs}` 3px, library covers at `{rounded.sm}` 6px.

---

## Components

### Book Cover (carousel item) — `components.cover`
The signature component. Rounded `{rounded.sm}`, shadow `{elevation.card}`. **Focused** state:
full opacity, larger scale, `3px solid {colors.accent}` outline offset 3px. **Neighbor** state:
opacity `{components.cover.sideOpacity}` 0.45 and slight desaturation + smaller scale.
**Unavailable** state (missing file): grayed/dimmed with a broken-cover placeholder — the book
stays in the shelf with its reading position intact.

### Reading Progress Bar — `components.progress-bar`
A 4px ambient bar (rounded 2px) directly under every cover and under the focused book's meta.
Track `{colors.track-prog}`, fill `{colors.accent}`. When a book is fully read, the fill switches
to `{colors.success}` — the one place green appears.

### Chip / Pill — `components.chip`
Background `{colors.chip}`, text `{colors.text-mid}`, `{typography.ui-label}`, padding 3×10,
`{rounded.pill}`. Used for the page counter (`142 / 340`, numeric token), the "↑ Library"
affordance, and key hints.

### Scrubber — `components.scrubber-track` + `components.scrubber-knob`
A bottom rail (6px, `{colors.track}`, `{rounded.xs}`) spanning the window minus edge insets. The
traversed portion fills with `{colors.accent}` (Apple HIG's defining slider trait), reading-direction
aware: in RTL (manga) the fill grows from the screen-right edge toward the knob, in LTR from the left.
The thumb is a 16px white handle (`{colors.text}`) carrying the `{elevation.focus-glow}` accent halo —
accent reads as progress (the fill), white reads as the grabber — and grows to 20px while dragging.
**RTL-aware**: in manga (right-to-left) reading, dragging **left advances** the page, consistent with
the direction-aware key bindings. The scrubber is **auto-hidden**; it fades in on mouse-move / arrow /
drag and out after a short idle.

### Scrubber Preview Popover — `components.preview-popover`
Appears directly above the knob **only while dragging**. Background `{colors.surface-float}`, 1px
`{colors.hairline-float}`, `{rounded.md}`, shadow `{elevation.float}`, with a small downward caret.
Shows **1 thumbnail (single) or 2 (double)** per the active spread layout, plus `p.X–Y / N` in the
numeric token. **During drag the page body does not change** — only the popover and counter update;
the page commits on release. Thumbnails are pulled from the existing page-thumbnail set (no new decode).

### Thumbnail Failed State — colors PROPOSED (needs sign-off)
When a thumbnail/page fails to decode, the cell uses a desaturated-red treatment: surface `{colors.error-surface}` (#2a1820) with a `{colors.error}` (#d16b7c) 1px border and glyph. These two hues are **not yet in the DESIGN palette** — they are proposed additions tuned to the dark canvas and deliberately distinct from the forbidden traffic-light close (`{colors.win-close}` #ff5f57). Pending design sign-off.

### Title Bar — `components.title-bar`
Background `{colors.surface-raised}`, 1px bottom `{colors.hairline}`, `{typography.ui-label}` in
`{colors.text-muted}`, with the document/library name centered and a count chip on the right.

### Primary Button — `components.button-primary`
Background `{colors.accent}`, white text, `{rounded.md}`, padding 8×16. The empty-library
call-to-action ("Add books") and other affirmative actions.

### Library Nav — `components.nav-bar`
A **top, centered glass pill** floating over the Library carousel for adding books. It is drawn
on top of the stage; its bottom edge may slightly overlap the focused cover so the background
shows through — reinforcing the "glass" read.

- **Content**: ICON-ONLY twin capsules — `file` (Add files) and `folder` (Add folder). There are
  **no on-screen text labels** (accessible-label only) and **no tooltips**.
- **Each capsule** is circular; only the hovered/pressed cell glows softly with
  `{colors.accent-glow}`, and its icon brightens `{colors.text-mid}` → `{colors.text-high}`.
- **Glass treatment**: `{colors.glass-fill}` fill + a 1px `{colors.glass-border}` rim + a 1px
  `{colors.glass-highlight}` top inner highlight line + a `{colors.shadow-popover}` drop shadow
  (blur 22 / y-offset 8). **No backdrop-blur** — Slint 1.x cannot blur what's behind it, so the
  glass is faked with translucent fill + rim + top highlight + shadow.
- **Golden-ratio sizing** (phi ≈ 1.618, stepped through consecutive Fibonacci px): icon 21px →
  circular capsule diameter 34px → pill height 55px; item gap 13px; pill padding 11px.
- **Interaction model**: mouse + screen-reader oriented. The pill is **NOT keyboard-reachable** —
  keyboard navigation stays owned by the carousel. Clicking a capsule fires the OS file/folder
  picker (`rfd`) and returns focus to the carousel.
- **Intentional deltas from the reference**: gashuu uses its own dark translucency (not blue
  glass), and **FILLED** icons (not outline) — a filled mass reads better on the dark canvas and
  resists low-DPI degradation.

### Settings Panel — `components.settings-panel`
A modal **content-hug glass panel** centered over the dimmed screen: 360px wide, exactly as tall as
its header + body + footer (the fixed φ outline was deliberately dropped 2026-06-04 — **φ relocated
into the component proportions**: the toggle track ratio, the 8/14/22 ≈ Fibonacci 8/13/21 spacing
ladder, and the segment padding), corner radius **21px** (= `nav-pill-radius` — it shares NavBar's
glass corner language). It is one
fake-glass object built from NavBar's four layers, with **layer 1 promoted to a top-sheen gradient**:
a `@linear-gradient(180deg, {colors.glass-sheen-top} 0%, {colors.glass-fill} 46%)` fill, a 1px
`{colors.glass-border}` rim, a 1px `{colors.glass-highlight}` top inner highlight, and ONE
`{colors.shadow-popover}` drop shadow (blur 22 / y-offset 8). No nested glass, no second shadow. (The sheen is a FILL gradient, not
an opacity layer — Slint `opacity` blurs text/SVG on HiDPI.) On a short window the panel height clamps
to fit and the **body scrolls** (see Responsive Behavior).

- **Seam + right-rail alignment**: each setting is a row with a fixed **label column** (132px,
  `{colors.text-mid}`, never wrapping/eliding) at the left margin. Rule: **every control ENDS at the
  right rail** (the body's right padding edge); **fill controls (Segmented) also START at the seam**
  (`labelColumn + {spacing.lg}`) with equal-width cells (HIG), while compact controls
  (Stepper — width-equalized — Toggle, and the Language pull-down) trail on the rail (macOS System
  Settings). Row height 34px; the 30px control atom centers within it; row pitch 48px ≈
  controlHeight × φ.
- **Sections**: Reading / Display / Performance / General, delineated by whitespace (22px ≈ Fib 21).
  Section headers are `{colors.text-dim}` **sentence-case semibold eyebrows** — smaller than the row
  labels on purpose (Apple grouped-list IA: hierarchy via position/whitespace/color, weight marks the
  header species) — NOT accent (accent stays interactive/selected-only).
- **Footer**: both-ends (HIG) — "⌨ Shortcuts" on the left edge, (Reset to global +) Close hard
  right, all on one shared vertical centerline; `{spacing.xl}`/`{spacing.md}` (18/10 ≈ φ) padding.
- **Toggle** is an Apple-proportioned switch: capsule track `controlHeight × φ` wide, 26px knob,
  2px inset. **Segmented** capsules are `{rounded.md}` with concentrically rounded cells.
- **Controls** are the token-driven atoms (`Segmented` / `Stepper` / `Toggle` / `Dropdown`), not std
  widgets.
- **Language pull-down** (`Dropdown`, Apple-HIG pull-down button): a fixed-width capsule
  (`dropdownWidth`) on the right rail showing the current value plus a `{colors.text-dim}`
  chevron; the open menu (a Slint `PopupWindow` — never clipped by the scroll body) lists options
  with an `{colors.accent}` check mark on the selected row and an accent hover fill. Language
  names always render in their own tongue ("English" / "日本語") and are never translated.
- **Scrollable body** is a Slint `Flickable` (NOT a std `ScrollView`, whose light scrollbar breaks the
  glass) with a thin **self-drawn scroll indicator** (3px `{colors.track-prog}` rail + `{colors.accent}`
  thumb) shown only on overflow.
- **Dismiss**: Esc, a backdrop click (the dimmed scrim outside the panel), or the Close button — all
  three close the dialog.

### Shortcuts Overlay — `components.shortcuts-overlay`
A second modal glass panel that lists the keyboard shortcuts read-only, reached from the settings
panel's **"⌨ Shortcuts"** footer link. It stacks **ON TOP of the still-open Settings Panel** (a layer,
not a replacement). It clones the settings glass recipe EXACTLY — same `{components.settings-panel.width}`
(360px) and `{components.settings-panel.rounded}` (21px), the same one-fake-glass-object build (top-sheen
gradient fill + 1px rim + 1px top inner highlight + ONE `{colors.shadow-popover}` shadow), the same `Flickable`
body + self-drawn scroll indicator. There is NO second glass token set; only the height differs.

- **Layered sizing**: the panel is **466px** tall (`Theme.shortcuts-h`), deliberately SHORTER than the
  settings panel's content-hug height, so the two panels read as a stack — a smaller modal floating over a
  larger one — rather than one swapping for the other. It is sized to fit the shortcuts text (17 lines at
  `{typography.ui-micro}`) plus a sticky header and a hairline footer with the Close button; on a short
  window it clamps and the body scrolls (same Marcotte clamp as the settings panel).
- **Double scrim (intended)**: the overlay draws its OWN full-area scrim over the settings dialog's scrim,
  so the screen behind dims a second time. The compounded dim is the signal that this is a modal over a
  modal, not an error.
- **Dismiss**: Esc, a backdrop click, or the Close button — all three close ONLY the overlay and return
  keyboard focus to the still-open settings panel underneath (never to the screen behind).

### Empty Library (0 books)
The Library screen, when empty, centers a single **`button-primary`** ("Select folders / files to
add") on a `{colors.surface-sunken}` panel, with a one-line `{typography.ui-micro}` helper. Books
are added via the OS file/folder picker (`rfd`). **There is no drag-and-drop drop zone** — file
loading is picker-only.

Once books exist, the top centered glass-pill nav (`components.nav-bar`, icon-only Add files /
Add folder) sits above the carousel for adding more books — the picker-only, no-drag-drop rule is
unchanged.

---

## Do's and Don'ts

### Do
- Let the cover and page art lead; keep the dark frame quiet around them.
- Use `{colors.accent}` for every interactive/"you are here" signal, and nothing else.
- Reserve `{colors.success}` strictly for the fully-read progress state.
- Auto-hide viewer chrome so the page reads edge-to-edge; bring it back on intent.
- Build depth from soft shadows, scale, and opacity; separate chrome with 1px hairlines.
- Use tabular figures for page counters so they don't jitter while scrubbing.
- Keep the three thumbnail surfaces' rounding consistent (`{rounded.xs}` pages, `{rounded.sm}` covers).

### Don't
- Don't add a second accent hue — the single blue is load-bearing.
- Don't use the traffic-light window-control colors anywhere in the UI.
- Don't put persistent heavy chrome over the reading area, or change the page during a scrub drag.
- Don't crop art to a fixed frame — letterbox/pillarbox instead.
- Don't introduce a light mode (out of scope; gashuu is dark-only).
- Don't ship a drag-and-drop drop zone — loading is file-picker only.
- Don't bring styling into `gashuu-core` — it stays headless (RGBA bytes + dimensions only).

---

## Responsive Behavior

gashuu is a resizable desktop window, not a breakpoint-driven web page. "Responsive" means
adapting to the live window size.

### Window-size adaptation
- **Spread auto-layout**: the existing `Auto` spread mode resolves single vs double from the
  window aspect ratio (landscape/square → double, portrait → single) and follows live resizes.
  This is the primary responsive behavior and the visual system must compose with it.
- **Carousel**: the number of visible neighbor covers grows/shrinks with width; the focused cover
  stays centered. Below a minimum width, neighbors may drop to one per side.
- **Scrubber**: the rail spans window width minus edge insets; the preview popover clamps inside
  the window so it never clips at the far edges.
- **Showing the thumbnail strip** shrinks the viewer height and may re-resolve `Auto` (accepted).
- **Settings panel**: keeps its fixed 360px width and hugs its content vertically; once the window gets
  short, the Marcotte clamp caps its height to the window minus a gutter on each side and the body scrolls
  (the sticky header/footer stay put). Never overflows the window.
- **Shortcuts overlay**: same fixed-then-clamp behavior as the settings panel (its 466px height clamps to
  the window minus a gutter on each side, then the body scrolls), one layer above it.

### Targets & minimums
- Interactive targets (knob, covers, buttons) stay ≥ ~32px effective hit area.
- Define a sensible minimum window size so the double spread + chrome remain usable.

---

## Iteration / Agent Prompt Guide

This document is read by coding agents implementing gashuu's Slint UI. To use it:

1. **One surface at a time.** Build/refine a single component (e.g., the scrubber) before moving on.
2. **Reference tokens, not raw hex.** Centralize these tokens in a single Slint `global Theme {
   ... }` (colors, spacing, radii, font sizes) and reference `Theme.accent`, etc. — never paste
   `#5b8cff` inline. Treat this file as the source of truth for that global.
3. **Slint, not CSS.** Map shadows to `drop-shadow-*`, gradients to `@linear-gradient`, opacity to
   `opacity:`. Where a token's note flags a Slint difference, follow the note.
4. **Respect the headless boundary.** `gashuu-core` carries no styling; all theme/visual concerns
   live in the `gashuu` (Slint) crate. The core↔UI contract is RGBA bytes + dimensions.
5. **The single-accent rule is load-bearing.** Adding a second accent, a light mode, or a
   drag-and-drop drop zone breaks the system — raise it as a design change, don't just add it.
6. **Consistency across the three thumbnail surfaces** (page strip, scrubber preview, library
   covers): shared rounding, shared shadow tokens, shared loading/failed/placeholder treatment.

> This DESIGN.md is gashuu's own visual design system (not an analysis of a third-party brand).
> It is the **independent, standalone** reference for look-and-feel; engineering structure,
> data model, and PR decomposition live in `docs/superpowers/specs/`.
