slint::include_modules!();

mod keymap;
mod viewer_state;

use gashuu_core::{DecodedImage, ReadingDirection, Settings};
use keymap::{map_key, KeyCommand};
use std::cell::RefCell;
use std::rc::Rc;
use viewer_state::ViewerState;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Load persisted settings; corrupt/unreadable files fall back to defaults
    // (the corrupt-file recovery policy lives here in the UI layer, by design).
    let settings = Settings::load().unwrap_or_else(|e| {
        tracing::warn!(error = %e, "failed to load settings; using defaults");
        Settings::default()
    });

    let ui = ViewerWindow::new()?;
    let state = Rc::new(RefCell::new(ViewerState::from_settings(&settings)));
    let settings = Rc::new(RefCell::new(settings));

    // Initial paint so rtl/single/status are all initialized before the first
    // folder is opened (refresh shows "No folder opened" and clears the images).
    refresh(&ui, &state.borrow());

    // Open Folder button: pick a directory, load it, refresh the view.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        ui.on_open_folder(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Some(dir) = rfd::FileDialog::new().pick_folder() else {
                return;
            };
            match state.borrow_mut().open_folder(&dir) {
                Ok(()) => {
                    tracing::info!(dir = %dir.display(), "opened folder");
                    let mut s = settings.borrow_mut();
                    if s.track_recent_files {
                        s.push_recent(dir.clone());
                        if let Err(e) = s.save() {
                            tracing::error!(error = %e, "failed to save settings");
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to open folder");
                    ui.set_status_text(format!("Error: {e}").into());
                    return;
                }
            }
            refresh(&ui, &state.borrow());
        });
    }

    // Keyboard navigation forwarded from the FocusScope.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        ui.on_nav(move |token| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let dir = state.borrow().reading_direction();
            let Some(cmd) = map_key(token.as_str(), dir) else {
                return;
            };
            let started = std::time::Instant::now();
            match cmd {
                KeyCommand::Turn(action) => {
                    let moved = state.borrow_mut().apply(action);
                    if moved {
                        refresh(&ui, &state.borrow());
                    }
                    // Log every page-turn latency (cache hits target <50ms; the
                    // first visit to a page also includes a synchronous decode).
                    // Observe with RUST_LOG=debug.
                    tracing::debug!(
                        elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                        moved,
                        "page turn"
                    );
                }
                KeyCommand::ToggleSpread => {
                    if state.borrow_mut().toggle_spread() {
                        settings.borrow_mut().spread_mode = state.borrow().spread_mode();
                        refresh(&ui, &state.borrow());
                    }
                }
                KeyCommand::ToggleReadingDirection => {
                    if state.borrow_mut().toggle_reading_direction() {
                        settings.borrow_mut().reading_direction =
                            state.borrow().reading_direction();
                        refresh(&ui, &state.borrow());
                    }
                }
                KeyCommand::ToggleCover => {
                    if state.borrow_mut().toggle_cover() {
                        settings.borrow_mut().cover_mode = state.borrow().cover_mode();
                        refresh(&ui, &state.borrow());
                    }
                }
            }
        });
    }

    ui.run()?;
    // Persist settings on exit so even a first run writes a file the user can
    // hand-edit. Save failure is logged, not fatal.
    if let Err(e) = settings.borrow().save() {
        tracing::error!(error = %e, "failed to save settings on exit");
    }
    Ok(())
}

/// Push the current spread + status into the UI.
fn refresh(ui: &ViewerWindow, state: &ViewerState) {
    ui.set_status_text(state.status_text().into());
    ui.set_rtl(matches!(state.reading_direction(), ReadingDirection::Rtl));
    match state.current_spread() {
        Some(Ok(spread)) => {
            ui.set_leading_page(to_slint_image(&spread.leading));
            match spread.trailing {
                Some(trailing) => {
                    ui.set_trailing_page(to_slint_image(&trailing));
                    ui.set_single(false);
                }
                None => {
                    ui.set_trailing_page(slint::Image::default());
                    ui.set_single(true);
                }
            }
        }
        Some(Err(e)) => {
            tracing::error!(error = %e, "failed to decode page");
            ui.set_status_text(format!("Decode error: {e}").into());
            ui.set_leading_page(slint::Image::default());
            ui.set_trailing_page(slint::Image::default());
            ui.set_single(true);
        }
        None => {
            // Source loaded but empty (or no source yet): clear and show single
            // so the view matches the status text.
            ui.set_leading_page(slint::Image::default());
            ui.set_trailing_page(slint::Image::default());
            ui.set_single(true);
        }
    }
}

/// Convert core RGBA bytes into a `slint::Image`.
fn to_slint_image(decoded: &DecodedImage) -> slint::Image {
    let mut buffer =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(decoded.width(), decoded.height());
    buffer.make_mut_bytes().copy_from_slice(decoded.rgba());
    slint::Image::from_rgba8(buffer)
}
