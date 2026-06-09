slint::include_modules!();

mod add_loader;
mod app;
mod carousel;
mod cover_loader;
mod enum_adapters;
mod handlers;
mod i18n;
mod keymap;
mod library_model;
mod navigation;
mod page_jump;
mod page_loader;
mod thumbnail_strip;
mod view_sync;
mod viewer_state;
mod viewport;

use carousel::{apply_selection_flags, bind_carousel_model, build_carousel_model, cover_requests};
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

    // Load persisted settings and library; corrupt/unreadable files fall back
    // to defaults (the corrupt-file recovery policy lives here in the UI layer,
    // by design). Missing files return Ok(default) from Settings::load /
    // Library::load, so the Err arm fires only on a GENUINE failure (corrupt
    // data, I/O error, NoDataDir). Errors are collected and surfaced on the
    // home screen after the initial refresh, which itself overwrites
    // status-text, so the notice must be set after that call.
    let mut load_errs: Vec<String> = Vec::new();
    let settings = match Settings::load() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load settings; using defaults");
            load_errs.push(format!("settings ({e})"));
            Settings::default()
        }
    };
    let library = match Library::load() {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load library; starting empty");
            load_errs.push(format!("library ({e})"));
            Library::new()
        }
    };

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
            // NOT switch screens — the open-folder/archive sites only switch on a
            // user gesture, and the carousel-open/bookmark sites skip their
            // `go_to_viewer` for this variant (see the `enter_viewer` guard there),
            // so the user is left on a refreshed Library. Rebuild the carousel
            // through the shared chokepoint so the removed book disappears and the
            // cover-epoch bump drops any in-flight cover for it; the active search
            // filter is preserved by the chokepoint. Do NOT reset focus.
            refresh_library_carousel(ui, deps, false);
            if removed {
                // `removed == true` means THIS path performed the removal, so it
                // owns the notice. A concurrent path that already removed+notified
                // yields `removed == false` (idempotent) and stays silent below.
                let status = empty_book_removed_status(loader, &title, save_error.as_deref());
                ui.set_status_text(status.into());
            }
            // `removed == false`: another path already removed+notified this book
            // (race idempotency), so add no notice — but the carousel rebuild
            // above still ran, keeping this screen consistent.
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

/// Outcome of an add batch: the canonical paths actually inserted (new books
/// only, in INPUT order) and the count of paths REJECTED because they could not
/// be opened as a book — either a source with zero image pages (the empty-book
/// rule) or an unreadable / unsupported source. Duplicates are NOT counted in
/// `skipped`: a path already in the library (or repeated within the batch) is
/// neither added nor rejected, mirroring `Library::add`'s `None`.
struct AddReport {
    added: Vec<std::path::PathBuf>,
    skipped: usize,
}

/// Apply already-probed sources to `lib`, the UI-thread APPLY half of the bulk
/// add (issue 206). The probe half runs off the UI thread (`add_loader::probe_path`
/// on rayon workers) so opening each archive never freezes the event loop; this
/// half takes the resulting [`add_loader::ProbeOutcome`]s — which the controller
/// has already re-sorted to INPUT order — and mutates the `!Send` `Library` here:
///
/// - `ProbeKind::Empty` — opened but zero image pages: skip and count in
///   `skipped` (the empty-book rule).
/// - `ProbeKind::FormatDisabled` / `ProbeKind::Unreadable` — could not be opened
///   as a book: skip, count in `skipped`, and log (the same level + detail the
///   old synchronous `add_paths` logged; logging is deferred to here so the probe
///   half stays pure).
/// - `ProbeKind::Counted(count)` — add via `Library::add` (canonicalizes, dedups,
///   re-sorts). On a genuine insert (`Some(canonical)`) the page count is recorded
///   immediately so a freshly added book shows "1 / N" without waiting for its
///   first open; a duplicate (`None`) is silently dropped (neither added nor
///   skipped).
///
/// Behaviour is byte-identical to the pre-206 synchronous `add_paths`; only the
/// probe was moved off-thread.
fn apply_outcomes(lib: &mut Library, outcomes: Vec<add_loader::ProbeOutcome>) -> AddReport {
    use add_loader::ProbeKind;
    let mut added = Vec::new();
    let mut skipped = 0usize;
    for add_loader::ProbeOutcome { path, kind, .. } in outcomes {
        match kind {
            ProbeKind::Empty => {
                skipped += 1;
                tracing::debug!(path = %path.display(), "skipping empty source (no image pages)");
            }
            ProbeKind::FormatDisabled { format } => {
                skipped += 1;
                tracing::info!(
                    path = %path.display(),
                    %format,
                    "skipping source: format disabled in safer mode"
                );
            }
            ProbeKind::Unreadable { error } => {
                skipped += 1;
                tracing::warn!(%error, path = %path.display(), "skipping unreadable source");
            }
            ProbeKind::Counted(count) => {
                if let Some(canonical) = lib.add(path).map(std::path::Path::to_path_buf) {
                    // Record the probed count on the freshly inserted book so it
                    // shows "1 / N" before its first open. `set_page_count`
                    // re-finds the book by its canonical path.
                    lib.set_page_count(&canonical, count);
                    added.push(canonical);
                }
                // `None` here means a duplicate (within the batch or already
                // present): neither added nor skipped, as before.
            }
        }
    }
    AddReport { added, skipped }
}

/// Resolve a VISIBLE carousel `index` to its underlying library book path,
/// through the search state's projection — the SAME hop `on_carousel_open` uses
/// (the carousel row is an index into `visible_indices`, which maps to a library
/// row). Returns `None` for an out-of-range index or a carousel/library desync.
/// Borrows `library` and `search` only for the duration of the call.
pub(crate) fn visible_index_to_path(
    library: &Rc<RefCell<Library>>,
    search: &Rc<RefCell<LibrarySearchState>>,
    index: i32,
) -> Option<std::path::PathBuf> {
    if index < 0 {
        return None;
    }
    let search = search.borrow();
    let library_index = search.visible_indices().get(index as usize).copied()?;
    let lib = library.borrow();
    lib.books()
        .get(library_index)
        .map(|book| book.path().to_path_buf())
}

/// Return the 0-based VISIBLE (filtered) carousel row index of `path`, or `None`
/// if `path` is not among the currently visible rows. `visible_indices` is the
/// search state's projection (library rows in natural order that pass the filter
/// or are forced visible); the returned position is the index INTO that slice,
/// i.e. the carousel row to focus. `path` must be a canonical path as returned by
/// `add_paths`.
fn visible_focus_index_for_path(
    lib: &Library,
    visible_indices: &[usize],
    path: &std::path::Path,
) -> Option<usize> {
    visible_indices.iter().position(|&library_index| {
        lib.books()
            .get(library_index)
            .is_some_and(|book| book.path() == path)
    })
}

/// Resolve the one-shot carousel `focused-index` to land on when ENTERING the
/// Library screen ("continue reading"): the VISIBLE row of `last_opened`, or `0`
/// as the fallback. The fallback covers every unresolvable case — no last-opened
/// book yet (`None`), the last-opened book filtered out of the current visible
/// set, an empty library, or a stale path no longer in `books`. Resolved THROUGH
/// the visible projection (`visible_indices` from the search state) so the index
/// is a carousel row, not a full-library index. Pure (library + projection in,
/// `i32` out) so it is headless-testable; the result is set ONCE at entry — after
/// that, user navigation owns `focused-index` (never a continuous binding).
fn entry_focus_index(lib: &Library, visible_indices: &[usize]) -> i32 {
    lib.last_opened()
        .and_then(|path| visible_focus_index_for_path(lib, visible_indices, path))
        .map(|index| index as i32)
        .unwrap_or(0)
}

/// Snap the carousel's `focused-index` to the last-read book ("continue reading")
/// when ENTERING the Library screen. A plain set at the entry moment — NOT a
/// binding — so user navigation owns focus afterwards. This OVERRIDES the
/// refresh's reset-to-0: a caller that just ran `refresh_library_carousel` with
/// `reset_focus = true` calls this immediately after, and the snap always wins.
/// Resolved through the CURRENT visible set (`entry_focus_index`); the borrow is
/// confined to the snap computation and drops before the UI set.
fn snap_carousel_focus_to_last_opened(
    ui: &ViewerWindow,
    library: &Rc<RefCell<Library>>,
    search: &Rc<RefCell<LibrarySearchState>>,
) {
    let focus = {
        let lib = library.borrow();
        entry_focus_index(&lib, search.borrow().visible_indices())
    };
    ui.set_carousel_focused_index(focus);
}

/// Clamp a carousel focused index into the valid range for a projection of
/// `visible_count` rows: `[0, visible_count - 1]`, or `0` when the projection is
/// empty. Pure so the destructive-delete refresh can pin the focused index to a
/// valid row BEFORE the Slint side reads it — an index past the shrunken
/// projection's end is the documented index-out-of-range crash risk. A negative
/// `old` (never produced by the live carousel, but defensive) floors to 0.
pub(crate) fn clamp_focused_index(old: i32, visible_count: usize) -> i32 {
    if visible_count == 0 {
        return 0;
    }
    let last = (visible_count - 1) as i32;
    old.clamp(0, last)
}

/// Push the selection-toolbar count text and select-all label into the UI.
///
/// Called from every point where the selection set or the visible projection
/// changes (toggle, select-all, exit, carousel rebuild, language switch, boot)
/// so the toolbar strings are always current without a full refresh.
///
/// Borrow discipline: `selection`, `search`, and `library` are distinct
/// `RefCell`s, so the three shared `Ref`s are taken together in one block scope
/// (both projection reads need the same trio) and drop at the block's `}` before
/// the UI setters run.
pub(crate) fn push_selection_strings(
    ui: &ViewerWindow,
    localizer: &i18n::Localizer,
    selection: &Rc<RefCell<LibrarySelectionState>>,
    search: &Rc<RefCell<LibrarySearchState>>,
    library: &Rc<RefCell<Library>>,
) {
    let loader = localizer.loader();
    // One shared-borrow group: `selection`, `search`, and `library` are distinct
    // `RefCell`s, so holding all three immutable `Ref`s at once is safe, and both
    // projection reads need the same trio. The group drops at the block's `}`.
    let (total, visible_selected, all_visible) = {
        let sel = selection.borrow();
        let srch = search.borrow();
        let lib = library.borrow();
        (
            sel.count(),
            sel.visible_selected_count(&srch, &lib),
            sel.all_visible_selected(&srch, &lib),
        )
    };
    ui.set_carousel_selection_count_text(
        crate::i18n::dynamic::selection_count_text(loader, total, visible_selected).into(),
    );
    ui.set_carousel_select_all_label(
        crate::i18n::dynamic::select_all_label(loader, all_visible).into(),
    );
    // The destructive toolbar twins: the pre-composed "Delete (N)…" label and the
    // `has-selection` gate (the DangerButton is disabled at N=0). Driven by the
    // TOTAL selection count, like the title, so they track every selection change.
    ui.set_carousel_delete_label(
        crate::i18n::dynamic::selection_delete_label(loader, total).into(),
    );
    ui.set_carousel_has_selection(total > 0);
}

/// The shared collaborators a Library carousel refresh threads together
/// (borrowed-collaborator bundle — same argument-count-cohesion intent as the
/// docs/patterns.md cohesion-wrapper flavor (`SpreadContext`), but holding `&Rc`
/// borrows rather than owned `Copy` values): the persisted `library`, the
/// `covers` stream controller, the `search` projection, the bulk-`selection`
/// state, and the `localizer` (for composing the selection-toolbar strings after
/// the projection changes). They ALWAYS travel together for a carousel rebuild,
/// so bundling them as borrows keeps `refresh_library_carousel` /
/// `apply_add_report` under the argument-count limit and documents that
/// they are one collaboration unit, not independent params.
struct CarouselRefresh<'a> {
    library: &'a Rc<RefCell<Library>>,
    covers: &'a cover_loader::CoverController,
    search: &'a Rc<RefCell<LibrarySearchState>>,
    selection: &'a Rc<RefCell<LibrarySelectionState>>,
    localizer: &'a Rc<i18n::Localizer>,
}

/// Project the CURRENT (already-recomputed) search state into the carousel:
/// rebuild + bind the filtered carousel model, optionally reset carousel focus
/// to row 0, and (re)start cover loading for the visible rows. The SINGLE place
/// the carousel + cover stream are refreshed from the shared search state, shared
/// by the initial boot build, the debounced query callback, and the add path.
///
/// This function only READS `visible_indices()`; it does NOT recompute. Every
/// caller mutates the search state through a recomputing entry point first —
/// `set_query` (startup seed + search-changed) or `force_visible` (add) — so the
/// visible set is already consistent here, avoiding a redundant double-recompute.
///
/// Borrow discipline: all reads share ONE `library.borrow()` scope that drops
/// before the UI bind and `covers.start` (which takes a `borrow_mut` to persist
/// any prefetched page counts) — never hold a `borrow()` across `start`.
fn refresh_library_carousel(ui: &ViewerWindow, deps: &CarouselRefresh, reset_focus: bool) {
    // Read everything the refresh needs under a single borrow, then drop it
    // before the UI mutations and `covers.start`.
    let (book_count, model, cover_reqs, indices) = {
        let lib = deps.library.borrow();
        let indices = deps.search.borrow().visible_indices().to_vec();
        (
            lib.books().len() as i32,
            build_carousel_model(&lib, &indices),
            cover_requests(&lib, &indices),
            indices,
        )
    };

    ui.set_library_book_count(book_count);
    // Idle bottom-strip label: the total library size, shown when no transient
    // notice occupies the strip (the count is the strip's idle state).
    ui.set_library_count_text(
        crate::i18n::dynamic::library_count_text(deps.localizer.loader(), book_count as usize)
            .into(),
    );
    bind_carousel_model(ui, model);
    if reset_focus {
        ui.set_carousel_focused_index(0);
    }
    // Re-apply the bulk selection over the freshly built (unselected) rows so a
    // selection survives a query change / add (selection is keyed by path, not
    // index). Reads `library` + `selection`; both `Ref`s drop before `covers.start`
    // (which takes a `borrow_mut` to persist prefetched counts).
    {
        let lib = deps.library.borrow();
        let selection = deps.selection.borrow();
        apply_selection_flags(ui, &lib, &indices, |path| selection.contains(path));
    }
    // Refresh the selection-toolbar strings: the visible projection just changed
    // (query change, add, boot), so visible_selected_count / all_visible_selected
    // may have moved. Both `Ref`s drop before `covers.start`.
    push_selection_strings(
        ui,
        deps.localizer,
        deps.selection,
        deps.search,
        deps.library,
    );
    // Dispatch covers nearest the focused row first: on a large library the
    // visible neighbourhood streams in immediately instead of queueing behind
    // hundreds of off-screen rows. Read the focus AFTER the reset above so a
    // reset-focus refresh starts from row 0. (The add path moves focus to the
    // new book only after this refresh; its cover is a fresh miss either way,
    // so ordering by the pre-add focus is fine there.)
    let focus_row = ui.get_carousel_focused_index().max(0) as usize;
    deps.covers.start(
        ui.as_weak(),
        deps.library,
        cover_loader::prioritize_by_focus(cover_reqs, focus_row),
    );
}

/// Which status notice to surface after `apply_outcomes` applies a probed batch.
///
/// The four arms cover the full 2×2 of (added==0 vs added>0) × (skipped==0 vs
/// skipped>0).  The save-failure arm is handled separately in
/// `apply_add_report` and is NOT part of this enum.
#[derive(Debug, PartialEq)]
enum AddNotice {
    /// All picked paths were already in the library (no new additions, no rejections).
    AlreadyInLibrary,
    /// Every path was rejected (no images or unreadable); nothing was added.
    NoneAddedAllSkipped { skipped: usize },
    /// Some books were added and some paths were rejected.
    AddedWithSkips { added: usize, skipped: usize },
    /// All picked paths were added successfully; none were rejected.
    Added { added: usize },
}

/// Pure decision function: maps the `(added, skipped)` counts from `add_paths`
/// to the appropriate [`AddNotice`] variant.  No I/O, no side-effects.
fn select_add_notice(added: usize, skipped: usize) -> AddNotice {
    match (added, skipped) {
        (0, 0) => AddNotice::AlreadyInLibrary,
        (0, s) => AddNotice::NoneAddedAllSkipped { skipped: s },
        (n, 0) => AddNotice::Added { added: n },
        (n, s) => AddNotice::AddedWithSkips {
            added: n,
            skipped: s,
        },
    }
}

/// Apply an already-computed add `report` to the library: persist, rebuild the
/// filtered carousel, and surface the outcome on the status line, restoring
/// carousel focus in every case. The UI-thread tail of the bulk add (issue 206),
/// run from the `add-finalize` handler once the off-thread probe completes (and
/// the `apply_outcomes` mutation has produced the `report`).
///
/// Shared by the Add Books and Add Folder paths; `op` distinguishes the two only
/// in the save-failure trace message. When nothing new was added there is nothing
/// to persist or rebuild, so it short-circuits after the status update.
///
/// Sources with no image pages (or that cannot be opened) were rejected by the
/// probe + `apply_outcomes` before they entered the library; the status notice
/// names how many were skipped (added-some-skipped-some, or none-added-all-empty),
/// falling back to the already-in-library message only when the skip count is zero.
///
/// Newly added books are FORCED visible under the active filter (so an add never
/// silently hides the new book behind a non-matching query); the filter text
/// stays in place, and the forced override is cleared on the next user query
/// change (see `LibrarySearchState::set_query`).
fn apply_add_report(
    ui: &ViewerWindow,
    deps: &CarouselRefresh,
    report: AddReport,
    op: &'static str,
    loader: &i18n_embed::fluent::FluentLanguageLoader,
) {
    let AddReport {
        added: added_paths,
        skipped,
    } = report;
    if added_paths.is_empty() {
        // Nothing new entered the library: nothing to persist or rebuild. Route
        // through the pure decision fn so every branch is testable without Slint.
        let notice = match select_add_notice(0, skipped) {
            AddNotice::NoneAddedAllSkipped { skipped: s } => {
                crate::i18n::dynamic::no_books_added_empty(loader, s)
            }
            _ => crate::i18n::dynamic::already_in_library(loader),
        };
        ui.set_status_text(notice.into());
        ui.invoke_focus_carousel();
        return;
    }
    // Rebuild from the in-memory state even if the save fails, so the newly added
    // books are visible; the save error is then surfaced (not just traced). Keep
    // the just-added paths visible under the active filter, then refresh through
    // the shared chokepoint (which recomputes the filter, rebuilds + binds the
    // model, and restarts the cover stream). Focus is set explicitly below to the
    // new book's visible row, so do NOT reset focus to 0 here.
    let save_result = deps.library.borrow().save();
    // `search` and `library` are distinct RefCells, so the mut borrow of one and
    // the shared borrow of the other cannot conflict; the `library.borrow()`
    // drops at the `;` before refresh. `force_visible` recomputes internally, so
    // the visible set is consistent before `refresh_library_carousel` reads it.
    deps.search
        .borrow_mut()
        .force_visible(added_paths.clone(), &deps.library.borrow());
    refresh_library_carousel(ui, deps, false);
    match save_result {
        Err(e) => {
            tracing::error!(error = %e, "failed to save library after {op}");
            ui.set_status_text(
                crate::i18n::dynamic::added_books_save_failed(loader, added_paths.len(), &e).into(),
            );
        }
        Ok(()) => {
            // Some books were added; route through the pure decision fn so the
            // 4-way mapping is testable without Slint.
            let notice = match select_add_notice(added_paths.len(), skipped) {
                AddNotice::AddedWithSkips {
                    added: n,
                    skipped: s,
                } => crate::i18n::dynamic::added_books_skipped(loader, n, s),
                AddNotice::Added { added: n } => crate::i18n::dynamic::added_books(loader, n),
                // added_paths is non-empty here, so AlreadyInLibrary and
                // NoneAddedAllSkipped are unreachable; exhaustive for safety.
                _ => crate::i18n::dynamic::added_books(loader, added_paths.len()),
            };
            ui.set_status_text(notice.into());
        }
    }
    // Focus the first newly added book by its VISIBLE row (the carousel renders
    // the filtered slice), not its full-library index.
    if let Some(first_path) = added_paths.first() {
        let index = {
            let lib = deps.library.borrow();
            let search = deps.search.borrow();
            visible_focus_index_for_path(&lib, search.visible_indices(), first_path)
        };
        if let Some(index) = index {
            ui.set_carousel_focused_index(index as i32);
        } else {
            // force_visible(added_paths) + recompute guarantees the just-added book is
            // a visible row, so this is unreachable in practice. Fail loudly in dev/test;
            // in release, log and fall through (focus stays on the carousel via the
            // unconditional invoke_focus_carousel below).
            debug_assert!(
                false,
                "add: forced-visible book {} not found in visible rows",
                first_path.display()
            );
            tracing::warn!(
                path = %first_path.display(),
                "add: forced-visible book not found in visible rows; focus not restored"
            );
        }
    }
    ui.invoke_focus_carousel();
}

#[cfg(test)]
mod tests {
    use super::*;
    use gashuu_core::ArchivePolicy;

    /// Test convenience around the split add: probe `paths` synchronously in
    /// input order, then apply the outcomes. This is the pre-206 `add_paths`
    /// behaviour, retained so the apply-half tests below exercise the real
    /// `apply_outcomes` mutation path through one call. Production no longer has a
    /// synchronous `add_paths` — the probe runs off the UI thread (`add_loader`)
    /// and the apply runs in the `add-finalize` handler — but the probe + apply
    /// halves are unchanged in behaviour, so testing them composed is faithful.
    fn add_paths(
        lib: &mut Library,
        paths: Vec<std::path::PathBuf>,
        policy: ArchivePolicy,
    ) -> AddReport {
        let outcomes = paths
            .into_iter()
            .enumerate()
            .map(|(index, path)| add_loader::probe_path(index, path, policy))
            .collect();
        apply_outcomes(lib, outcomes)
    }

    // ---- add_paths (empty-book rule) -------------------------------------
    //
    // Since the empty-book rule, `add_paths` PROBES each source before insert:
    // a source must contain at least one image page to be added. A folder is the
    // cheapest fixture — a zero-byte `*.png` counts as a page (listing is
    // extension-based), an empty folder probes to `EmptyBook`, and a nonexistent
    // path probes to an I/O error. These helpers build real temp dirs so probing
    // sees a genuine filesystem (the same reason the older tests already used
    // tempdirs: `Library::add` canonicalizes).

    /// Create a fresh temp directory under `parent/<name>` holding `pages`
    /// zero-byte `*.png` files (so it probes to a `pages`-page book). With
    /// `pages == 0` the directory is empty and probes to `EmptyBook`. Returns the
    /// directory path (its canonical form is what `Library::add` stores).
    fn make_book_dir(parent: &std::path::Path, name: &str, pages: usize) -> std::path::PathBuf {
        let dir = parent.join(name);
        std::fs::create_dir_all(&dir).expect("create book dir");
        for i in 0..pages {
            std::fs::write(dir.join(format!("page{i:03}.png")), []).expect("write page");
        }
        dir
    }

    /// Canonicalize a path the same way `Library::add` does, so test expectations
    /// match the stored/returned canonical paths.
    fn canon(path: &std::path::Path) -> std::path::PathBuf {
        path.canonicalize().expect("canonicalize existing path")
    }

    #[test]
    fn add_paths_empty_vec_returns_zero() {
        let mut lib = gashuu_core::Library::new();
        let report = add_paths(&mut lib, vec![], ArchivePolicy::default());
        assert!(report.added.is_empty());
        assert_eq!(report.skipped, 0);
        assert_eq!(lib.books().len(), 0);
    }

    #[test]
    fn add_paths_new_paths_counted() {
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let vol1 = make_book_dir(root.path(), "vol1", 1);
        let vol2 = make_book_dir(root.path(), "vol2", 2);
        let report = add_paths(
            &mut lib,
            vec![vol1.clone(), vol2.clone()],
            ArchivePolicy::default(),
        );
        assert_eq!(report.added.len(), 2);
        assert_eq!(report.skipped, 0);
        assert_eq!(lib.books().len(), 2);
        // The returned vec holds the CANONICAL paths in INPUT order.
        assert_eq!(report.added, vec![canon(&vol1), canon(&vol2)]);
    }

    #[test]
    fn add_paths_dedup_within_batch() {
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let vol1 = make_book_dir(root.path(), "vol1", 1);
        let report = add_paths(
            &mut lib,
            vec![vol1.clone(), vol1.clone()],
            ArchivePolicy::default(),
        );
        assert_eq!(
            report.added.len(),
            1,
            "duplicate within the batch must not be double-counted"
        );
        // A duplicate is neither added nor rejected, so it is NOT counted as skipped.
        assert_eq!(
            report.skipped, 0,
            "a duplicate is not an empty/unreadable skip"
        );
        assert_eq!(lib.books().len(), 1);
    }

    #[test]
    fn add_paths_dedup_against_existing() {
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let vol1 = make_book_dir(root.path(), "vol1", 1);
        let vol2 = make_book_dir(root.path(), "vol2", 1);
        lib.add(vol1.clone());
        let report = add_paths(
            &mut lib,
            vec![vol1.clone(), vol2.clone()],
            ArchivePolicy::default(),
        );
        assert_eq!(
            report.added.len(),
            1,
            "a path already in the library must not be counted"
        );
        assert_eq!(
            report.skipped, 0,
            "an existing path is not an empty/unreadable skip"
        );
        assert_eq!(lib.books().len(), 2);
    }

    #[test]
    fn add_paths_returns_canonical_paths_and_skips_duplicates() {
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let vol1 = make_book_dir(root.path(), "vol1", 1);
        // `vol1/.` and `vol1` canonicalize to the same path, so the second is a
        // duplicate and dropped.
        let with_dot = vol1.join(".");
        let expected = canon(&vol1);
        let report = add_paths(
            &mut lib,
            vec![with_dot.clone(), with_dot.clone()],
            ArchivePolicy::default(),
        );
        assert_eq!(report.added, vec![expected.clone()]);
        assert_eq!(report.skipped, 0);
        assert_eq!(lib.books().len(), 1);
        assert_eq!(lib.books()[0].path(), expected.as_path());
    }

    #[test]
    fn add_paths_all_existing_returns_zero() {
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let vol1 = make_book_dir(root.path(), "vol1", 1);
        let vol2 = make_book_dir(root.path(), "vol2", 1);
        lib.add(vol1.clone());
        lib.add(vol2.clone());
        let before = lib.books().len();
        let report = add_paths(
            &mut lib,
            vec![vol1.clone(), vol2.clone()],
            ArchivePolicy::default(),
        );
        assert!(report.added.is_empty(), "all-duplicate batch must add 0");
        assert_eq!(report.skipped, 0, "duplicates are not skips");
        assert_eq!(lib.books().len(), before, "books count must not change");
    }

    #[test]
    fn add_paths_mixed_batch_counts_added_and_skipped() {
        // A valid book, an empty folder, and a duplicate of the valid book:
        // 1 added, 1 skipped (the empty), and the duplicate dropped silently.
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let valid = make_book_dir(root.path(), "valid", 1);
        let empty = make_book_dir(root.path(), "empty", 0);
        let report = add_paths(
            &mut lib,
            vec![valid.clone(), empty.clone(), valid.clone()],
            ArchivePolicy::default(),
        );
        assert_eq!(
            report.added,
            vec![canon(&valid)],
            "only the valid book is added"
        );
        assert_eq!(report.skipped, 1, "the empty folder is the one skip");
        assert_eq!(lib.books().len(), 1);
        assert_eq!(lib.books()[0].path(), canon(&valid).as_path());
    }

    #[test]
    fn add_paths_all_empty_batch_adds_zero_skips_all() {
        // Every picked source is empty: nothing added, all counted as skipped.
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let e1 = make_book_dir(root.path(), "e1", 0);
        let e2 = make_book_dir(root.path(), "e2", 0);
        let e3 = make_book_dir(root.path(), "e3", 0);
        let report = add_paths(&mut lib, vec![e1, e2, e3], ArchivePolicy::default());
        assert!(
            report.added.is_empty(),
            "no book added from an all-empty batch"
        );
        assert_eq!(report.skipped, 3, "all three empty sources are skipped");
        assert_eq!(lib.books().len(), 0);
    }

    #[test]
    fn add_paths_unreadable_path_is_skipped() {
        // A nonexistent path cannot be opened (I/O error), so it is rejected as a
        // skip — never added (an "unreadable" source is NOT classified as empty,
        // but is still kept out of the library).
        let mut lib = gashuu_core::Library::new();
        let report = add_paths(
            &mut lib,
            vec![std::path::PathBuf::from(
                "/nonexistent_gashuu_add_paths_unreadable",
            )],
            ArchivePolicy::default(),
        );
        assert!(report.added.is_empty(), "an unreadable path is never added");
        assert_eq!(
            report.skipped, 1,
            "the unreadable path is counted as skipped"
        );
        assert_eq!(lib.books().len(), 0);
    }

    #[test]
    fn add_paths_sets_page_count_immediately() {
        // A freshly added book carries its probed page count so the carousel can
        // show "1 / N" before the book is ever opened.
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let three = make_book_dir(root.path(), "three", 3);
        let report = add_paths(&mut lib, vec![three.clone()], ArchivePolicy::default());
        assert_eq!(report.added.len(), 1);
        assert_eq!(report.skipped, 0);
        let book = lib
            .books()
            .iter()
            .find(|b| b.path() == canon(&three))
            .expect("added book present");
        assert_eq!(
            book.page_count_opt(),
            Some(3),
            "the probed page count is recorded on add"
        );
    }

    #[test]
    fn visible_focus_index_for_path_uses_filtered_rows() {
        let mut lib = Library::new();
        assert!(lib
            .add(std::path::PathBuf::from("/manga/alpha.cbz"))
            .is_some());
        assert!(lib
            .add(std::path::PathBuf::from("/manga/beta.cbz"))
            .is_some());
        assert!(lib
            .add(std::path::PathBuf::from("/manga/gamma.cbz"))
            .is_some());

        let path = lib.books()[2].path().to_path_buf();
        // gamma is the only visible row, so its visible position is 0.
        assert_eq!(visible_focus_index_for_path(&lib, &[2], &path), Some(0));
        // gamma is not among the visible rows, so it has no visible position.
        assert_eq!(visible_focus_index_for_path(&lib, &[0, 1], &path), None);
    }

    // ---- entry_focus_index (continue reading) ----------------------------

    /// Build a 3-book library (alpha/beta/gamma, natural order) and mark `index`
    /// as last-opened via `register_opened`. Returns the library; the natural
    /// order is alpha(0), beta(1), gamma(2) since the paths sort lexically.
    fn library_with_last_opened(index: usize) -> Library {
        let mut lib = Library::new();
        for leaf in ["alpha", "beta", "gamma"] {
            assert!(lib
                .add(std::path::PathBuf::from(format!("/manga/{leaf}.cbz")))
                .is_some());
        }
        let path = lib.books()[index].path().to_path_buf();
        lib.register_opened(&path, None);
        lib
    }

    #[test]
    fn entry_focus_index_resolves_last_opened_with_no_filter() {
        // beta is last-opened and all three rows are visible (natural-order
        // identity projection), so the snap lands on beta's row (1).
        let lib = library_with_last_opened(1);
        assert_eq!(entry_focus_index(&lib, &[0, 1, 2]), 1);
    }

    #[test]
    fn entry_focus_index_uses_filtered_position() {
        // gamma is last-opened and the visible set is the filtered slice
        // [beta, gamma]; gamma's VISIBLE row is 1, not its library index 2.
        let lib = library_with_last_opened(2);
        assert_eq!(entry_focus_index(&lib, &[1, 2]), 1);
    }

    #[test]
    fn entry_focus_index_falls_back_when_last_opened_filtered_out() {
        // alpha is last-opened but the filter hides it (visible set is gamma
        // only), so the snap falls back to row 0.
        let lib = library_with_last_opened(0);
        assert_eq!(entry_focus_index(&lib, &[2]), 0);
    }

    #[test]
    fn entry_focus_index_falls_back_when_no_last_opened() {
        // A fresh library has never opened a book, so the fallback is row 0.
        let mut lib = Library::new();
        assert!(lib
            .add(std::path::PathBuf::from("/manga/alpha.cbz"))
            .is_some());
        assert_eq!(lib.last_opened(), None, "no book opened yet");
        assert_eq!(entry_focus_index(&lib, &[0]), 0);
    }

    #[test]
    fn entry_focus_index_falls_back_for_empty_library() {
        // No books and no visible rows: the fallback is row 0 (never panics on
        // the empty slice).
        let lib = Library::new();
        assert_eq!(entry_focus_index(&lib, &[]), 0);
    }

    // ---- clamp_focused_index (bulk-delete focus safety) -------------------

    #[test]
    fn clamp_focused_index_pins_into_shrunken_projection() {
        // The crash guard: after a bulk delete shrinks the projection, a focused
        // index past the new last row must clamp DOWN to the last valid row.
        assert_eq!(clamp_focused_index(7, 3), 2, "past-end clamps to last row");
        // An empty projection (everything deleted) floors to 0.
        assert_eq!(clamp_focused_index(7, 0), 0, "empty projection floors to 0");
        assert_eq!(clamp_focused_index(0, 0), 0, "0 on empty stays 0");
        // An in-range index is preserved unchanged.
        assert_eq!(clamp_focused_index(1, 4), 1, "in-range index unchanged");
        assert_eq!(clamp_focused_index(0, 1), 0, "single row keeps focus at 0");
        // A negative index (defensive; not produced by the live carousel) floors.
        assert_eq!(clamp_focused_index(-1, 4), 0, "negative floors to 0");
    }

    #[test]
    fn add_paths_returns_input_order_while_books_are_natural_order() {
        // Focus follows the FIRST input path, not natural order: `add_paths`
        // returns the inserted paths in INPUT order, whereas `lib.books()` keeps
        // them in NATURAL (sorted) order. Both share one parent dir so their leaf
        // names (vol1, vol10) drive the natural sort.
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let vol10 = canon(&make_book_dir(root.path(), "vol10", 1));
        let vol1 = canon(&make_book_dir(root.path(), "vol1", 1));
        let report = add_paths(
            &mut lib,
            vec![vol10.clone(), vol1.clone()],
            ArchivePolicy::default(),
        );

        // Returned vec is in INPUT order (vol10 first, vol1 second).
        assert_eq!(report.added[0], vol10);
        assert_eq!(report.added[1], vol1);

        // The library itself is in NATURAL order (vol1 before vol10).
        let books: Vec<_> = lib
            .books()
            .iter()
            .map(|book| book.path().to_path_buf())
            .collect();
        assert_eq!(books, vec![vol1, vol10]);
    }

    // Note: `build_carousel_model` is now headless (it builds the model from
    // visible indices; `bind_carousel_model` does the UI bind), and is unit-tested
    // directly in `carousel::tests`. The Library -> carousel row mapping invariants
    // (length, 1-based `current`, availability, natural `Library::books()` order)
    // are covered by `library_model::tests` against the pure `carousel_data` /
    // `carousel_data_for_indices` helpers that the builder delegates to.

    // ---- select_add_notice (reject-empty-books status routing) --------------

    #[test]
    fn select_add_notice_already_in_library_when_both_zero() {
        assert_eq!(select_add_notice(0, 0), AddNotice::AlreadyInLibrary);
    }

    #[test]
    fn select_add_notice_none_added_all_skipped_when_added_zero_skipped_nonzero() {
        assert_eq!(
            select_add_notice(0, 3),
            AddNotice::NoneAddedAllSkipped { skipped: 3 }
        );
    }

    #[test]
    fn add_paths_rar_blocked_by_policy_is_skipped_not_added() {
        // A .cbr file is rejected at probe time when allow_rar=false; it must be
        // counted as skipped, not added, and must not enter the library.
        let root = tempfile::tempdir().expect("tempdir");
        let cbr = root.path().join("manga.cbr");
        // Extension check fires before any bytes are read; any content works.
        std::fs::write(&cbr, b"dummy").expect("write dummy cbr");

        let mut lib = gashuu_core::Library::new();
        let policy = ArchivePolicy { allow_rar: false };
        let report = add_paths(&mut lib, vec![cbr], policy);

        assert!(report.added.is_empty(), "blocked RAR must never be added");
        assert_eq!(report.skipped, 1, "blocked RAR must be counted as skipped");
        assert_eq!(lib.books().len(), 0);
    }

    #[test]
    fn select_add_notice_added_with_skips_when_both_nonzero() {
        assert_eq!(
            select_add_notice(2, 1),
            AddNotice::AddedWithSkips {
                added: 2,
                skipped: 1
            }
        );
    }

    #[test]
    fn select_add_notice_added_when_added_nonzero_skipped_zero() {
        assert_eq!(select_add_notice(5, 0), AddNotice::Added { added: 5 });
    }
}
