use crate::ViewerWindow;
use js_sys::Uint8Array;
use slint::fontique_011::{fontique, shared_collection};
use std::{fmt::Display, sync::Arc};
use wasm_bindgen_futures::JsFuture;
use web_sys::{wasm_bindgen::JsCast, Response};

/// Relative to the document, so it works under any Trunk `public_url`.
pub(crate) const JA_FONT_URL: &str = "fonts/NotoSansJP-gashuu-subset.ttf";
/// Family the UI is switched to after registration: the embedded default.
const LATIN_FAMILY: &str = "Inter";

/// Schedule the font fetch, registration and re-shape on Slint's executor.
/// Boot can continue while the font loads.
pub(crate) fn load_japanese_fallback(ui: slint::Weak<ViewerWindow>) {
    let task = slint::spawn_local(async move {
        let result = fetch_bytes(JA_FONT_URL)
            .await
            .and_then(register_ja_fallback);
        match result {
            Ok(()) => {
                if let Some(ui) = ui.upgrade() {
                    ui.set_web_font_family(LATIN_FAMILY.into());
                }
            }
            Err(error) => warn_unavailable(error),
        }
    });
    if let Err(error) = task {
        warn_unavailable(error);
    }
}

async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or_else(|| "browser window unavailable".to_owned())?;
    let response = JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|error| format!("font fetch failed: {error:?}"))?
        .dyn_into::<Response>()
        .map_err(|_| "font fetch did not return a Response".to_owned())?;
    if !response.ok() {
        return Err(format!("font fetch returned HTTP {}", response.status()));
    }
    let body = response
        .array_buffer()
        .map_err(|error| format!("could not read font response: {error:?}"))?;
    let buffer = JsFuture::from(body)
        .await
        .map_err(|error| format!("could not read font response: {error:?}"))?;
    Ok(Uint8Array::new(&buffer).to_vec())
}

fn register_ja_fallback(bytes: Vec<u8>) -> Result<(), String> {
    let blob = fontique::Blob::new(Arc::new(bytes));
    let mut collection = shared_collection();
    let fonts = collection.register_fonts(blob, None);
    if fonts.is_empty() {
        return Err("font data did not register a family".to_owned());
    }
    for script in ["Hira", "Kana", "Hani"] {
        collection.append_fallbacks(
            fontique::FallbackKey::new(fontique::Script::from_str_unchecked(script), None),
            fonts.iter().map(|(family, _)| *family),
        );
    }
    Ok(())
}

fn warn_unavailable(error: impl Display) {
    tracing::warn!(error = %error, "japanese web font unavailable; CJK text renders as missing glyphs");
}
