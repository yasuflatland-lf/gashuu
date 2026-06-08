# Settings dialog — "Clear data" buttons layout (option C1)

- Date: 2026-06-08
- Branch: `refactor/add_private_mode`
- Status: Implemented (brainstorm-approved; panel-reviewed)

## Problem

The two data-clearing utilities — "読書履歴をクリア" (Clear reading history) and
"カバーキャッシュをクリア" (Clear cover cache) — sat horizontally, left-aligned, at the
bottom of the **一般 (General)** section. With the long Japanese labels the row was cramped
and left an uneven right-hand gap, and the pair did not read as a coherent group.

## Decision (C1)

Promote the two buttons into a dedicated **"データ" (Data)** section, **vertically stacked,
full-width**, keeping the neutral `SecondaryButton` weight. Group separation matches the
existing sections (eyebrow + spacing, **no divider line**).

This was chosen over: A (horizontal, status quo), B (plain vertical, no section), D
(horizontal 50/50 split), and C2 (C1 + a hairline divider). C1 reuses the existing section
pattern exactly and adds no new visual element (per the panel's design-system guidance).

## Layout (as implemented)

- A new section placed **after 一般, before the footer** (⌨ ショートカット / 閉じる).
- Section eyebrow **"データ"** rendered with the existing section-eyebrow style (identical to
  読み方 / 表示 / パフォーマンス / 一般).
- Separation above the eyebrow uses the existing inter-section gap (`Theme.space-xxl`, 22px),
  inherited from the content layout — same as every other section. **No rule line.**
- The two `SecondaryButton`s stack in a `VerticalLayout`, each **full-width** (call-site
  `horizontal-stretch: 1`, overriding the component's own `horizontal-stretch: 0`), with an
  inter-button gap of `Theme.space-sm` (8px). The transient `data-action-status` feedback line
  moved into this section.
- Weight unchanged: neutral bordered `SecondaryButton` (chip fill + `hairline-float` border +
  white text). No red (DESIGN.md "red is scarce" stays intact). The footer Close / Reset stay
  `PrimaryButton`.

## Tokens / i18n

- Added a localized section label `settings-section-data` ("Data" / "データ") to `Strings.slint`
  and both `i18n/en/gashuu.ftl` and `i18n/ja/gashuu.ftl`, following the existing
  `settings-section-*` naming, pushed in `Localizer::apply`.
- Reuse existing spacing tokens only (`space-xxl` section gap, `space-sm` button gap). No new
  tokens, no new decoration. The `SecondaryButton` component is untouched (full-width is a
  call-site `horizontal-stretch`).
- Existing button labels, callbacks (`clear-reading-history()` / `clear-cover-cache()`), and
  Rust handlers are unchanged.

## Files changed

- `crates/gashuu/ui/SettingsDialog.slint` — moved the two clear buttons out of 一般 into a new
  "データ" section; stacked them vertically, full-width.
- `crates/gashuu/ui/Strings.slint` — added `settings-section-data`.
- `crates/gashuu/i18n/en/gashuu.ftl`, `crates/gashuu/i18n/ja/gashuu.ftl` — added
  `settings-section-data`.
- `crates/gashuu/src/i18n/mod.rs` — push `settings-section-data` in `Localizer::apply`.
- `DESIGN.md` — documented the "データ" section and the full-width vertical destructive-utilities
  grouping.

## Non-goals / constraints

- No change to button weight or color (stay neutral; no red).
- No new visual element (no divider) — reuse the existing section pattern.
- Stay within the existing Fibonacci/φ token system.

## Out of scope (incremental on the prior settings polish)

- Confirmation dialogs for the clear actions (already decided: immediate + status feedback).
- Component-level a11y for `PrimaryButton` (separate repo-wide follow-up).

## Verification

- Gates: `scripts/check-tokens.sh`, `cargo fmt --check`, `cargo clippy -p gashuu
  --all-targets -- -D warnings`, `cargo nextest run -p gashuu --profile ci` — all green (299
  tests).
- Visual: screenshot confirmed the two buttons render as a full-width vertical "データ" group,
  neutral weight, consistent section spacing, each Japanese label on a single line, no divider.
