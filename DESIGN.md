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
  search-border-rest: "#2f385026" # hairline-float at ~15% alpha — resting search-field rim, threshold-of-visibility
  glass-sheen-top: "#1a2030d1" # stage-top at ~82% alpha — top stop of the settings panel's fill gradient
  shadow-popover: "#00000080"  # Theme.shadow-popover — popover/panel drop-shadow ink (50% black)
  scrim: "#000000a0"           # Theme.scrim — standard modal backdrop (~63% black); used by settings / confirm / shortcuts overlays
  scrim-soft: "#00000060"      # Theme.scrim-soft — lighter recede veil (~38% black); scrim is too dense for a NavBar recede; used by the NavBar recede in selection mode
  success: "#41c98a"
  # Destructive-operation red (bulk-delete epic). Red is scarce — destructive
  # BUTTONS only; selection visuals stay accent. Distinct from the STATE-semantics
  # error/error-surface below (which mark a failed decode/load, not an action).
  danger: "#c1455e"           # filled destructive-button ground; white label ≈ 4.91:1 (WCAG AA pass)
  danger-glow: "#c1455e40"    # danger at ~25% alpha — hover/focus ring (symmetric to accent-glow); its own value, never an alias of error
  # STATE semantics: failed decode/load surface + hue. Tuned to the dark canvas,
  # distinct from the forbidden traffic-light close (win-close #ff5f57). NOT for
  # destructive buttons (white-on-error ≈ 3.44:1, fails AA — use danger instead).
  error: "#d16b7c"            # failed-decode border + glyph hue
  error-surface: "#2a1820"    # failed-decode cell ground
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
  # pill/full are for CIRCLES (square elements) only. A text capsule uses
  # height/2 (Theme.nav-capsule-radius for the 34px atom): the renderer clamps
  # the radius PER AXIS, so 9999px on a non-square rect renders an ELLIPSE,
  # not a capsule (docs/patterns.md).
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
    padding: 8px 16px         # vertical 16px total split 9 top / 7 bottom — optical centering (metric centering sits the label ~1px high)
  button-danger:
    backgroundColor: "{colors.danger}"   # destructive red, not accent — red is scarce, buttons only
    textColor: "{colors.text}"
    typography: "{typography.ui-label}"
    rounded: "{rounded.md}"
    padding: 8px 16px         # same geometry as button-primary (9/7 optical-centering split)
    glow: "{colors.danger-glow}"  # hover/focus ring (drop-shadow), symmetric to the accent glow
    icon: optional            # leading glyph, 16px (Theme.button-icon), colorize {colors.text};
                              # SUPPLEMENTS the label, never replaces it (destructive-label safety
                              # requirement) — the toolbar delete button passes delete.svg
  # bulk-delete epic — selection chrome. Selection is ALWAYS accent
  # (blue). Red/danger NEVER appears in selection chrome — the delete DangerButton
  # in SelectionToolbar and the ConfirmDialog confirm button are the only red elements.
  selection-badge:
    # Two-state atom; both states share an IDENTICAL footprint (space-huge) — zero size/position jump on toggle.
    # checked=true:  accent disc + white check glyph (the "selected" state)
    # checked=false: hollow ring — glass-fill backing (legible over bright cover art),
    #                1px text-mid border, no glyph (the "unselected but in selection mode" affordance)
    # Badge shows on ALL covers while selection mode is active — the simultaneous ring appearance
    # is the mode-changed signal. It is absent in normal mode.
    size: "{spacing.huge}"    # 26px footprint, radius-pill (full circle)
    checkSize: "{spacing.lg}" # 14px check.svg glyph (checked=true only), colorize: text (white), centered
    backgroundColor: "{colors.accent}"   # checked=true ground
    uncheckedBackground: "{colors.glass-fill}"  # checked=false backing (for legibility over art)
    uncheckedBorder: "1px solid {colors.text-mid}"  # checked=false ring
    checkColor: "{colors.text}"
  selection-toolbar:
    # Glass pill — shares NavBar's four-layer glass recipe exactly.
    height: "{nav-pill-height}"      # 55px (golden-ratio nav capsule × φ)
    width: content-hug               # intrinsic preferred width — MUST be a layout child
                                     # (Carousel wraps it in a centering HorizontalLayout);
                                     # absolutely positioned it defaults to the PARENT's width
    rounded: "{nav-pill-radius}"     # 21px (consecutive-Fibonacci nav glass corner)
    fill: "{colors.glass-fill}"
    border: "1px solid {colors.accent}"     # mode-context differentiation: accent (not glass-border) signals the active mode
    highlight: "{colors.glass-highlight}"   # 1px top inner rim
    shadow: "0 8px 22px {colors.shadow-popover}"
    # Count pill (mode indicator, left cell)
    countPillBackground: "{colors.accent-glow}"
    countPillBorder: "1px solid {colors.accent}"
    # countPillGlow removed — was a "triple-accent" rhythm (accent border + accent-glow fill + drop-shadow all at once)
    countPillTextColor: "{colors.accent}"
    countPillTypography: "{typography.ui-label}"
    countPillRounded: "nav-capsule / 2"          # 17px TRUE capsule (Theme.nav-capsule-radius);
                                                 # rounded.pill on a non-square rect = ellipse
    countPillPadding: "{nav-capsule-pad}"        # 21px (= nav-capsule/φ, Fibonacci) HIG label inset
    # Select-all/deselect-all capsule (center cell)
    # Idle: HIG "bordered" — chip fill + hairline ring (a button reads as a button at rest);
    # hover: accent-glow fill + accent border ring; pressed: darker
    selectAllBackground: "{colors.chip}"         # idle ground
    selectAllBorder: "1px solid {colors.hairline-float}"  # idle ring
    selectAllTextColor: "{colors.text-mid}"      # idle
    selectAllHoverTextColor: "{colors.text-high}"
    selectAllHoverBackground: "{colors.accent-glow}"
    selectAllHoverBorder: "1px solid {colors.accent}"
    selectAllRounded: "nav-capsule / 2"          # 17px TRUE capsule (Theme.nav-capsule-radius)
    selectAllPadding: "{nav-capsule-pad}"        # 21px (= nav-capsule/φ, Fibonacci) HIG label inset
    selectAllNarrowBreakpoint: 560px             # collapses to check.svg icon-only ≤ this width
    # Delete cell (button-danger atom) — leading trash glyph next to the always-on label
    deleteIcon: delete.svg                       # 16px, colorize {colors.text} (see button-danger.icon)
    # Exit capsule (right cell, circular nav-capsule diameter)
    exitSize: "{nav-capsule}"        # 34px circle
    exitIcon: close.svg              # @image-url("assets/close.svg") + colorize (Cancel Fill — filled disc
                                     # with knocked-out ✕, 96px intrinsic / viewBox 24); sized Theme.nav-icon;
                                     # bare-✕ fallback remains an open author visual-check (disc-in-capsule legibility)
    exitTextColor: "{colors.text-mid}"
    exitHoverTextColor: "{colors.text-high}"
    exitHoverBackground: "{colors.accent-glow}"
    exitHoverBorder: "1px solid {colors.accent}"
  selection-entry-pill:
    # Mouse-only entry point into selection mode (normal mode, covers visible).
    height: "{nav-capsule}"          # 34px — shorter than the toolbar; same capsule atom
    width: content-hug               # label preferred-width + nav-capsule-pad × 2
    rounded: "nav-capsule / 2"       # 17px TRUE capsule (Theme.nav-capsule-radius)
    # Idle: HIG "bordered" — chip fill + hairline ring (reads as a button at rest)
    # Hover: accent-glow fill + accent border ring; pressed: darker
    backgroundColor: "{colors.chip}"
    border: "1px solid {colors.hairline-float}"
    textColor: "{colors.text-mid}"
    hoverTextColor: "{colors.text-high}"
    hoverBackground: "{colors.accent-glow}"
    hoverBorder: "1px solid {colors.accent}"
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
    segmentRadius: "{rounded.md}"          # Theme.radius-md capsule; selected pill = Theme.seg-cell-radius (md − seg-pill-inset = 5px — consecutive-Fibonacci 3/5/8 concentric: inset → pill radius → capsule radius)
    toggleTrack: "controlHeight × φ"       # Theme.toggle-track-w ≈ 48.5×30 — Apple's 51×31 switch proportion; track corner = height/2 TRUE capsule (radius-pill would render an ellipse on the non-square track); knob inset 2px; off track {colors.track-prog} (visible on the glass), knob carries a subtle {colors.shadow-popover} depth shadow
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
  confirm-dialog:
    width: 360px              # Theme.settings-w — REUSED; fluid-width clamp: min(settings-w, parent − 2 × space-xl)
    rounded: 21px             # Theme.settings-radius — REUSED (same NavBar glass corner language)
    # Glass tokens all reused from settings-panel (sheenTop/fill/border/highlight/shadow); no new token set.
    # Body layout tokens (spacing, typography) also reused from settings-panel.
    cancelFocus: true         # Cancel holds default focus on open (init + changed focus-epoch)
    confirmGround: "{colors.danger}"   # destructive confirm uses DangerButton; neutral uses PrimaryButton (toggled by `danger` prop)
    infoBandBackground: "{colors.surface-raised}"  # neutral reassurance band
    warningTextColor: "{colors.danger}"            # open-book caution line
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
- **Search Border Rest** (`{colors.search-border-rest}` — `#2f385026`): The library search field's resting 1px rim — the hairline-float hue held at threshold-of-visibility (~15% alpha), so the field is barely outlined until it brightens to `{colors.accent}` on focus.
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

### Scrims & Shadows
- **Scrim** (`{colors.scrim}` — `#000000a0`): Standard modal backdrop (~63% black). Drawn full-area behind settings, shortcuts-overlay, and confirm-dialog panels.
- **Scrim Soft** (`{colors.scrim-soft}` — `#00000060`): Lighter recede veil (~38% black). Used by the NavBar recede in selection mode — `scrim` at full weight is too dense for a veil over an interactive control (the search field and nav capsules stay usable through it).
- **Shadow Popover** (`{colors.shadow-popover}` — `#00000080`): Drop-shadow ink for floating popovers and glass pills (blur 22px / y-offset 8px). 50% black — heavier than the glass-fill alpha, so the pill reads as lifted without needing a bright border.

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
stays in the shelf with its reading position intact. **Selection-mode hover ring** (in selection
mode only): hovering any cover shows a 1px `{colors.accent}` ring — a pointer hint that the cover
is a selectable target; the focused cover's 3px ring is unchanged. Normal mode shows no ring on
hover.

Cover overlays occupy two distinct corners and can render simultaneously:

- **BookmarkRibbon** (top-LEFT): a display-only bookmark-shape image (`bookmark.svg`, `colorize:
  {colors.text}` — white — sized `{spacing.huge}²` = 26×26 px) that floats FULLY ABOVE the cover's
  top edge. Clearance = `{spacing.md}` (10px ≈ `{spacing.huge}` / φ², the nav-search-radius
  derivation); zero overlap with the cover art. The area above the card is the dark stage gradient
  (`{colors.stage-top}` → `{colors.canvas}`), so the white glyph keeps contrast regardless of cover
  art — the reason the ribbon sits outside the card. It appears on the single book whose path equals
  `Library.last_opened` — the "continue reading" signifier that explains why that cover is
  automatically focused when entering the Library. White is a passive status color borrowed from the
  typography token ladder; the accent returns to interactive-only duty. Shape is the semantic
  differentiator: the ribbon form (as opposed to the circular SelectionBadge) specifically means
  "resume here". If the ribbon ever needs its own hue, `colorize: {colors.text}` in
  `BookmarkRibbon.slint` is the documented re-mint point.
- **SelectionBadge** (top-RIGHT): the bulk-selection check mark. Occupies the opposite corner so
  both overlays can be visible at the same time without overlapping. The ribbon now sitting OUTSIDE
  the card further improves their separation — the badge is inside top-right, the ribbon outside
  top-left.

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

### Thumbnail Failed State — `{colors.error}` / `{colors.error-surface}` (accepted)
When a thumbnail/page fails to decode, the cell uses a desaturated-red treatment: surface `{colors.error-surface}` (#2a1820) with a `{colors.error}` (#d16b7c) 1px border and glyph. These two hues are **STATE semantics** — they mark a failed decode/load, not an interactive action — tuned to the dark canvas and deliberately distinct from the forbidden traffic-light close (`{colors.win-close}` #ff5f57). They are **NOT** for destructive buttons: white label text on `{colors.error}` is only ≈ 3.44:1, below the WCAG AA floor — destructive buttons use the deeper `{colors.danger}` instead.

### Title Bar — `components.title-bar`
Background `{colors.surface-raised}`, 1px bottom `{colors.hairline}`, `{typography.ui-label}` in
`{colors.text-muted}`, with the document/library name centered and a count chip on the right.

### Primary Button — `components.button-primary`
Background `{colors.accent}`, white text, `{rounded.md}`, padding 8×16 (the vertical 16px is
split 9 top / 7 bottom — the optical-centering nudge shared with the segmented labels). The empty-library
call-to-action ("Add books") and other affirmative actions.

### Danger Button — `components.button-danger`
The destructive-action counterpart of the primary button (used by the bulk-delete epic for
"Delete N book(s)" and similar). Structurally identical to `components.button-primary` — white text,
`{rounded.md}`, the same 8×16 padding (9 top / 7 bottom optical-centering split) — but its ground is the
deeper destructive red `{colors.danger}` (#c1455e) rather than `{colors.accent}`, so the white label clears
the WCAG AA contrast floor (≈ 4.91:1) on the dark canvas. On hover/press the ground darkens and a
`{colors.danger-glow}` ring lights up (a drop-shadow glow symmetric to the accent glow). **Red is scarce:**
reserve this button — and the `danger` hue — for destructive actions only; selection and "you are here"
visuals stay `{colors.accent}`.

### Selection Badge — `components.selection-badge`
A two-state atom overlaid on a cover while selection mode is active (bulk-delete epic; two-state added in the UI-polish pass). An atom — no interactive state, no callback. Rendered inside `CoverCard` at the top-right corner, inset `{spacing.xs}` from the edges. **Both states share an identical `{spacing.huge}` 26px footprint (`{rounded.pill}`, full circle) — toggling `checked` causes zero size/position jump.**

- **`checked=true` (selected):** `{colors.accent}` disc + `check.svg` colorized to `{colors.text}` (white), `{spacing.lg}` 14px, centered.
- **`checked=false` (unselected, but in selection mode):** `{colors.glass-fill}` backing (for legibility over bright cover art), 1px `{colors.text-mid}` border ring, no glyph.

The badge shows on **every cover** while selection mode is active — the simultaneous hollow-ring appearance across all covers is the mode-changed signal. In normal mode the badge is absent (`if`-gated on `selection-mode`). **Red is reserved for destructive actions** (the `SelectionToolbar` DangerButton and the `ConfirmDialog` confirm button) — this badge is strictly accent.

### SelectionToolbar — `components.selection-toolbar`
An organism shown **below the NavBar**, centered, only while selection mode is active and no modal is open (bulk-delete epic; slide transition added in the UI-polish pass). Not in the keyboard focus chain; mouse + screen-reader only (keyboard navigation stays carousel-owned).

**Slide transition** (§2.6): the SelectionToolbar slides via a vertical y-slide into and out of a `clip: true` strip (`{nav-pill-height}` band) anchored at `nav.y + nav.height + {nav-item-gap}` (13px). The toolbar slides between `y=0` (visible) and `y=-height` (tucked under the NavBar, clipped away); `animate y` at `motion-fast`. The strip hosts **only** the SelectionToolbar — the former Select entry pill has been removed and selection entry now lives in the NavBar itself. **No `opacity` is used anywhere** (HiDPI child-blur gotcha — see `docs/patterns.md`); the reveal is pure geometry plus the NavBar recede veil. The toolbar's input guard (`active` flag) ensures a slid-away toolbar never takes pointer/screen-reader input even if clipping leaks.

**Glass pill**: the NavBar's four-layer glass idiom — `{colors.glass-fill}` background, 1px `{colors.accent}` rim (mode-context differentiation: accent rim rather than `glass-border` signals the active selection mode), 1px `{colors.glass-highlight}` top inner highlight, `{colors.shadow-popover}` drop shadow (blur 22 / y-offset 8; suppressed to `transparent` while the toolbar is parked/slid away — a parked toolbar's offset-down shadow would bleed into the visible strip through the `clip: true` band, so the shadow is active-gated and drawn only while this bar is the visible one) — at `{nav-pill-height}` (55px) height. Width **hugs content** via the root's intrinsic preferred width, which only resolves when the pill is a **layout child**: Carousel wraps it in a full-width centering `HorizontalLayout` (absolutely positioned, a Slint element defaults to its parent's width and the pill spans the window). Still no binding-loop risk — no expression reads the layout's own preferred width.

**Left → right contents** (gap = `{nav-item-gap}`, padding = `{nav-pill-pad}`):

1. **Count pill** (mode indicator): a TRUE capsule — corner radius `nav-capsule / 2` (17px; `{rounded.pill}` would render an ellipse on a non-square rect) — with `{colors.accent-glow}` background, 1px `{colors.accent}` border, and a `{nav-capsule-pad}` (21px = capsule/φ) label inset. Text is `{typography.ui-label}` in `{colors.accent}`, with a +1px downward optical-centering nudge (metric line-box correction — descender-less labels read high without it). **When 0 books are selected**, the pill shows the mode label (`selection-mode-label` ftl key, e.g. "Selection mode" / "選択モード") rather than a count — the zero-count form carries no digit. From 1+ books the Rust-composed count string is shown (e.g. "3 selected" or "5 selected (2 outside search)"). Read-only (not a button). **Never red/danger** — selection language is accent. No drop-shadow glow on the count pill (the accent border + accent-glow fill is the rhythm; a third accent layer would produce a "triple-accent" visual).

2. **Select-all / deselect-all capsule**: TRUE capsule (`nav-capsule / 2` corners), `{nav-capsule}` (34px) height, width = measured label + `{nav-capsule-pad}` (21px) per side. Idle: HIG "bordered" — `{colors.chip}` fill + 1px `{colors.hairline-float}` ring, so it reads as a button at rest. Hover: `{colors.accent-glow}` fill + 1px `{colors.accent}` border + accent drop-shadow glow. Pressed: slightly darker fill + accent ring. Label text `{colors.text-mid}` at idle, brightening to `{colors.text-high}` on hover/press — the `NavItem` icon idiom. At **≤ 560px** window width (the `narrow` breakpoint), collapses to `check.svg` icon-only form (same `colorize` idiom as `SelectionBadge`); the full label is always the `accessible-label`. Fires the `select-all()` callback (Rust decides select-vs-deselect); also triggered by Cmd/Ctrl+A in the carousel `FocusScope`.

3. **Delete DangerButton — the ONLY red element in the app's chrome**: a leading `delete.svg` trash glyph (16px, colorized `{colors.text}`) next to the label "Delete (N)…" (Rust-composed with the exact selection count). `{colors.danger}` ground — the only place `danger` appears in the toolbar; all other cells are accent. **Hidden at N=0** (`if`-gated, not `disabled`) — zero layout cost and zero target for an empty selection. The label NEVER collapses in narrow mode (the icon supplements it, never replaces it): hiding a destructive control's label at any width is a safety hazard and was explicitly rejected in the spec. Fires `request-delete()` (Rust opens the `ConfirmDialog`).

4. **Exit capsule**: circular `{nav-capsule}` (34px) × `{nav-capsule}`, `{rounded.pill}`. Same hover/press states as the select-all capsule. Icon is `close.svg` (Streamline "Cancel Fill" — filled disc with knocked-out ✕, 96px intrinsic / viewBox 24) rendered via `@image-url` + `colorize`: idle `{colors.text-mid}`, hover/press `{colors.text-high}`, sized `Theme.nav-icon` (21px). Bare-✕ fallback remains an open author visual-check decision (disc-in-capsule legibility). Fires `exit()` — equivalent to pressing Esc in the carousel.

### Select Entry — NavBar capsule (formerly: Select Entry Pill)
Selection mode is entered via the **Select capsule inside the NavBar** (the `filter.svg`
`NavItem` — see Library Nav above). The separate text pill that formerly lived in the slide-strip
below the NavBar has been removed; it no longer exists as a component or a spec entry. The
`components.selection-entry-pill` token block in the front-matter above is retained for historical
reference (it describes the removed pill's look) but has no live consumers.

The slide-strip below the NavBar now hosts **only** the `SelectionToolbar`. Exit paths for
selection mode are: the toolbar ✕, Esc, and re-clicking the NavBar Select capsule.

**Placement rule (toolbar)**: the toolbar is suppressed while any modal (settings / shortcuts /
first-run guide) is open — the `!modal-open` guard mirrors the carousel `FocusScope`'s
modal-reject arm so pointer targets under a modal are always unreachable.

### ConfirmDialog — `components.confirm-dialog`
A reusable two-choice modal (issue 127, consumed by the bulk-delete epic). Generic: carries NO domain vocabulary — every word arrives through properties so the same component serves any confirm-or-cancel decision.

**Glass panel**: clones the `SettingsDialog` / `ShortcutsOverlay` glass recipe exactly — a full-area `Theme.scrim` backdrop, the same one-fake-glass object (top-sheen `@linear-gradient` fill + 1px `{colors.glass-border}` rim + 1px `{colors.glass-highlight}` top inner highlight + ONE `{colors.shadow-popover}` drop shadow, blur 22 / y-offset 8). No nested glass, no second shadow. The sheen is a FILL gradient, not an `opacity` layer (opacity blurs text/SVG on HiDPI — see `docs/patterns.md`). The panel is fluid-width: caps at `{components.settings-panel.width}` (360px) on wide windows, leaving a `{spacing.xl}` gutter each side on narrow ones (FirstRunGuide clamp).

**Key model — EVERY dismiss path is cancel except one explicit confirm action:**
- Esc → `cancel()`.
- Return/Enter → `cancel()`. A reflexive press can never fire the destructive action — the ancestor `FocusScope` maps both Esc and Return to `cancel()`. The confirm button's `FocusButton` wrapper rejects Return (and all keys except Space), so Return bubbles up even when confirm holds focus.
- Backdrop click (scrim `TouchArea` outside the panel) → `cancel()`.
- Cancel button (pointer click or Tab→Space) → `cancel()`.
- Confirm button (pointer click or Tab→Space ONLY) → `confirm()`.
- All other keys are swallowed by the ancestor `FocusScope`'s catch-all `accept` so nothing leaks to content mounted behind the modal.

**Default focus: Cancel.** Set on BOTH `init` (fires on every `if`-gated open) and `changed focus-epoch` (re-claims focus after a stacked overlay closes). The parent MUST mount via `if`, NOT `visible:` — `init` fires only on subtree reconstruction.

**Tab containment**: two `FocusButton` stops (Cancel, Confirm). The ancestor `FocusScope` self-rotates Tab between them in-trap rather than deferring to window-level Tab navigation (which could carry focus out of the modal into live carousel elements behind it). See "Modal Tab containment" in `docs/patterns.md`.

**Body content structure (for the bulk-delete use case):**
1. Title line — carries the TOTAL selection count.
2. Optional itemized body lines — up to 10 titles, then "…and M more", then "N selected outside the current search" when applicable. The parent truncates before binding; the component renders what it receives.
3. Info band — `{colors.surface-raised}` raised panel with `{colors.text-muted}` text. Neutral reassurance ("Files on disk are kept"). Hidden when `info-text` is empty.
4. Warning line — `{colors.danger}` text. Fires only when the open book is among the selection. Hidden when `warning-text` is empty.
5. Action row — Cancel left, Confirm right (both pinned right via a leading stretch spacer; `alignment:` is NOT set — see "alignment kills stretch" in `docs/patterns.md`). DangerButton for destructive confirm; PrimaryButton for neutral confirm (toggled by the `danger` property).

### Selection mode and destructive-confirm interaction patterns

**Key bindings** (Library carousel, `Carousel.slint` `FocusScope`):

| Key | Normal mode | Selection mode |
|---|---|---|
| `x` | Enter selection mode + toggle focused book | Toggle focused book |
| Space | — (no-op / falls through) | Toggle focused book |
| Cmd/Ctrl+A | — (no-op / falls through) | Select all / deselect all |
| Delete / Backspace | — (no-op / falls through) | Open `ConfirmDialog` (no-op if N=0) |
| Esc | — (reject, no-op) | Clear selection + exit selection mode |
| Return | Open focused book | Open focused book (unchanged in both modes) |
| Cover click | Focus clicked cover | Focus + toggle clicked book |
| Cover double-click | Open clicked book (any visible cover; same path as Return) | — (the two clicks toggle twice = net no-op; never opens) |

Return is **never repurposed** — it always opens the focused book, in both modes. The `x` key is the primary keyboard entry into selection mode; the NavBar **Select capsule** is the mouse entry point (it enters mode only on first click, exits on re-click — does NOT toggle the focused book).

**Toolbar placement rationale**: the `SelectionToolbar` is a separate overlay anchored immediately below the `NavBar` (`y = nav.y + nav.height + {nav-item-gap}` — 13px), NOT embedded in the NavBar itself. This keeps the NavBar's chrome and the add-books pill visually and interactively untouched — the destructive control is separated from the constructive ones by a deliberate spatial gap. The DangerButton is the rightmost element of the toolbar, as far as possible from the accent-only count pill and capsules on the left, reinforcing the "destructive is at the end of the row" HIG convention.

### Library Nav — `components.nav-bar`
A **top, centered glass pill** floating over the Library carousel. It is drawn on top of the
stage; its bottom edge may slightly overlap the focused cover so the background shows through —
reinforcing the "glass" read.

- **Content**: the search field (left), a thin divider, then FIVE icon-only circular capsules
  right-of-divider — `file` (Add files), `folder` (Add folder), `select` (filter.svg; bulk-select
  toggle), `bookmark` (bookmark.svg; continue-reading jump), and `settings` (rightmost, unchanged).
  On macOS, where the OS panel picks files and folders in one dialog, the file + folder pair
  collapse into a single combined `plus` (Add books) capsule, giving FOUR capsules total.
  **No on-screen text labels** (accessible-label only) and **no tooltips**.
- **Select capsule** (`filter.svg`): toggles bulk-selection mode. While selection mode is on, it
  shows a persistent accent ring (`{colors.accent}` border + `{colors.accent-glow}` fill — the
  hover look, held) so the active mode is legible even through the recede veil. On an empty library
  the capsule dims to `{colors.text-faint}` icon and its `TouchArea` is inert, but it stays mounted
  so the pill width never jumps. Re-clicking exits selection mode (symmetric to the toolbar ✕ and
  Esc exit paths).
- **Bookmark capsule** (`bookmark.svg`): jumps to the continue-reading book via the same open path
  Return and a cover double-click use. Always enabled — with no bookmark, the click answers with a status notice in
  the bottom strip; a faint/disabled capsule would hide the affordance instead of inviting the
  press.
- **Each capsule** is circular; only the hovered/pressed/active cell glows softly with
  `{colors.accent-glow}`, and its icon brightens `{colors.text-mid}` → `{colors.text-high}`.
- **Search field** (left of the divider): a barely-visible 1px `{colors.search-border-rest}` rim at
  rest — held at threshold-of-visibility so the field reads as part of the glass — animating to a
  1px `{colors.accent}` edge on focus.
- **Glass treatment**: `{colors.glass-fill}` fill + a 1px `{colors.glass-border}` rim + a 1px
  `{colors.glass-highlight}` top inner highlight line + a `{colors.shadow-popover}` drop shadow
  (blur 22 / y-offset 8). **No backdrop-blur** — Slint 1.x cannot blur what's behind it, so the
  glass is faked with translucent fill + rim + top highlight + shadow.
- **Golden-ratio sizing** (phi ≈ 1.618, stepped through consecutive Fibonacci px): icon 21px →
  circular capsule diameter 34px → pill height 55px; item gap 13px; pill padding 11px. Pill width
  is computed from tokens: search field + (4 combined / 5 split) capsules + divider + (5 / 6) gaps
  + 2 × pill padding — no layout-preferred-width binding loop.
- **Interaction model**: mouse + screen-reader oriented. The pill is **NOT keyboard-reachable** —
  keyboard navigation stays owned by the carousel. Clicking a capsule fires the OS file/folder
  picker (`rfd`) and returns focus to the carousel.
- **Selection-mode recede**: when selection mode is active, a top-most no-input `{colors.scrim-soft}` veil
  is painted over the pill (animated at `motion-fast`). The veil is pointer-transparent (no `TouchArea`) —
  the search field and every nav capsule stay fully usable while receded. The veil fades in the same beat
  as the slide-strip transition so both land together.
- **Intentional deltas from the reference**: gashuu uses its own dark translucency (not blue
  glass), and **FILLED** icons (not outline) — a filled mass reads better on the dark canvas and
  resists low-DPI degradation.

### Library Bottom Status Strip
A full-width, bottom-pinned `{typography.ui-micro}` text line (`{colors.text-muted}`, centered,
`{spacing.md}` inset from the window bottom), visible only on the Library screen. It shares the
same `status-text` / `library-count-text` channel as the Viewer toolbar notices.

**Two states, one Text element** — transient notices always take precedence:
- **Idle** (when `status-text` is empty): shows the **total library count** — "N book(s)" (en) /
  "N 冊" (ja), Fluent-composed and pushed from Rust. Hidden entirely at 0 books (an empty library
  already shows the CTA; an idle "0 books" would be noise).
- **Active notice** (when `status-text` is non-empty): shows the transient feedback string
  ("Added N book(s)", "Added N, skipped M with no images", "Removed … — no images found",
  "Deleted…", "No bookmark registered", open errors, etc.). The count resumes
  automatically the next time `status-text` is cleared.

This is not a new chrome element — it reuses the existing notice channel and its position tokens.

### Settings Panel — `components.settings-panel`
A modal **content-hug glass panel** centered over the dimmed screen: 360px wide, exactly as tall as
its header + body + footer (the fixed φ outline was deliberately dropped 2026-06-04 — **φ relocated
into the component proportions**: the toggle track ratio, the 8/14/22 ≈ Fibonacci 8/13/21 spacing
ladder, and the segment pill inset), corner radius **21px** (= `nav-pill-radius` — it shares NavBar's
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
  right, all on one shared vertical centerline; `{spacing.xl}` horizontal / `{spacing.lg}` vertical
  padding (18 / 14 ≈ Fib 13 — the same ladder rung as the row gap, so the footer breathes on the
  body's row rhythm).
- **Toggle** is an Apple-proportioned switch: capsule track `controlHeight × φ` wide, 26px knob,
  2px inset; the off track is `{colors.track-prog}` (the darker `{colors.track}` vanished into the
  glass — HIG keeps the off state plainly visible), the knob carries a subtle depth shadow and
  slides on the Carousel's spring curve. **Segmented** capsules are `{rounded.md}` with a
  concentrically rounded selected pill inset 3px — a consecutive-Fibonacci 3/5/8 triple (inset →
  pill radius → capsule radius); labels center in the full 30px control height so ascenders never
  clip, with a 1px downward optical nudge (descender-less labels sit high under metric centering).
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
- Use `{colors.accent}` for ALL selection visuals (badge, count pill, toolbar hover states) — red/danger is reserved for the destructive delete/confirm buttons only.
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
- **Carousel**: every loaded cover renders at every window width; the focused cover stays
  centered and covers past the window edge simply clip. (An earlier neighbor-drop — centered
  cover ± one below a 560px threshold — was removed by user decision: it read as thumbnails
  vanishing on resize and during moves.) The 560px `narrow` flag remains and is forwarded to
  `SelectionToolbar`, collapsing its select-all capsule from a text label to the `check.svg`
  icon-only form at narrow widths.
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
