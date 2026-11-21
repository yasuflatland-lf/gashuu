//! Browser timings for the in-memory CBZ spike.

use gashuu_core::{
    cache::CacheDispatch, CoreError, DecodedImage, PageEntry, PageSource, ZipSource,
};
use std::cell::{Cell, OnceCell};
use std::fmt::Display;
use std::sync::Arc;
use std::time::Duration;
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use web_time::Instant;

thread_local! {
    static BOOK: OnceCell<String> = const { OnceCell::new() };
    static PAGE_READ: Cell<Option<PageRead>> = const { Cell::new(None) };
    static FIRST_PAGE_STARTED: Cell<Option<Instant>> = const { Cell::new(None) };
    static TURN_STARTED: Cell<Option<Instant>> = const { Cell::new(None) };
    static TURN_TARGET: Cell<Option<(usize, Instant)>> = const { Cell::new(None) };
}

struct PageRead {
    duration: Duration,
    finished: Instant,
    succeeded: bool,
}

pub(crate) fn set_book(book: String) {
    BOOK.with(|current| current.set(book).expect("one book per browser load"));
}

fn log(name: &str, value: impl Display) {
    BOOK.with(|book| {
        let book = book.get().expect("book name set before logging");
        let line = format!("GASHUU_METRIC {name}={value} book={book}");
        web_sys::console::log_1(&JsValue::from_str(&line));
    });
}

pub(crate) fn log_duration(name: &str, duration: Duration) {
    log(name, format_args!("{:.3}", duration.as_secs_f64() * 1000.0));
}

pub(crate) fn source_opened(at: Instant) {
    FIRST_PAGE_STARTED.set(Some(at));
}

pub(crate) fn begin_turn(at: Instant) {
    TURN_STARTED.set(Some(at));
}

pub(crate) fn track_turn(epoch: usize) {
    if let Some(started) = TURN_STARTED.take() {
        TURN_TARGET.set(Some((epoch, started)));
    }
}

pub(crate) fn spread_applied(epoch: usize) {
    let applied = crate::clock::now();
    if let Some(started) = FIRST_PAGE_STARTED.take() {
        log_duration("first_page_ms", applied - started);
    }
    if let Some((target_epoch, started)) = TURN_TARGET.take() {
        if epoch == target_epoch {
            log_duration("turn_ms", applied - started);
        }
    }
}

/// Time the real ZIP read at the boundary used by the core cache.
pub(crate) struct TimedZipSource(pub(crate) ZipSource);

impl PageSource for TimedZipSource {
    fn list_pages(&self) -> Vec<PageEntry> {
        self.0.list_pages()
    }

    fn read_bytes(&self, index: usize) -> Result<Vec<u8>, CoreError> {
        let started = crate::clock::now();
        let result = self.0.read_bytes(index);
        let finished = crate::clock::now();
        PAGE_READ.set(Some(PageRead {
            duration: finished - started,
            finished,
            succeeded: result.is_ok(),
        }));
        result
    }

    fn skipped_count(&self) -> usize {
        self.0.skipped_count()
    }

    fn listing_truncated(&self) -> bool {
        self.0.listing_truncated()
    }
}

pub(crate) fn decode_page(
    dispatch: &CacheDispatch,
    index: usize,
) -> Result<Arc<DecodedImage>, CoreError> {
    PAGE_READ.set(None);
    let result = dispatch.decode_and_cache(index);
    let finished = crate::clock::now();
    if let Some(read) = PAGE_READ.take() {
        log_duration("read_ms", read.duration);
        if read.succeeded {
            // The public dispatch API combines decode with cache lookup and insertion.
            log_duration("decode_ms", finished - read.finished);
        }
    }
    result
}

pub(crate) fn install_heap_logger(window: &web_sys::Window) {
    let callback = Closure::<dyn FnMut()>::new(|| {
        let memory: js_sys::WebAssembly::Memory = wasm_bindgen::memory().unchecked_into();
        let buffer: js_sys::ArrayBuffer = memory.buffer().unchecked_into();
        log("wasm_heap_bytes", buffer.byte_length());
    });
    js_sys::Reflect::set(
        window.as_ref(),
        &JsValue::from_str("gashuuLogWasmHeap"),
        &callback.into_js_value(),
    )
    .expect("expose the wasm heap logger");
}
