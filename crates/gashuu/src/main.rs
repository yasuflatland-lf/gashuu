slint::include_modules!();

mod keymap;
mod viewer_state;

use gashuu_core::DecodedImage;
use keymap::map_key;
use std::cell::RefCell;
use std::rc::Rc;
use viewer_state::ViewerState;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let ui = ViewerWindow::new()?;
    let state = Rc::new(RefCell::new(ViewerState::new()));

    ui.set_status_text(state.borrow().status_text().into());

    // Open Folder button: pick a directory, load it, refresh the view.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        ui.on_open_folder(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Some(dir) = rfd::FileDialog::new().pick_folder() else {
                return;
            };
            match state.borrow_mut().open_folder(&dir) {
                Ok(()) => tracing::info!(dir = %dir.display(), "opened folder"),
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
                let moved = state.borrow_mut().apply(action);
                if moved {
                    refresh(&ui, &state.borrow());
                }
            }
        });
    }

    ui.run()?;
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
        }
        None => {}
    }
}

/// Convert core RGBA bytes into a `slint::Image`.
fn to_slint_image(decoded: &DecodedImage) -> slint::Image {
    let mut buffer =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(decoded.width, decoded.height);
    buffer.make_mut_bytes().copy_from_slice(&decoded.rgba);
    slint::Image::from_rgba8(buffer)
}
