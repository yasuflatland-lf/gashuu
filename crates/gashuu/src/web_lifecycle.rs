use std::rc::Rc;
use wasm_bindgen::{closure::Closure, JsCast};
use web_sys::VisibilityState;

/// Register page-hide and hidden-visibility flushes for the app lifetime.
pub(crate) fn wire_suspend_persistence(flush: Rc<dyn Fn()>) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };

    // Browser events run on an empty wasm stack, after Slint callbacks return;
    // therefore no callback's RefCell borrow remains live during a flush.
    let on_pagehide = Closure::<dyn FnMut()>::new({
        let flush = Rc::clone(&flush);
        move || flush()
    });
    listen(&window, "pagehide", on_pagehide);

    let on_visibility = Closure::<dyn FnMut()>::new({
        let document = document.clone();
        move || {
            if document.visibility_state() == VisibilityState::Hidden {
                flush();
            }
        }
    });
    listen(&document, "visibilitychange", on_visibility);
}

fn listen(target: &web_sys::EventTarget, event: &str, handler: Closure<dyn FnMut()>) {
    match target.add_event_listener_with_callback(event, handler.as_ref().unchecked_ref()) {
        Ok(()) => handler.forget(),
        Err(error) => tracing::error!(?error, event, "failed to register listener"),
    }
}
