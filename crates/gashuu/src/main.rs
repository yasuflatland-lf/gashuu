// Windows release builds use the "windows" subsystem to suppress the extra console
// window; debug keeps the console for `tracing`. No-op on non-Windows targets.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

slint::include_modules!();

mod add_books;
mod add_controller;
mod carousel;
mod carousel_refresh;
mod cover_loader;
mod dialog_session;
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
mod ui_marshal;
mod update;
mod use_cases;
mod view_sync;
mod viewer_state;
mod viewport;
mod window_state;

pub(crate) use add_books::apply_outcomes;
pub(crate) use carousel_refresh::{
    apply_add_report, finalize_empty_book_rejected, finalize_remove, push_selection_toolbar_state,
    refresh_library_carousel, snap_carousel_focus_to_last_opened, visible_index_to_path,
    CarouselRefresh,
};
use dialog_session::DialogSession;
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
    current_book_name, route_view_modes_to_sink, write_back_position, ViewModeRoute,
};
#[cfg(not(test))]
use viewer_state::SpreadCacheState;
use viewer_state::{StatusContent, ViewerState};
use viewport::ViewportState;

/// Load a persisted value, falling back to its `Default` on a RECOVERABLE
/// failure. `label` names the source for both the `errs` notice
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

/// Rewrite an existing `settings.json` when it no longer matches the canonical
/// serialization of the loaded settings — i.e. the file was corrupt (recovered to
/// defaults) or carried out-of-bounds values that `Settings::normalize` repaired
/// (e.g. an inflated window size discarded for the default). This persists the
/// repair at startup so a bad file is fixed immediately instead of relying on the
/// next clean exit. A missing file is left untouched (a fresh install writes on its
/// first real save). Best-effort: a write failure is logged and surfaced, never
/// fatal.
fn repair_settings_file_if_needed(settings: &Settings, errs: &mut Vec<String>) {
    let Ok(path) = Settings::config_path() else {
        return;
    };
    if !path.exists() {
        return;
    }
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
    let Ok(canonical) = settings.to_json() else {
        return;
    };
    if canonical == on_disk {
        return;
    }
    if let Err(e) = settings.save_to(&path) {
        tracing::warn!(error = %e, "failed to rewrite repaired settings file");
        errs.push(format!("settings rewrite ({e})"));
    }
}

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    // ICU4X bundles no Japanese line-break dictionary, so Slint's text layout spams a
    // `log::warn!` per CJK run; silence the `icu_provider` target, keeping RUST_LOG.
    let env_filter = tracing_subscriber::EnvFilter::from_default_env().add_directive(
        "icu_provider=off"
            .parse()
            .expect("static directive is valid"),
    );
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    // Load persisted state, collecting notices to surface AFTER the initial refresh,
    // which overwrites status-text.
    let mut load_errs: Vec<String> = Vec::new();
    let settings = load_or_default("settings", Settings::load, &mut load_errs);
    // Self-heal an invalid settings file: in-memory values are already sane, but the
    // bad bytes persist. Rewrite at startup so a crash before clean exit can't lose it.
    repair_settings_file_if_needed(&settings, &mut load_errs);
    let library = match Library::load() {
        Ok(library) => library,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load library; using defaults");
            let notice = match Library::data_path() {
                Ok(path) if path.exists() => {
                    let unix_now_secs = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    match Library::quarantine_corrupt_file(&path, unix_now_secs) {
                        Ok(destination) => {
                            tracing::warn!(
                                path = %destination.display(),
                                "corrupt library file kept aside"
                            );
                            let destination_name = destination
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy();
                            format!("library ({e}); corrupt file kept as {destination_name}")
                        }
                        Err(quarantine_error) => {
                            tracing::warn!(
                                error = %quarantine_error,
                                path = %path.display(),
                                "failed to keep corrupt library file aside"
                            );
                            format!("library ({e})")
                        }
                    }
                }
                _ => format!("library ({e})"),
            };
            load_errs.push(notice);
            Library::new()
        }
    };

    let ui = ViewerWindow::new()?;
    // Boot the Fluent localizer with the persisted language; `apply()` pushes
    // every static string into the Strings global before the first paint.
    let localizer = Rc::new(i18n::Localizer::new(settings.language));
    localizer.push_strings_to_ui(&ui);
    // macOS' NSOpenPanel picks files AND folders in one panel, so the NavBar collapses
    // its two add capsules into one combined capsule. Compile-time constant.
    ui.set_combined_add_picker(cfg!(target_os = "macos"));
    let state = Rc::new(RefCell::new(ViewerState::from_settings(&settings)));
    let viewport = Rc::new(RefCell::new(ViewportState::from_settings(&settings)));
    let settings = Rc::new(RefCell::new(settings));
    let dialog_session = Rc::new(RefCell::new(DialogSession::new()));

    // The persisted shelf, shared so the carousel model build, the focused-index
    // clamp, and later PR-L's add / PR-R's position write-back can all reach it.
    let library = Rc::new(RefCell::new(library));

    // Thumbnail-strip controller: owns the backing model + generation bookkeeping
    // (epoch + cancel guard). `Rc` so both open handlers share one controller.
    let thumbs = Rc::new(ThumbnailController::new(&ui));

    // Cover controller for the carousel (epoch + cancel guard). `start` runs after the
    // model is (re)built so a refresh supersedes covers still streaming from the prior view.
    let covers = Rc::new(cover_loader::CoverController::new());

    // Page controller for the viewer body. Owns async page-decode epoch and
    // dispatch dedup for cache-miss spreads.
    let pages = Rc::new(page_loader::PageController::new());

    // Bulk-add controller (issue 206): probes each source off the UI thread so a bulk
    // add never freezes the event loop. `start` dispatches; `add-finalize` applies.
    let adder = Rc::new(add_controller::AddController::new());

    // Shared library-search filter state, so every path (search, add/open backfill,
    // open-time rebuild) projects the SAME visible-index set. Starts on the empty query.
    let search = Rc::new(RefCell::new(LibrarySearchState::default()));
    ui.set_library_search_query("".into());
    // Seed the visible set under the empty query so search state is consistent before
    // the first `refresh_library_carousel`, which only READS `visible_indices()`.
    search
        .borrow_mut()
        .set_query(String::new(), &library.borrow());

    // Shared bulk-selection state, keyed by path so it is orthogonal to the search
    // projection — a query change never drops a selection.
    let selection = Rc::new(RefCell::new(LibrarySelectionState::default()));

    // The "open a book" use-case, shared via `Rc` so the open flow lives in one place
    // (`use_cases::OpenBookUseCase`); `run` is headless and `finalize_open` applies the UI effects.
    let open_book = Rc::new(use_cases::OpenBookUseCase::new(
        Rc::clone(&state),
        Rc::clone(&settings),
        Rc::clone(&viewport),
        Rc::clone(&library),
    ));

    // Seed the carousel from the persisted library so boot shows saved books. This is
    // the single build+bind+focus-reset+cover-start path; cover streaming starts once here.
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
    // Continue reading: override the refresh's reset-to-0 with a one-shot snap to the
    // last-read book's visible row (resolved through the empty-query visible set).
    snap_carousel_focus_to_last_opened(&ui, &library, &search);

    // One startup sweep keeps the cover cache under its size cap + reclaims orphans
    // (issue 143). Dispatched AFTER the initial stream so visible covers get workers first.
    cover_loader::spawn_cache_prune();

    // Top-level screen state machine (boots to Library). `Rc<RefCell<…>>` so carousel
    // callbacks and the Viewer's GoToLibrary arm can flip it via the seam functions.
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

    // Surface load failures AFTER the initial refresh (which overwrites status-text).
    // Missing files return Ok(default), so this fires only on genuine failures.
    if !load_errs.is_empty() {
        ui.set_status_text(
            crate::i18n::dynamic::load_failed(localizer.loader(), &load_errs.join(" and ")).into(),
        );
    }

    // Wire all event handlers onto the window (handlers/, #151).
    handlers::wire_open_handlers(
        &ui, &state, &viewport, &settings, &library, &open_book, &covers, &pages, &adder, &search,
        &selection, &localizer,
    );
    handlers::wire_carousel_handlers(
        &ui, &state, &viewport, &library, &nav, &open_book, &covers, &pages, &thumbs, &search,
        &selection, &localizer,
    );
    handlers::wire_selection_handlers(
        &ui, &state, &library, &covers, &search, &selection, &localizer,
    );
    handlers::wire_viewer_input_handlers(&ui, &state, &viewport, &pages, &localizer);
    handlers::wire_settings_handlers(
        &ui,
        &state,
        &viewport,
        &settings,
        &dialog_session,
        &library,
        &covers,
        &pages,
        &search,
        &selection,
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
    // GitHub Releases update checker: wire the dialog/settings callbacks, then kick off
    // a throttled, non-forced background check. Reuses the shared settings cell.
    handlers::wire_update_handlers(&ui, &settings);
    handlers::start_update_check(&ui, &settings, false);

    // Restore the last window size + position before the first paint. No-op on a
    // fresh install; off-screen positions are dropped in favor of centering.
    window_state::restore_geometry(&ui, &settings.borrow());

    ui.run()?;
    // Persist page counts the cover prefetch resolved after the last refresh, so a book
    // counted this session isn't re-counted next launch. Safe: event loop exited.
    covers.flush_counts(&library);
    // Write the current reading position back to the library before exit. Safe: the
    // event loop has exited, so `state`/`library` are unborrowed.
    write_back_position(&state, &library);
    // Persist the open book's view modes to its override, mirroring into GLOBAL Settings
    // only when no book is open (ADR-0007 clobber guard lives in `route_view_modes_to_sink`).
    route_view_modes_to_sink(
        ViewModeRoute::AppExit,
        &state,
        &viewport,
        &settings,
        &library,
    );
    // Record the final window geometry so the next launch restores it. Safe: the window
    // handle is still alive (`ui` in scope) and `settings` is unborrowed.
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
    failed_trailing_page: Option<usize>,
    status: &StatusContent,
) {
    ui.set_single(single);
    let base_status = crate::i18n::dynamic::format_status(loader, status);
    match failed_trailing_page {
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
fn spread_request(slots: SpreadCacheState) -> SpreadDecodeRequest {
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
    match state.classify_spread() {
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
                // Mounting the LoadingSlot perturbs PageView's TouchArea hit-testing (phantom
                // `changed mouse-x`); suppress the pointer-reveal so chrome doesn't flip on MISS.
                ui.invoke_arm_pointer_reveal_suppression();

                #[cfg(not(test))]
                if let Some(dispatch) = state.dispatch_handle() {
                    pages.dispatch_spread(ui_weak, dispatch, spread_request(slots));
                }
            }
        }
        None => {
            // Source loaded but empty (or none yet): clear and show single so the view
            // matches the status text ("No folder opened" / "Folder contains no images").
            ui.set_status_text(crate::i18n::dynamic::format_status(loader, &content).into());
            pages.clear_target();
            clear_page_view(ui, viewport);
        }
    }
    // Keep the thumbnail-strip highlight in sync with the current spread's
    // leading page after every navigation/refresh.
    ui.set_current_index(state.index() as i32);

    // Seed the scrubber chrome (1-based) from the current spread; `double` mirrors
    // whether it has a trailing page. Display-only — does NOT change the page body.
    let total = state.page_count();
    let current_1based = current_page_1based(state);
    ui.set_scrubber_total_pages(total as i32);
    ui.set_scrubber_current_page(current_1based as i32);
    // `preview_is_double` resolves the trailing page with the SAME layout as the body
    // (decode-free), so it's the exact "has a trailing page" predicate without decoding.
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

/// Finalize an `open_book.run(path)` outcome on the UI — the headless use case
/// returns data and this applies every Slint effect. On failure, set the localized
/// error status; on success, `refresh()` the viewer, rebuild the carousel when the
/// open back-filled a page count (`count_changed`), launch thumbnails, and append
/// each localized notice. The single place the carousel-open and bookmark-jump sites
/// share this UI wiring, so the `OpenOutcome` match lives in exactly one spot.
fn finalize_open(
    ui: &ViewerWindow,
    state: &Rc<RefCell<ViewerState>>,
    viewport: &Rc<RefCell<ViewportState>>,
    pages: &PageController,
    thumbs: &Rc<ThumbnailController>,
    deps: &CarouselRefresh,
    outcome: use_cases::OpenOutcome,
) {
    let loader = deps.localizer.loader();
    match outcome {
        use_cases::OpenOutcome::Error(e_str) => {
            ui.set_status_text(crate::i18n::dynamic::open_error_str(loader, &e_str).into());
        }
        use_cases::OpenOutcome::Success {
            notices,
            count_changed,
        } => {
            pages.reset_for_source();
            refresh(ui, &state.borrow(), viewport, loader, pages, ui.as_weak());
            // On a page-count back-fill, rebuild the carousel preserving the ACTIVE filter
            // (moved out of the now-headless `OpenBookUseCase::run`). The chokepoint only
            // READS `visible_indices()`, so recompute the filter first.
            if count_changed {
                {
                    let lib = deps.library.borrow();
                    deps.search.borrow_mut().recompute(&lib);
                }
                // Page-count refresh, NOT a filter change: never reset the carousel's
                // focused index (matches the old open-coded path).
                refresh_library_carousel(ui, deps, false);
            }
            // Kick off parallel thumbnail generation for the newly opened source (success
            // path only; empty books early-return, failures are `Error`).
            thumbs.start(
                ui.as_weak(),
                state.borrow().current_source(),
                state.borrow().page_count(),
                state.borrow().open_file().map(std::path::Path::to_path_buf),
            );
            for detail in crate::i18n::dynamic::format_notices(loader, &notices) {
                let base = ui.get_status_text().to_string();
                ui.set_status_text(format!("{base} \u{2014} {detail}").into());
            }
        }
        use_cases::OpenOutcome::EmptyBookRejected {
            title,
            removed,
            save_error,
        } => {
            pages.reset_for_source();
            // Source opened cleanly but has zero pages: already removed + re-saved. This
            // arm does NOT switch screens (see the `enter_viewer` guard at the open sites).
            finalize_empty_book_rejected(
                ui,
                deps,
                &crate::open_book::EmptyBookOutcome {
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
    // Clear status-text on entry: the Library strip renders it, so the Viewer's page
    // status would otherwise leak under the carousel. Add/save feedback is set AFTER.
    ui.set_status_text("".into());
    // Rebuild so the model reflects the CURRENT `last_opened`, then override the
    // reset-to-0 with the continue-reading snap (reset_focus=true: entry owns focus).
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
