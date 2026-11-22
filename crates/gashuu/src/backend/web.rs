use super::web_client::{Delivery, Effects, Outgoing, Ready, WebClient};
use super::{
    BackendError, BookBackend, CountResult, Done, ImageResult, OpenedBook, SessionId, SpreadPages,
};
use gashuu_core::{ArchivePolicy, BookId, CacheConfig, DecodedImage};
use js_sys::{Array, Object, Reflect, Uint8Array};
use std::cell::{OnceCell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use web_sys::{ErrorEvent, MessageEvent, Url, Worker, WorkerOptions};

const LOADER: &str = "gashuu-worker_loader.js";

pub(super) struct WorkerBackend {
    client: RefCell<WebClient>,
    worker: Worker,
    _onmessage: Closure<dyn Fn(MessageEvent)>,
    _onerror: Closure<dyn Fn(ErrorEvent)>,
}

impl WorkerBackend {
    fn new() -> Result<Rc<Self>, String> {
        let document = web_sys::window()
            .and_then(|window| window.document())
            .ok_or("document unavailable")?;
        let base = document
            .base_uri()
            .map_err(js_error)?
            .ok_or("document base URI unavailable")?;
        let url = Url::new_with_base(LOADER, &base).map_err(js_error)?;
        // The Trunk loader uses importScripts and must run as a classic Worker.
        let worker =
            Worker::new_with_options(&url.href(), &WorkerOptions::new()).map_err(js_error)?;
        Ok(Rc::new_cyclic(|backend: &Weak<Self>| {
            let weak = backend.clone();
            let onmessage = Closure::new(move |event: MessageEvent| {
                if let Some(backend) = weak.upgrade() {
                    match read_envelope(&event.data()) {
                        Ok((text, images)) => {
                            backend.dispatch(|client| client.on_message(&text, images))
                        }
                        Err(detail) => backend.failed(&detail),
                    }
                }
            });
            let weak = backend.clone();
            let onerror = Closure::new(move |event: ErrorEvent| {
                if let Some(backend) = weak.upgrade() {
                    event.prevent_default();
                    backend.failed(&event.message());
                }
            });
            worker.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
            worker.set_onerror(Some(onerror.as_ref().unchecked_ref()));
            Self {
                client: RefCell::new(WebClient::new()),
                worker,
                _onmessage: onmessage,
                _onerror: onerror,
            }
        }))
    }

    fn dispatch(&self, f: impl FnOnce(&mut WebClient) -> Effects) {
        let (outgoing, deliveries) = f(&mut self.client.borrow_mut());
        for Outgoing(text) in outgoing {
            let envelope = Object::new();
            // A fresh object accepts the field and carries no pixel buffers on requests.
            Reflect::set(&envelope, &"m".into(), &text.into()).expect("set worker metadata");
            if let Err(error) = self.worker.post_message(&envelope) {
                self.failed(&js_error(error));
                break;
            }
        }
        self.deliver(deliveries);
    }

    fn failed(&self, detail: &str) {
        self.worker.terminate();
        let deliveries = self.client.borrow_mut().on_worker_failed(detail);
        self.deliver(deliveries);
    }

    fn deliver(&self, deliveries: Vec<Delivery>) {
        for delivery in deliveries {
            match delivery {
                Delivery::Now(ready) => self.run(ready),
                Delivery::Deferred(ready) => {
                    if let Err(error) = slint::invoke_from_event_loop(move || {
                        if let Some(backend) = shared() {
                            backend.run(ready);
                        }
                    }) {
                        tracing::debug!(%error, "dropped backend result; event loop gone");
                    }
                }
            }
        }
    }

    fn run(&self, ready: Ready) {
        let callback = ready.into_callback(&self.client.borrow());
        if let Some(callback) = callback {
            callback();
        }
    }
}

impl Drop for WorkerBackend {
    fn drop(&mut self) {
        self.worker.set_onmessage(None);
        self.worker.set_onerror(None);
        self.worker.terminate();
    }
}

fn read_envelope(envelope: &JsValue) -> Result<(String, Vec<Vec<u8>>), String> {
    let text = Reflect::get(envelope, &"m".into())
        .map_err(js_error)?
        .as_string()
        .ok_or("worker envelope is missing metadata")?;
    let buffers = Reflect::get(envelope, &"b".into()).map_err(js_error)?;
    let images = if buffers.is_undefined() {
        Vec::new()
    } else {
        let buffers: Array = buffers.dyn_into().map_err(js_error)?;
        buffers
            .iter()
            .map(|value| {
                // Keep slot alignment for invalid buffers so unpack reports a per-slot error.
                value
                    .dyn_into::<Uint8Array>()
                    .map_or_else(|_| Vec::new(), |bytes| bytes.to_vec())
            })
            .collect()
    };
    Ok((text, images))
}

fn js_error(error: JsValue) -> String {
    error.as_string().unwrap_or_else(|| format!("{error:?}"))
}

pub(super) fn shared() -> Option<Rc<WorkerBackend>> {
    thread_local! {
        static BACKEND: OnceCell<Option<Rc<WorkerBackend>>> = const { OnceCell::new() };
    }
    BACKEND.with(|backend| {
        backend
            .get_or_init(|| {
                WorkerBackend::new()
                    .map_err(|detail| {
                        tracing::error!(%detail, "could not start web worker");
                    })
                    .ok()
            })
            .clone()
    })
}

impl BookBackend for WorkerBackend {
    fn open(
        &self,
        id: &BookId,
        policy: ArchivePolicy,
        cache: CacheConfig,
        done: Done<Result<OpenedBook, BackendError>>,
    ) {
        self.dispatch(|client| client.open(id, policy, cache, done));
    }

    fn close(&self, session: SessionId) {
        self.dispatch(|client| (client.close(session), Vec::new()));
    }

    fn cached_page(&self, session: SessionId, index: usize) -> Option<Arc<DecodedImage>> {
        self.client.borrow_mut().cached_page(session, index)
    }

    fn decode_spread(
        &self,
        session: SessionId,
        leading: usize,
        trailing: Option<usize>,
        done: Done<SpreadPages>,
    ) {
        self.dispatch(|client| client.decode_spread(session, leading, trailing, done));
    }

    fn thumbnail(&self, session: SessionId, index: usize, max_side: u32, done: Done<ImageResult>) {
        self.dispatch(|client| client.thumbnail(session, index, max_side, done));
    }

    fn cover(
        &self,
        id: &BookId,
        max_side: u32,
        on_cover: Done<ImageResult>,
        on_count: Option<Done<CountResult>>,
    ) {
        self.dispatch(|client| client.cover(id, max_side, on_cover, on_count));
    }

    fn cancel_covers(&self) {
        self.dispatch(|client| (client.cancel_covers(), Vec::new()));
    }

    fn probe_page_count(&self, id: &BookId, policy: ArchivePolicy, done: Done<CountResult>) {
        self.dispatch(|client| client.probe_page_count(id, policy, done));
    }
}
