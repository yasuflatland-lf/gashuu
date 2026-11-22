use crate::storage_status::StorageStatus;
use js_sys::Reflect;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

pub(crate) async fn read_status() -> StorageStatus {
    let window = web_sys::window().expect("browser window");
    let navigator = window.navigator();
    let storage = navigator.storage();
    let persisted = match storage.persisted() {
        Ok(p) => JsFuture::from(p).await.ok().and_then(|v| v.as_bool()),
        Err(_) => None,
    };
    let estimate = match storage.estimate() {
        Ok(p) => JsFuture::from(p).await.ok(),
        Err(_) => None,
    };
    let number = |key: &str| {
        estimate
            .as_ref()
            .and_then(|v| Reflect::get(v, &JsValue::from_str(key)).ok())
            .and_then(|v| v.as_f64())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .map(|v| v as u64)
    };
    let standalone = window
        .match_media("(display-mode: standalone)")
        .ok()
        .flatten()
        .is_some_and(|m| m.matches())
        || Reflect::get(&navigator, &"standalone".into())
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
    StorageStatus {
        persisted,
        usage: number("usage"),
        quota: number("quota"),
        standalone,
        ios_family: crate::web_ios::is_ios_family(
            &navigator.user_agent().unwrap_or_default(),
            navigator.max_touch_points() as u32,
        ),
    }
}
