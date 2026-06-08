# English Fluent catalog for gashuu.
#
# ID convention: <screen>-<element>[-<variant>], kebab-case.
# Prefixes: settings-, guide- (FirstRunGuide), carousel-, navbar-,
#           shortcuts-, viewer-pill-, stepper-, viewer- (status/dynamic),
#           notice-, common-.
# A11y-only strings get an -a11y suffix.
# Strings shared across screens live under the primary owner's prefix.

# ---- settings ----

# SettingsDialog header: shown when editing the current book's per-book settings.
settings-book-title = Book settings

# SettingsDialog header (global defaults) / NavBar settings-icon a11y /
# ViewerPill settings-icon a11y — one message, primary owner is SettingsDialog.
settings-title = Settings

# Section eyebrows
settings-section-reading = Reading
settings-section-display = Display
settings-section-performance = Performance
settings-section-general = General

# Reading section — Direction row
settings-direction-label = Direction
settings-direction-ltr = Left to Right
settings-direction-rtl = Right to Left
settings-direction-a11y = Reading direction

# Reading section — Spread row
settings-spread-label = Spread
settings-spread-single = Single
settings-spread-double = Double
settings-spread-auto = Auto
settings-spread-a11y = Spread mode

# Display section — Cover row
settings-cover-label = Cover
settings-cover-standalone = Standalone
settings-cover-paired = Paired
settings-cover-a11y = Cover mode

# Display section — Fit row
settings-fit-label = Fit
settings-fit-whole = Whole
settings-fit-width = Width
settings-fit-actual = Actual
settings-fit-a11y = Fit mode

# Performance section — rows and footnote
settings-cache-label = Cache size (pages)
settings-cache-a11y = Cache size in pages
settings-preload-label = Preload radius
settings-preload-a11y = Preload radius
settings-track-recent-label = Track recent files
settings-track-recent-a11y = Track recent files
settings-performance-note = Cache & preload apply to newly opened books.
settings-allow-rar-label = Allow CBR/RAR archives
settings-allow-rar-a11y = Allow CBR/RAR archives

# General section — Clear action buttons
settings-clear-history-label = Clear reading history
settings-clear-cache-label = Clear cover cache

# General section — Clear action status feedback (issue #178)
settings-history-cleared = Reading history cleared.
settings-history-clear-failed = Could not clear reading history.
settings-cache-cleared =
    { $n ->
        [one] Removed { $n } cached file ({ $size }).
       *[other] Removed { $n } cached files ({ $size }).
    }
settings-cache-clear-failed = Could not clear cover cache.

# General section — Language row
settings-language-label = Language
settings-language-a11y = Display language

# Footer: Shortcuts affordance label and its a11y label (also used as the
# ShortcutsOverlay panel header — shortcuts-title is the primary owner there).
settings-shortcuts-label = ⌨ Shortcuts

# Footer: Reset to global (per-book settings only)
settings-reset-to-global = Reset to global

# ---- shortcuts ----

# ShortcutsOverlay panel header / SettingsDialog footer accessible-label.
shortcuts-title = Keyboard shortcuts

# Multi-line keyboard reference rendered read-only in ShortcutsOverlay.
# File indentation: section headers 4 spaces, body lines 6 spaces.
# Fluent strips the common 4-space prefix from block values, so delivered text
# has: headers 0 spaces (flush), body lines 2 spaces — formerly matching the deleted messages.rs arms.
# Blank lines between sections are preserved naturally.
shortcuts-help =
    Navigation:
      Space = next page    Backspace = previous page
      Arrows follow the reading direction (LTR: → next; RTL: ← next)

    Modes:
      D = spread (single → double → auto)
      R = reading direction (LTR / RTL)
      C = cover layout (standalone / paired)

    Zoom & fit:
      + / - = zoom in / out    0 = reset view    1 = actual size    f = cycle fit

    View:
      T = toggle thumbnail strip

    Library:
      Up = return to the library

    Selection:
      x = enter selection mode    Space = toggle focused
      Cmd/Ctrl+A = select all visible / deselect all
      Delete / Backspace = delete selected books
      Esc = exit selection mode

# ---- guide ----

# FirstRunGuide overlay
guide-welcome = Welcome to gashuu
guide-intro = A quick guide to get you started:
guide-open = Open: use the toolbar buttons — Open Folder… / Open Archive… (CBZ/ZIP/CBR/RAR).
guide-turn-pages = Turn pages: Space = next, Backspace = previous. Arrow keys follow the reading direction.
guide-modes = Modes: D = spread (single → double → auto), R = reading direction (LTR/RTL), C = cover layout.
guide-zoom-fit = Zoom & fit: + / - to zoom, 0 to reset, 1 for actual size, f to cycle fit mode. Wheel zooms at the cursor; drag to pan.
guide-thumbnails = Thumbnails: T toggles the thumbnail strip; click a thumbnail to jump to that page.
guide-settings = Settings: open the Settings dialog from the toolbar to change these options anytime.
guide-got-it = Got it

# ---- carousel ----

# Empty library state (0 books)
carousel-empty-title = Your library is empty
carousel-empty-subtitle = Add books to start your shelf.
carousel-empty-cta = Select folders / files to add
# Files-only CTA wording for platforms without the combined picker (non-macOS).
carousel-empty-cta-files = Select files to add

# No-results state (library has books but active filter matches none)
carousel-no-results-title = No matching books
carousel-no-results-hint = Try a different search.

# Idle bottom-strip label: total library size, shown when no transient notice
# occupies the strip. { $n } is the total book count (only composed for n > 0).
library-count =
    { $n ->
        [one] { $n } book
       *[other] { $n } books
    }

# ---- navbar ----

# SearchField placeholder and a11y labels (all three uses in NavBar.slint)
navbar-search-placeholder = Search library
navbar-search-a11y = Search library

# NavItem a11y labels for the action capsules. add-books labels the single
# combined capsule on macOS (one NSOpenPanel picks files and folders);
# add-files/add-folder label the two separate capsules on other platforms.
navbar-add-files-a11y = Add files
navbar-add-folder-a11y = Add folder
navbar-add-books-a11y = Add books

# NavBar settings capsule a11y — deduped to settings-title.

# ---- viewer-pill ----

# PageJumpField a11y label
viewer-pill-goto-page-a11y = Go to page

# Thumbnail capsule a11y label
viewer-pill-thumbnails-a11y = Toggle thumbnails

# Settings capsule a11y — deduped to settings-title.

# ---- stepper ----

# Accessible labels for the decrease/increase buttons; { $label } is the
# parent SettingRow's a11y-label (e.g. "Cache size in pages").
# Named arg handles word order across languages.
stepper-decrease = Decrease { $label }
stepper-increase = Increase { $label }

# ---- common ----

# Close button used in SettingsDialog and ShortcutsOverlay footers.
common-close = Close

# ---- confirm-delete ----

# Cancel button in the bulk-delete confirmation dialog.
confirm-delete-cancel = Cancel

# Dialog title: { $n } is the count of books to delete.
confirm-delete-title = Delete { $n } book(s)?

# Overflow line when the preview list is truncated: { $n } is the number of
# books not shown in the preview.
confirm-delete-more = …and { $n } more

# Explanatory body: what gets removed and what stays on disk.
confirm-delete-keep-files = Removes the library entry, reading position, per-book settings, and cached covers. The files on disk are kept.

# Warning shown when the currently open book is in the delete set.
confirm-delete-open-book = The currently open book is included — it will be closed.

# Warning shown when the selection spans books outside the current search filter.
# { $n } is the count of books outside the search projection.
confirm-delete-outside-search = Includes { $n } book(s) outside the current search.

# ---- viewer ----
# Dynamic status-line messages (mapped to the former msg_* functions of the deleted src/messages.rs).

# Static status strings
viewer-no-folder = No folder opened
viewer-no-images = Folder contains no images
# Library-screen status when Down is pressed but no book is open yet.
viewer-no-open-book = No book is open

# Compact spread-mode labels for the status line's [mode · direction] tail
viewer-spread-single = single
viewer-spread-double = double
viewer-spread-auto = auto

# Compact reading-direction labels for the status line
viewer-direction-ltr = LTR
viewer-direction-rtl = RTL

# Parameterized status/error strings
viewer-open-error = Error: { $error }
viewer-page-unavailable = (page { $page } unavailable)
viewer-decode-error = Decode error: { $error }

# ---- bookmark-ribbon ----

# BookmarkRibbon accessible label — "continue reading" signifier on library covers.
continue-reading = Continue reading

# ---- notice ----
# Parameterized notice strings (mapped to the former msg_* functions of the deleted src/messages.rs).

# Fluent trims leading whitespace on values — the historical leading space of
# this archive-skip suffix must be wrapped in a string-literal placeable.
notice-skipped-detail-archive = {" "}(zip-slip or oversized)
notice-entries-skipped = { $n } entries skipped{ $detail }
notice-failed-save-settings = Failed to save settings: { $error }
notice-failed-save-library = Failed to save library: { $error }
notice-could-not-save-settings = Could not save settings: { $error }
notice-load-failed = Could not load { $what }; starting fresh.
notice-already-in-library = Already in library — no new books added.
# Notice when the NavBar bookmark capsule is clicked but no continue-reading
# bookmark is registered (or it points at a book no longer in the library).
notice-bookmark-none = No bookmark registered
notice-added-books = Added { $n } book(s)
notice-added-books-save-failed = Added { $n } book(s), but could not save library: { $error }
# Some books were added but some paths had no images and were skipped.
notice-added-books-skipped = Added { $n } book(s), skipped { $skipped } with no images
# All picked paths had no images; nothing was added to the library.
notice-no-books-added-empty = No books added — { $skipped } item(s) had no images
# An existing library entry was auto-removed because its source has no images.
notice-empty-book-removed = Removed "{ $title }" — no images found

# Notice after a successful bulk delete. { $n } is the count of books deleted.
notice-deleted-books = Deleted { $n } book(s)

# Notice when the library save failed after a bulk delete, meaning nothing was
# actually removed. { $error } is the error detail.
notice-delete-save-failed = Could not save the library: { $error } — nothing was deleted; your files are untouched.

# ---- selection ----

# Toolbar count label shown when zero books are selected (just entered selection mode).
selection-mode-label = Selection mode
# Toolbar count label when all selected books are in the current visible projection.
selection-count = { $n } selected
# Toolbar count label when some selected books are outside the visible search projection.
# $m is the number of selected books that are outside (off-screen / filtered out).
selection-count-outside = { $n } selected ({ $m } outside search)
# Toolbar toggle button: select all visible books.
selection-select-all = Select all
# Toolbar toggle button: deselect all visible books (shown when all visible are selected).
selection-deselect-all = Deselect all
# NavBar button that enters selection mode.
selection-enter = Select
# A11y label for the toolbar button that exits selection mode.
selection-exit-a11y = Exit selection mode

# DangerButton label in the SelectionToolbar; also the confirm-button label in
# the bulk-delete ConfirmDialog (bound via Strings.selection-delete).
selection-delete = Delete
