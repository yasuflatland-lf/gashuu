slint::include_modules!();

mod keymap;
mod viewer_state;

use gashuu_core::{DecodedImage, Settings};
use keymap::map_key;
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
    let state = Rc::new(RefCell::new(ViewerState::with_cache_config(
        settings.cache_size,
        settings.preload_pages,
    )));
    let settings = Rc::new(RefCell::new(settings));

    ui.set_status_text(state.borrow().status_text().into());

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
        ui.on_nav(move |token| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if let Some(action) = map_key(token.as_str()) {
                let started = std::time::Instant::now();
                let moved = state.borrow_mut().apply(action);
                if moved {
                    refresh(&ui, &state.borrow());
                }
                // Log every page-turn latency (cache hits target <50ms; the first
                // visit to a page also includes a synchronous decode). Observe with
                // RUST_LOG=debug.
                tracing::debug!(
                    elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                    moved,
                    "page turn"
                );
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

/// Push the current page + status into the UI.
fn refresh(ui: &ViewerWindow, state: &ViewerState) {
    ui.set_status_text(state.status_text().into());
    match state.current_image() {
        Some(Ok(image)) => ui.set_current_page(to_slint_image(&image)),
        Some(Err(e)) => {
            tracing::error!(error = %e, "failed to decode page");
            ui.set_status_text(format!("Decode error: {e}").into());
            // Clear the stale page so the view matches the error status rather than
            // showing the previously decoded page.
            ui.set_current_page(slint::Image::default());
        }
        None => {
            // Source loaded but empty (no decodable page): clear any stale image
            // so the view matches the "Folder contains no images" status.
            ui.set_current_page(slint::Image::default());
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
