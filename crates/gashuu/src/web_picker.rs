//! Browser file input for the Library's Add Books action.

use crate::{
    backend::Done,
    picker::{web::accept_filter, PickedBook},
};
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use web_sys::HtmlInputElement;

type PickDone = Rc<RefCell<Option<Done<Vec<PickedBook>>>>>;

pub(crate) struct Input {
    element: HtmlInputElement,
    done: PickDone,
    _change: Closure<dyn Fn()>,
    cancel: Closure<dyn Fn()>,
}

impl Input {
    pub(crate) fn new(
        on_change: fn(&HtmlInputElement) -> Vec<PickedBook>,
    ) -> Result<Self, JsValue> {
        let window = web_sys::window().ok_or("window unavailable")?;
        let document = window.document().ok_or("document unavailable")?;
        let body = document.body().ok_or("document body unavailable")?;
        let element = document
            .create_element("input")?
            .dyn_into::<HtmlInputElement>()?;
        let navigator = window.navigator();
        let agent = navigator.user_agent().unwrap_or_default();
        if let Some(accept) = accept_filter(&agent, navigator.max_touch_points().max(0) as u32) {
            element.set_accept(accept);
        }
        element.set_type("file");
        element.set_multiple(true);
        element.set_hidden(true);

        let done: PickDone = Rc::default();
        let change = Closure::<dyn Fn()>::new({
            let element = element.clone();
            let done = done.clone();
            move || {
                let callback = done.borrow_mut().take();
                if let Some(callback) = callback {
                    let picks = on_change(&element);
                    element.set_value("");
                    callback(picks);
                }
            }
        });
        let cancel = Closure::<dyn Fn()>::new({
            let done = done.clone();
            move || {
                let callback = done.borrow_mut().take();
                if let Some(callback) = callback {
                    callback(Vec::new());
                }
            }
        });
        element.add_event_listener_with_callback("cancel", cancel.as_ref().unchecked_ref())?;
        element.set_onchange(Some(change.as_ref().unchecked_ref()));
        // Safari requires the input to be attached to the document.
        body.append_child(&element)?;
        Ok(Self {
            element,
            done,
            _change: change,
            cancel,
        })
    }

    pub(crate) fn pick(&self, done: Done<Vec<PickedBook>>) {
        // Replace a pending callback if an older browser omitted the cancel event.
        *self.done.borrow_mut() = Some(done);
        self.element.click();
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        self.element.set_onchange(None);
        let _ = self
            .element
            .remove_event_listener_with_callback("cancel", self.cancel.as_ref().unchecked_ref());
        self.element.remove();
    }
}
