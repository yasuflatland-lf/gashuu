//! Browser file input for the Library's Add Books action.

use crate::{add_controller::AddController, i18n, web_books, web_ios, ViewerWindow};
use gashuu_core::Settings;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{File, HtmlInputElement};

struct Picker {
    input: HtmlInputElement,
    _on_change: Closure<dyn FnMut()>,
}

impl Picker {
    fn new(on_files: impl Fn(Vec<File>) + 'static) -> Result<Self, JsValue> {
        let document = web_sys::window()
            .and_then(|window| window.document())
            .ok_or_else(|| JsValue::from_str("document unavailable"))?;
        let body = document
            .body()
            .ok_or_else(|| JsValue::from_str("document body unavailable"))?;
        let input = document
            .create_element("input")?
            .dyn_into::<HtmlInputElement>()?;
        input.set_type("file");
        let navigator = web_sys::window().map(|window| window.navigator());
        let user_agent = navigator
            .as_ref()
            .and_then(|navigator| navigator.user_agent().ok())
            .unwrap_or_default();
        let max_touch_points = navigator
            .as_ref()
            .map_or(0, |navigator| navigator.max_touch_points().max(0) as u32);
        // The iOS picker greys out CBZ files when an accept filter is set.
        if !web_ios::is_ios_family(&user_agent, max_touch_points) {
            input.set_accept(".cbz,.zip");
        }
        input.set_multiple(true);
        input.set_hidden(true);
        body.append_child(&input)?;

        let change_input = input.clone();
        let on_change = Closure::<dyn FnMut()>::new(move || {
            let Some(files) = change_input.files() else {
                return;
            };
            let files = (0..files.length())
                .filter_map(|index| files.get(index))
                .collect();
            // Selecting the same file again must also fire a change event.
            change_input.set_value("");
            on_files(files);
        });
        input.set_onchange(Some(on_change.as_ref().unchecked_ref()));
        Ok(Self {
            input,
            _on_change: on_change,
        })
    }

    fn click(&self) {
        self.input.click();
    }
}

impl Drop for Picker {
    fn drop(&mut self) {
        self.input.set_onchange(None);
        self.input.remove();
    }
}

pub(crate) fn wire(
    ui: &ViewerWindow,
    settings: &Rc<RefCell<Settings>>,
    adder: &Rc<AddController>,
    localizer: &Rc<i18n::Localizer>,
) {
    let picker = Picker::new({
        let weak = ui.as_weak();
        let settings = Rc::clone(settings);
        let adder = Rc::clone(adder);
        let localizer = Rc::clone(localizer);
        move |files| {
            spawn_local(read_files(
                files,
                weak.clone(),
                Rc::clone(&settings),
                Rc::clone(&adder),
                Rc::clone(&localizer),
            ));
        }
    });
    let weak = ui.as_weak();
    let localizer = Rc::clone(localizer);
    ui.on_add_books(move || match &picker {
        // The browser requires the click to run during the user gesture.
        Ok(picker) => picker.click(),
        Err(error) => {
            tracing::warn!(?error, "could not create the browser file picker");
            if let Some(ui) = weak.upgrade() {
                ui.set_add_toast_text(
                    i18n::dynamic::web_picker_unavailable(localizer.loader()).into(),
                );
            }
        }
    });
}

async fn read_files(
    files: Vec<File>,
    weak: slint::Weak<ViewerWindow>,
    settings: Rc<RefCell<Settings>>,
    adder: Rc<AddController>,
    localizer: Rc<i18n::Localizer>,
) {
    let mut paths = Vec::with_capacity(files.len());
    let mut unsupported = 0;
    let mut unreadable = 0;
    for file in files {
        let name = file.name();
        if !web_books::supported_name(&name) {
            unsupported += 1;
            continue;
        }
        match JsFuture::from(file.array_buffer()).await {
            Ok(buffer) => {
                let bytes = Arc::from(js_sys::Uint8Array::new(&buffer).to_vec());
                paths.push(web_books::insert(&name, bytes));
            }
            Err(error) => {
                tracing::warn!(%name, ?error, "could not read selected file");
                unreadable += 1;
            }
        }
    }
    let mut notices = Vec::new();
    if unsupported > 0 {
        notices.push(i18n::dynamic::web_unsupported_files(
            localizer.loader(),
            unsupported,
        ));
    }
    if unreadable > 0 {
        notices.push(i18n::dynamic::web_unreadable_files(
            localizer.loader(),
            unreadable,
        ));
    }
    let notice = (!notices.is_empty()).then(|| notices.join("\n"));
    if paths.is_empty() {
        if let (Some(ui), Some(notice)) = (weak.upgrade(), notice) {
            ui.set_status_text("".into());
            ui.set_add_toast_text(notice.into());
            ui.invoke_focus_carousel();
        }
        return;
    }
    let policy = settings.borrow().archive_policy();
    adder.start_from_picker(weak, paths, policy, notice);
}
