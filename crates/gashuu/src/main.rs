// On Windows, a GUI binary built for the default "console" subsystem spawns an
// extra console window alongside the app window on launch. Switch RELEASE builds
// to the "windows" subsystem so end users see only the app window; debug builds
// keep the console so `tracing` output stays visible while developing. The
// attribute is a no-op on non-Windows targets.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

slint::include_modules!();

mod add_books;
mod add_loader;
mod app;
mod carousel;
mod carousel_refresh;
mod cover_loader;
mod enum_adapters;
mod handlers;
mod i18n;
mod keymap;
mod library_model;
mod navigation;
mod open_book;
mod page_count_prefetch;
mod page_jump;
mod page_loader;
mod remove_books;
mod selection_projection;
mod thumbnail_strip;
mod view_sync;
mod viewer_state;
mod viewport;
mod window_state;

pub(crate) use add_books::apply_outcomes;
pub(crate) use carousel_refresh::{
    apply_add_report, finalize_empty_book_removed, finalize_remove, push_selection_strings,
    refresh_library_carousel, snap_carousel_focus_to_last_opened, visible_index_to_path,
    CarouselRefresh,
};
use gashuu_core::{CoreError, DecodedImage, Library, ReadingDirection, Settings};
use library_model::{LibrarySearchState, LibrarySelectionState};
use navigation::{screen_to_index, NavState};
use page_loader::PageController;
#[cfg(not(test))]
use page_loader::{PageSlot, SpreadDecodeRequest};
use std::cell::RefCell;
use std::rc::Rc;
use thumbnail_strip::ThumbnailController;
pub(crate) use view_sync::{
    apply_global_view_to_runtime, current_book_name, persist_view_modes, write_back_position,
    ViewModeRoute,
};
#[cfg(not(test))]
use viewer_state::SpreadSlots;
use viewer_state::{StatusContent, ViewerState};
use viewport::ViewportState;

/// Load a persisted value, falling back to its `Default` on a RECOVERABLE
/// failure. The single home of the corrupt-file recovery policy — default on
/// error, collect a surfaceable notice, log a warning — which was hand-written
/// once per source before. `label` names the source for both the `errs` notice
/// (`"<label> (<e>)"`, surfaced on the home screen) and the log. A missing file
/// returns `Ok(default)` from the loader, so this fallback fires only on a
/// GENUINE failure (corrupt data, I/O error, `NoDataDir`). Stays UI-side (it
/// logs via `tracing`) so `gashuu-core` remains headless.
fn load_or_default<T: Default>(
    label: &str,
    load: impl FnOnce() -> Result<T, CoreError>,
    errs: &mut Vec<String>,
) -> T {
    match load() {
        Ok(value) => value,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load {label}; using defaults");
            errs.push(format!("{label} ({e})"));
            T::default()
        }
    }
}

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    // Slint's text layout (parley -> icu_segmenter) emits a `log::warn!` for every
    // CJK run because ICU4X bundles no Japanese line-break dictionary. Segmentation
    // still works via a per-character fallback, so the resulting "ICU4X data error:
    // No segmentation model for language: ja" lines are pure noise. They reach this
    // subscriber through tracing-subscriber's tracing-log bridge; silence the
    // `icu_provider` target that emits them while leaving any RUST_LOG override for
    // our own targets intact (a target directive overrides the global default).
    let env_filter = tracing_subscriber::EnvFilter::from_default_env().add_directive(
        "icu_provider=off"
            .parse()
            .expect("static directive is valid"),
    );
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    // Load persisted settings and library through `load_or_default` — the single
    // home of the corrupt-file recovery policy (default-on-error + collect a
    // notice). Errors are collected and surfaced on the home screen after the
    // initial refresh, which itself overwrites status-text, so the notice must be
    // set after that call.
    let mut load_errs: Vec<String> = Vec::new();
    let settings = load_or_default("settings", Settings::load, &mut load_errs);
    let library = load_or_default("library", Library::load, &mut load_errs);

    let ui = ViewerWindow::new()?;
    // Boot the Fluent localizer with the persisted language; `apply()` pushes
    // every static string into the Strings global before the first paint.
    let localizer = Rc::new(i18n::Localizer::new(settings.language));
    localizer.apply(&ui);
    // Platform capability, pushed once: macOS' NSOpenPanel picks files AND
    // folders in one panel, so the Library NavBar collapses its two add
    // capsules into a single combined one (the dialog flavor itself is decided
    // in `on_add_books`). Compile-time constant — never changes at runtime.
    ui.set_combined_add_picker(cfg!(target_os = "macos"));
    let state = Rc::new(RefCell::new(ViewerState::from_settings(&settings)));
    let viewport = Rc::new(RefCell::new(ViewportState::from_settings(&settings)));
    let settings = Rc::new(RefCell::new(settings));

    // The persisted shelf, shared so the carousel model build, the focused-index
    // clamp, and later PR-L's add / PR-R's position write-back can all reach it.
    let library = Rc::new(RefCell::new(library));

    // Thumbnail-strip controller. Owns the strip's backing model and the
    // generation bookkeeping (epoch + cancel double-guard); its `new` binds the
    // model into the UI via `set_thumbnails` internally. Wrapped in `Rc` so both
    // open handlers (via `OpenBookUseCase`) can share the single controller.
    let thumbs = Rc::new(ThumbnailController::new(&ui));

    // Cover controller for the library carousel. Owns the epoch + cancel
    // double-guard; `start` is called after the carousel model is (re)built so a
    // library refresh supersedes any covers still streaming from the prior view.
    let covers = Rc::new(cover_loader::CoverController::new());

    // Page controller for the viewer body. Owns async page-decode epoch and
    // dispatch dedup for cache-miss spreads.
    let pages = Rc::new(page_loader::PageController::new());

    // Bulk-add controller (issue 206): probes each picked source off the UI
    // thread so a bulk add never freezes the event loop. Owns the supersede
    // epoch + per-generation outcome accumulator; the add handlers call `start`,
    // and the `add-finalize` callback runs the apply half on the UI thread.
    // Shared via `Rc` so the add and finalize handlers see one controller.
    let adder = Rc::new(add_loader::AddController::new());

    // Shared library-search filter state. Owned here and shared via `Rc` with the
    // search-query callback (live filtering), the add/open backfill paths, and the
    // open-time page-count rebuild in `OpenBookUseCase`, so every path projects the
    // SAME visible-index set. Starts on the empty query (every book visible).
    let search = Rc::new(RefCell::new(LibrarySearchState::default()));
    ui.set_library_search_query("".into());
    // Seed the visible set against the loaded library under the empty query
    // (every book visible). `set_query` recomputes internally, so the search
    // state is consistent before the first `refresh_library_carousel`, which now
    // only READS `visible_indices()`.
    search
        .borrow_mut()
        .set_query(String::new(), &library.borrow());

    // Shared bulk-selection state (bulk-delete epic, PR-2). Owned here and shared
    // via `Rc` with the carousel toggle / cover-click / exit handlers and the
    // carousel refresh (which re-applies the selection flags over the rebuilt
    // visible rows). Keyed by path, so it is orthogonal to the search projection —
    // a query change never drops a selection. Nothing is deleted in this PR.
    let selection = Rc::new(RefCell::new(LibrarySelectionState::default()));

    // The "open a book" use-case, bundling the shared collaborators it threads
    // (state, settings, viewport, library, thumbs, covers, search). Built once and
    // shared via `Rc` by the Open Folder / Open Archive / carousel-open handlers so
    // the open flow lives in exactly one place (`app::OpenBookUseCase`). The search
    // state lets the open-time page-count rebuild preserve the active filter.
    let open_book = Rc::new(app::OpenBookUseCase::new(
        Rc::clone(&state),
        Rc::clone(&settings),
        Rc::clone(&viewport),
        Rc::clone(&library),
        Rc::clone(&thumbs),
        Rc::clone(&covers),
        Rc::clone(&search),
    ));

    // Seed the carousel from the persisted library so the home screen shows the
    // saved books on boot. The empty-query visible set was already computed by the
    // `set_query(String::new(), …)` seed above; `refresh_library_carousel` reads
    // those visible indices, builds + binds the filtered model, resets carousel
    // focus to 0, and starts cover loading for the visible rows in ONE place — so
    // the initial build and every later filter/add refresh share the same code
    // path. Cover streaming is started exactly once here (no separate
    // `covers.start`).
    refresh_library_carousel(
        &ui,
        &CarouselRefresh {
            library: &library,
            covers: &covers,
            search: &search,
            selection: &selection,
            localizer: &localizer,
        },
        true,
    );
    // Continue reading: the app boots on the Library screen, so override the
    // refresh's reset-to-0 with a one-shot snap to the last-read book's visible
    // row, resolved through the empty-query visible set seeded above.
    snap_carousel_focus_to_last_opened(&ui, &library, &search);

    // One startup sweep keeps the cover cache under its size cap and reclaims
    // key-orphaned covers (issue 143). Dispatched AFTER the initial cover
    // stream above so the visible covers grab the rayon workers first.
    cover_loader::spawn_cache_prune();

    // Top-level screen state machine. App boots to Library (the carousel home).
    // Held in an Rc<RefCell<…>> so the carousel callbacks and the Viewer's
    // GoToLibrary key arm can all flip it through the seam functions below.
    let nav = Rc::new(RefCell::new(NavState::new()));
    // Push the initial screen so the window shows the Library on boot.
    ui.set_screen(screen_to_index(nav.borrow().screen()));

    // The centered title-bar name starts blank — nothing is open yet — and is
    // set to the folder/archive name on a successful open (see the open handlers).
    ui.set_current_book_name("".into());

    // Initial paint so rtl/single/status are all initialized before the first
    // folder is opened (refresh shows "No folder opened" and clears the images).
    refresh(
        &ui,
        &state.borrow(),
        &viewport,
        localizer.loader(),
        &pages,
        ui.as_weak(),
    );

    // Surface any load failures AFTER the initial refresh, which overwrites
    // status-text with "No folder opened". The carousel/home screen is visible
    // at this point, so the user sees the notice immediately. Missing files
    // return Ok(default), so this fires only on genuine failures.
    if !load_errs.is_empty() {
        ui.set_status_text(
            crate::i18n::dynamic::load_failed(localizer.loader(), &load_errs.join(" and ")).into(),
        );
    }

    // First-run guide: show the overlay exactly once. `seen_guide` is flipped and
    // persisted when the user dismisses it (see `on_dismiss_guide`).
    if !settings.borrow().seen_guide {
        ui.set_show_guide(true);
    }

    // Wire all event handlers onto the window (handlers/, #151).
    handlers::wire_open_handlers(
        &ui, &state, &viewport, &settings, &library, &open_book, &covers, &pages, &adder, &search,
        &selection, &localizer,
    );
    handlers::wire_carousel_handlers(
        &ui, &state, &viewport, &library, &nav, &open_book, &covers, &pages, &search, &selection,
        &localizer,
    );
    handlers::wire_selection_handlers(
        &ui, &state, &library, &covers, &search, &selection, &localizer,
    );
    handlers::wire_viewer_input_handlers(&ui, &state, &viewport, &pages, &localizer);
    handlers::wire_settings_handlers(
        &ui, &state, &viewport, &settings, &library, &covers, &pages, &search, &selection,
        &localizer,
    );
    handlers::wire_view_mode_handlers(
        &ui, &state, &viewport, &settings, &library, &pages, &search, &selection, &localizer,
    );
    handlers::wire_viewport_handlers(&ui, &viewport);
    handlers::wire_nav_handlers(
        &ui, &state, &viewport, &settings, &library, &nav, &covers, &pages, &search, &selection,
        &localizer,
    );
    // File/folder drag-and-drop onto the Library screen, feeding the same bulk-add
    // pipeline as the Add buttons (handlers/drag_drop.rs).
    handlers::wire_drag_drop_handlers(&ui, &settings, &adder);

    // Restore the last window size + position before the first paint. No-op on a
    // fresh install; off-screen positions are dropped in favor of centering.
    window_state::restore_geometry(&ui, &settings.borrow());

    ui.run()?;
    // Persist any page counts the cover prefetch resolved after the last carousel
    // refresh, so a book counted this session shows its real total next launch
    // instead of being re-counted by re-opening its archive. Safe here: the event
    // loop has exited, so `library` is unborrowed.
    covers.flush_counts(&library);
    // Write the current reading position back to the library before exit.
    // The `state` and `library` RefCells are no longer borrowed (the event
    // loop has exited), so there is no borrow conflict here.
    write_back_position(&state, &library);
    // Persist the open book's view modes to its override on exit, then mirror into
    // the GLOBAL Settings only when no book is open (the ADR-0007 clobber guard
    // lives inside `persist_view_modes`). cache/preload/track and seen_guide are
    // saved unconditionally below.
    persist_view_modes(
        ViewModeRoute::AppExit,
        &state,
        &viewport,
        &settings,
        &library,
    );
    // Record the final window geometry so the next launch restores it. The window
    // handle is still alive here (the event loop has exited but `ui` is in scope),
    // and `settings` is unborrowed.
    window_state::capture_geometry(&ui, &mut settings.borrow_mut());

    if let Err(e) = settings.borrow().save() {
        tracing::error!(error = %e, "failed to save settings on exit");
    }
    Ok(())
}

/// Upgrade a window `Weak` and run `f` with the live `ViewerWindow`, or no-op if
/// the window is gone (teardown race). Replaces the repeated
/// `let Some(ui) = ui_weak.upgrade() else { return; };` preamble.
fn with_ui(weak: &slint::Weak<ViewerWindow>, f: impl FnOnce(ViewerWindow)) {
    if let Some(ui) = weak.upgrade() {
        f(ui);
    }
}

/// Log a failed persistence save and surface it on the status line — the single
/// home of the direct log+status save-failure shape. The aggregation paths keep
/// their own composition and do NOT route through this: `NoticesContent`
/// (app.rs, pre-captured `Option<String>` composed in `finalize_open`), the
/// add-batch report (`apply_add_report`), the bulk-delete rollback
/// (`RemoveOutcome::SaveFailed`), and the log-only sites where no status line
/// is wanted (guide dismiss, exit, write-backs).
fn report_save_error(
    ui: &ViewerWindow,
    loader: &i18n_embed::fluent::FluentLanguageLoader,
    e: &CoreError,
    context: &'static str,
) {
    tracing::error!(error = %e, "{context}");
    ui.set_status_text(crate::i18n::dynamic::could_not_save_settings(loader, e).into());
}

pub(crate) fn clear_page_view(ui: &ViewerWindow, viewport: &Rc<RefCell<ViewportState>>) {
    ui.set_leading_loading(false);
    ui.set_trailing_loading(false);
    ui.set_leading_page(slint::Image::default());
    ui.set_trailing_page(slint::Image::default());
    ui.set_single(true);
    viewport.borrow_mut().set_content(0.0, 0.0);
    apply_viewport(ui, &viewport.borrow());
}

pub(crate) fn apply_spread_images(
    ui: &ViewerWindow,
    leading: slint::Image,
    trailing: Option<slint::Image>,
    single: bool,
) {
    ui.set_leading_page(leading);
    if single {
        ui.set_trailing_page(slint::Image::default());
    } else {
        ui.set_trailing_page(trailing.unwrap_or_default());
    }
    ui.set_single(single);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_spread_geometry(
    ui: &ViewerWindow,
    viewport: &Rc<RefCell<ViewportState>>,
    loader: &i18n_embed::fluent::FluentLanguageLoader,
    content_w: f32,
    content_h: f32,
    single: bool,
    trailing_failed: Option<usize>,
    status: &StatusContent,
) {
    ui.set_single(single);
    let base_status = crate::i18n::dynamic::format_status(loader, status);
    match trailing_failed {
        Some(failed) => ui.set_status_text(
            format!(
                "{base_status}  {}",
                crate::i18n::dynamic::page_unavailable(loader, failed + 1)
            )
            .into(),
        ),
        None => ui.set_status_text(base_status.into()),
    }
    viewport.borrow_mut().set_content(content_w, content_h);
    apply_viewport(ui, &viewport.borrow());
}

fn spread_content_size(leading: &DecodedImage, trailing: Option<&DecodedImage>) -> (f32, f32) {
    match trailing {
        Some(trailing) => (
            (leading.width() + trailing.width()) as f32,
            leading.height().max(trailing.height()) as f32,
        ),
        None => (leading.width() as f32, leading.height() as f32),
    }
}

#[cfg(not(test))]
fn page_slot(index: usize, decoded: Option<std::sync::Arc<DecodedImage>>) -> PageSlot {
    match decoded {
        Some(decoded) => PageSlot::hit(index, decoded),
        None => PageSlot::miss(index),
    }
}

#[cfg(not(test))]
fn spread_request(slots: SpreadSlots) -> SpreadDecodeRequest {
    let leading = page_slot(slots.leading.0, slots.leading.1);
    match slots.trailing {
        Some((index, decoded)) => SpreadDecodeRequest::double(leading, page_slot(index, decoded)),
        None => SpreadDecodeRequest::single(leading),
    }
}

/// Push the current spread + status into the UI, then re-anchor the viewport to
/// the new content size and push the resulting geometry.
pub(crate) fn refresh(
    ui: &ViewerWindow,
    state: &ViewerState,
    viewport: &Rc<RefCell<ViewportState>>,
    loader: &i18n_embed::fluent::FluentLanguageLoader,
    pages: &PageController,
    #[cfg_attr(test, allow(unused_variables))] ui_weak: slint::Weak<ViewerWindow>,
) {
    let content = state.status_content();
    ui.set_rtl(matches!(state.reading_direction(), ReadingDirection::Rtl));
    match state.spread_slots() {
        Some(slots) => {
            let leading_idx = slots.leading.0;
            let trailing_idx = slots.trailing.as_ref().map(|(index, _)| *index);
            pages.set_target(leading_idx, trailing_idx, slots.single);

            let leading_missing = slots.leading.1.is_none();
            let trailing_missing = slots
                .trailing
                .as_ref()
                .is_some_and(|(_, decoded)| decoded.is_none());

            if !leading_missing && !trailing_missing {
                let leading = slots.leading.1.as_ref().expect("checked HIT");
                let trailing = slots
                    .trailing
                    .as_ref()
                    .and_then(|(_, decoded)| decoded.as_ref());
                let (content_w, content_h) =
                    spread_content_size(leading, trailing.map(|img| img.as_ref()));
                ui.set_leading_loading(false);
                ui.set_trailing_loading(false);
                pages.clear_dispatched_spread(leading_idx, trailing_idx);
                apply_spread_images(
                    ui,
                    to_slint_image(leading),
                    trailing.map(|img| to_slint_image(img)),
                    slots.single,
                );
                apply_spread_geometry(
                    ui,
                    viewport,
                    loader,
                    content_w,
                    content_h,
                    slots.single,
                    None,
                    &content,
                );
            } else {
                ui.set_status_text(crate::i18n::dynamic::format_status(loader, &content).into());
                ui.set_single(slots.single);
                ui.set_leading_page(slint::Image::default());
                ui.set_trailing_page(slint::Image::default());
                viewport.borrow_mut().set_content(0.0, 0.0);
                apply_viewport(ui, &viewport.borrow());
                ui.set_leading_loading(leading_missing);
                ui.set_trailing_loading(trailing_missing);
                // Mounting the LoadingSlot perturbs PageView's TouchArea
                // hit-testing under a stationary cursor and emits a phantom
                // `changed mouse-x`; suppress the resulting pointer-reveal so the
                // chrome does not flip on every cache-MISS page turn.
                ui.invoke_arm_pointer_reveal_suppression();

                #[cfg(not(test))]
                if let Some(dispatch) = state.dispatch_handle() {
                    pages.dispatch_spread(ui_weak, dispatch, spread_request(slots));
                }
            }
        }
        None => {
            // Source loaded but empty (or no source yet): clear and show single
            // so the view matches the status text ("No folder opened" / "Folder
            // contains no images").
            ui.set_status_text(crate::i18n::dynamic::format_status(loader, &content).into());
            pages.clear_target();
            clear_page_view(ui, viewport);
        }
    }
    // Keep the thumbnail-strip highlight in sync with the current spread's
    // leading page after every navigation/refresh.
    ui.set_current_index(state.index() as i32);

    // Seed the scrubber chrome from the current spread. The scrubber uses 1-based
    // numbers; `double` mirrors whether the current spread has a trailing page.
    // These are display-only and do NOT change the page body.
    let total = state.page_count();
    let current_1based = current_page_1based(state);
    ui.set_scrubber_total_pages(total as i32);
    ui.set_scrubber_current_page(current_1based as i32);
    // `preview_is_double` resolves the trailing page using the SAME layout as the
    // body (and is decode-free), so it is the exact "current spread has a trailing
    // page" predicate without re-running `current_spread`'s decode.
    let is_double = state.preview_is_double(state.index());
    ui.set_scrubber_double(is_double);
    ui.set_page_jump_text(format!("{}", current_1based).into());
}

/// Build the empty-book auto-removal status line: the localized
/// `empty_book_removed` notice, with the localized library-save-failure detail
/// appended via the shared `format!("{base} \u{2014} {detail}")` pattern when
/// `save_error` is `Some`. Shared by the two removal paths (the open-time
/// `finalize_open` arm and the cover-time `on_empty_book_detected` handler) so
/// the compose-and-append logic lives in one spot; the formatting stays in
/// `main.rs` (per the spec) while `empty_book_removed` / `failed_save_library`
/// remain the pure `dynamic.rs` notice seams.
pub(crate) fn empty_book_removed_status(
    loader: &i18n_embed::fluent::FluentLanguageLoader,
    title: &str,
    save_error: Option<&str>,
) -> String {
    let base = crate::i18n::dynamic::empty_book_removed(loader, title);
    match save_error {
        Some(e) => {
            let detail = crate::i18n::dynamic::failed_save_library(loader, &e);
            format!("{base} \u{2014} {detail}")
        }
        None => base,
    }
}

/// Finalize an `open_book.run(...)` outcome on the UI. On failure, set the
/// localized error status; on success, `refresh()` the view and append each
/// localized notice to the status line. The single place the four open sites
/// (Open Folder, Open Archive, carousel-open, bookmark-jump) share this UI
/// wiring, so the `OpenOutcome` match + notice-append loop lives in exactly one
/// spot.
fn finalize_open(
    ui: &ViewerWindow,
    state: &Rc<RefCell<ViewerState>>,
    viewport: &Rc<RefCell<ViewportState>>,
    pages: &PageController,
    deps: &CarouselRefresh,
    outcome: app::OpenOutcome,
) {
    let loader = deps.localizer.loader();
    match outcome {
        app::OpenOutcome::Error(e_str) => {
            ui.set_status_text(crate::i18n::dynamic::open_error_str(loader, &e_str).into());
        }
        app::OpenOutcome::Success(notices) => {
            pages.set_source();
            refresh(ui, &state.borrow(), viewport, loader, pages, ui.as_weak());
            for detail in crate::i18n::dynamic::format_notices(loader, &notices) {
                let base = ui.get_status_text().to_string();
                ui.set_status_text(format!("{base} \u{2014} {detail}").into());
            }
        }
        app::OpenOutcome::EmptyBookRemoved {
            title,
            removed,
            save_error,
        } => {
            pages.set_source();
            // The source opened cleanly but has zero pages: the use case already
            // removed it from the library (if present) and re-saved. This arm does
            // NOT switch screens (see the `enter_viewer` guard at the carousel/
            // bookmark sites); the shared finalize rebuilds the carousel and shows
            // the notice when this path owns the removal.
            finalize_empty_book_removed(
                ui,
                deps,
                &crate::open_book::EmptyBookRemoval {
                    title,
                    removed,
                    save_error,
                },
            );
        }
    }
}

/// Returns the current 1-based page number (0 when no pages loaded).
fn current_page_1based(state: &ViewerState) -> usize {
    if state.page_count() == 0 {
        0
    } else {
        state.index() + 1
    }
}

/// Push the viewport's render geometry (content_x/y/w/h, logical px as `f32`)
/// into the UI properties.
fn apply_viewport(ui: &ViewerWindow, viewport: &ViewportState) {
    let (x, y, w, h) = viewport.geometry();
    ui.set_content_x(x);
    ui.set_content_y(y);
    ui.set_content_w(w);
    ui.set_content_h(h);
}

/// Switch the app to the Library carousel and sync the UI's `screen` property.
/// The single chokepoint for "go to Library" so no caller forgets to sync the
/// UI's `screen` property and restore carousel focus (mirrors `go_to_viewer`).
///
/// On every entry transition this REBUILDS the carousel model through the shared
/// `refresh_library_carousel` chokepoint. The model was bound once at boot, so a
/// per-row flag derived from `last_opened` (the "continue reading" ribbon) would
/// otherwise still carry its build-time value: after reading book X and coming
/// back, `last_opened` has changed but the bound rows have not. Rebuilding here
/// re-derives those rows from the CURRENT library, restarting cover loading from
/// the (already-warm) cache and re-applying the path-keyed selection — both
/// handled inside the chokepoint, so neither flashes nor drops. We then snap the
/// carousel focus to the last-read book so Return resumes it. This is a ONE-SHOT
/// set at the entry moment, NOT a binding — after entry the user owns focus.
fn go_to_library(ui: &ViewerWindow, nav: &Rc<RefCell<NavState>>, deps: &CarouselRefresh) {
    nav.borrow_mut().to_library();
    ui.set_screen(screen_to_index(nav.borrow().screen()));
    // The Library's bottom strip renders `status-text` (its only consumer), so
    // the Viewer's page status ("12–13 / 200 [double · RTL]") written by
    // `refresh` would otherwise leak under the carousel, where it is
    // meaningless. Clear it on entry; add/save feedback is set AFTER this.
    ui.set_status_text("".into());
    // Rebuild so the model reflects the CURRENT `last_opened` (freshness), then
    // override the chokepoint's reset-to-0 with the continue-reading snap.
    // Two writes: refresh_library_carousel sets focused-index to 0 (reset_focus =
    // true), then the snap below sets the real target — the second always wins.
    // Keeping reset_focus = true documents that entry owns the focus, not the
    // residual viewer focus.
    refresh_library_carousel(ui, deps, true);
    snap_carousel_focus_to_last_opened(ui, deps.library, deps.search);
    // Restore keyboard focus to the carousel so its key seams work immediately.
    ui.invoke_focus_carousel();
}

/// Switch the app to the Viewer and sync the UI's `screen` property. The single
/// chokepoint for "go to Viewer"; restores focus to the page area so keyboard
/// navigation keeps working.
fn go_to_viewer(ui: &ViewerWindow, nav: &Rc<RefCell<NavState>>) {
    nav.borrow_mut().to_viewer();
    ui.set_screen(screen_to_index(nav.borrow().screen()));
    ui.invoke_focus_pages();
}

/// Convert core RGBA bytes into a `slint::Image`.
fn to_slint_image(decoded: &DecodedImage) -> slint::Image {
    let mut buffer =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(decoded.width(), decoded.height());
    buffer.make_mut_bytes().copy_from_slice(decoded.rgba());
    slint::Image::from_rgba8(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_or_default_returns_loaded_value_and_records_no_notice() {
        let mut errs: Vec<String> = Vec::new();
        let value: u32 = load_or_default("settings", || Ok(42), &mut errs);
        assert_eq!(value, 42);
        assert!(errs.is_empty(), "a successful load records no notice");
    }

    #[test]
    fn load_or_default_falls_back_to_default_and_records_labelled_notice_on_err() {
        let mut errs: Vec<String> = Vec::new();
        let value: u32 = load_or_default("library", || Err(CoreError::NoDataDir), &mut errs);
        assert_eq!(
            value,
            u32::default(),
            "a recoverable failure yields the type's default"
        );
        // The notice is the label with the error detail appended (derived from the
        // error's own Display, so this pins the format, not the wording).
        assert_eq!(errs, vec![format!("library ({})", CoreError::NoDataDir)]);
    }
}
