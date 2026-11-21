//! Dispatch loader work on a native worker or inline in the browser.

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use rayon::spawn;

#[cfg(target_arch = "wasm32")]
pub(crate) fn spawn(job: impl FnOnce() + Send + 'static) {
    job();
}
