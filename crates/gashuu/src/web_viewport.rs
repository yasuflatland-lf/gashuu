use crate::ViewerWindow;
use slint::{ComponentHandle, LogicalSize};
use web_sys::wasm_bindgen::{closure::Closure, JsCast};

/// Match the Slint window to the browser viewport for the lifetime of the page.
pub(crate) fn follow_browser_viewport(ui: &ViewerWindow) {
    let Some(browser) = web_sys::window() else {
        return;
    };

    resize_to_viewport(ui, &browser);

    let ui = ui.as_weak();
    let resize_browser = browser.clone();
    let resize = Closure::<dyn FnMut()>::new(move || {
        if let Some(ui) = ui.upgrade() {
            resize_to_viewport(&ui, &resize_browser);
        }
    });
    match browser.add_event_listener_with_callback("resize", resize.as_ref().unchecked_ref()) {
        Ok(()) => resize.forget(),
        Err(error) => tracing::warn!(?error, "could not listen for browser viewport changes"),
    }
}

fn resize_to_viewport(ui: &ViewerWindow, browser: &web_sys::Window) {
    if let Some(size) = viewport_size(browser) {
        ui.window().set_size(size);
    }
}

fn viewport_size(browser: &web_sys::Window) -> Option<LogicalSize> {
    let width = browser.inner_width().ok()?.as_f64()? as f32;
    let height = browser.inner_height().ok()?.as_f64()? as f32;
    Some(LogicalSize::new(width, height))
}
