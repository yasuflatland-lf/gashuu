use std::path::Path;

pub(crate) fn accept_filter(user_agent: &str, max_touch_points: u32) -> Option<&'static str> {
    // An accept filter greys out CBZ files in the iOS picker.
    (!crate::web_ios::is_ios_family(user_agent, max_touch_points)).then_some(".cbz,.zip")
}

pub(super) fn supported_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cbz") || ext.eq_ignore_ascii_case("zip"))
}

#[cfg(target_arch = "wasm32")]
pub(super) use browser::WebPicker;
#[cfg(target_arch = "wasm32")]
pub(crate) use browser::{forget_files, has_notice, take_file, take_notice};

#[cfg(target_arch = "wasm32")]
mod browser {
    use super::supported_name;
    use crate::{
        backend::Done,
        i18n,
        picker::{BookPicker, PickedBook, UploadHandle},
        web_picker::Input,
    };
    use std::{
        cell::{Cell, RefCell},
        collections::HashMap,
    };
    use wasm_bindgen::JsValue;
    use wasm_bindgen_futures::{spawn_local, JsFuture};
    use web_sys::{File, HtmlInputElement};

    thread_local! {
        static FILES: RefCell<HashMap<u32, File>> = RefCell::default();
        static NEXT_KEY: Cell<u32> = const { Cell::new(0) };
        static NOTICE: Cell<Option<Notice>> = const { Cell::new(None) };
        static PERSIST_REQUESTED: Cell<bool> = const { Cell::new(false) };
    }

    #[derive(Clone, Copy)]
    enum Notice {
        Unavailable,
        Skipped {
            unsupported: usize,
            unreadable: usize,
        },
    }

    pub(crate) fn has_notice() -> bool {
        NOTICE.with(|notice| notice.get().is_some())
    }

    pub(crate) fn take_notice(localizer: &i18n::Localizer) -> Option<String> {
        match NOTICE.with(Cell::take)? {
            Notice::Unavailable => Some(i18n::dynamic::web_picker_unavailable(localizer.loader())),
            Notice::Skipped {
                unsupported,
                unreadable,
            } => {
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
                Some(notices.join("\n"))
            }
        }
    }

    pub(crate) fn take_file(key: u32) -> Option<File> {
        FILES.with_borrow_mut(|files| files.remove(&key))
    }

    pub(crate) fn forget_files(keys: &[u32]) {
        FILES.with_borrow_mut(|files| {
            for key in keys {
                files.remove(key);
            }
        });
    }

    pub(crate) struct WebPicker(Result<Input, JsValue>);

    impl WebPicker {
        pub(crate) fn new() -> Self {
            Self(Input::new(collect_files))
        }
    }

    impl BookPicker for WebPicker {
        fn pick_books(&self, done: Done<Vec<PickedBook>>) {
            request_persistence();
            NOTICE.with(|notice| notice.set(None));
            match &self.0 {
                Ok(input) => input.pick(done),
                Err(error) => {
                    tracing::warn!(?error, "could not create the browser file picker");
                    spawn_local(async {
                        NOTICE.with(|notice| notice.set(Some(Notice::Unavailable)));
                        done(Vec::new());
                    });
                }
            }
        }

        fn supports_folders(&self) -> bool {
            false
        }

        fn pick_folder(&self, done: Done<Option<PickedBook>>) {
            spawn_local(async { done(None) });
        }
    }

    fn collect_files(input: &HtmlInputElement) -> Vec<PickedBook> {
        let mut picks = Vec::new();
        let mut unsupported = 0;
        let mut unreadable = 0;
        if let Some(files) = input.files() {
            for index in 0..files.length() {
                let Some(file) = files.get(index) else {
                    unreadable += 1;
                    continue;
                };
                let name = file.name();
                if !supported_name(&name) {
                    unsupported += 1;
                    continue;
                }
                let key = NEXT_KEY.with(|next| {
                    let key = next.get();
                    next.set(key.checked_add(1).expect("upload keys exhausted"));
                    key
                });
                let size = file.size() as u64;
                FILES.with_borrow_mut(|files| files.insert(key, file));
                picks.push(PickedBook::Upload(UploadHandle { key, name, size }));
            }
        } else {
            unreadable += 1;
        }
        if unsupported > 0 || unreadable > 0 {
            NOTICE.with(|notice| {
                notice.set(Some(Notice::Skipped {
                    unsupported,
                    unreadable,
                }))
            });
        }
        picks
    }

    fn request_persistence() {
        if PERSIST_REQUESTED.with(|requested| requested.replace(true)) {
            return;
        }
        let Some(window) = web_sys::window() else {
            return;
        };
        let storage = window.navigator().storage();
        // Start the request in the click task, before opening the native picker.
        let request = storage.persist();
        spawn_local(async move {
            let result = match request {
                Ok(promise) => JsFuture::from(promise).await,
                Err(error) => Err(error),
            };
            match result {
                Ok(value) if value.as_bool() == Some(true) => {
                    tracing::info!("browser storage persistence granted")
                }
                Ok(_) => tracing::warn!("browser storage persistence denied"),
                Err(error) => tracing::warn!(?error, "browser storage persistence unavailable"),
            }
            if let Ok(persisted) = storage.persisted() {
                if let Ok(value) = JsFuture::from(persisted).await {
                    tracing::info!(
                        persisted = value.as_bool().unwrap_or(false),
                        "browser storage state"
                    );
                }
            }
            if let Ok(estimate) = storage.estimate() {
                if let Ok(estimate) = JsFuture::from(estimate).await {
                    tracing::info!(?estimate, "browser storage estimate");
                }
            }
        });
    }
}
