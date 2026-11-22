//! Browser-native page decoding and canvas readback.

#![cfg(target_arch = "wasm32")]

use gashuu_core::{check_pixel_limit, CoreError, DecodeFuture, DecodedImage, PageDecoder};
use js_sys::{Array, Object, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Blob, CanvasRenderingContext2d, ColorSpaceConversion, HtmlCanvasElement, ImageBitmap,
    ImageBitmapOptions, ImageData, OffscreenCanvas, OffscreenCanvasRenderingContext2d,
    PremultiplyAlpha, Window, WorkerGlobalScope,
};

pub struct NativePageDecoder;

impl PageDecoder for NativePageDecoder {
    fn decode(&self, bytes: Vec<u8>) -> DecodeFuture {
        Box::pin(async move {
            let global = js_sys::global();
            let bitmap = start_decode(&global, bytes)
                .map_err(decode_error)?
                .await
                .map_err(decode_error)?
                .dyn_into::<ImageBitmap>()
                .map_err(|_| decode_error("createImageBitmap returned no bitmap".into()))?;
            let result = read_bitmap(&global, &bitmap);
            bitmap.close();
            result
        })
    }
}

fn start_decode(global: &Object, bytes: Vec<u8>) -> Result<JsFuture, JsValue> {
    let parts = Array::of1(&Uint8Array::from(bytes.as_slice()));
    let blob = Blob::new_with_u8_array_sequence(&parts)?;
    let options = bitmap_options()?;
    let promise = if let Some(window) = global.dyn_ref::<Window>() {
        window.create_image_bitmap_with_blob_and_image_bitmap_options(&blob, &options)?
    } else if let Some(worker) = global.dyn_ref::<WorkerGlobalScope>() {
        worker.create_image_bitmap_with_blob_and_image_bitmap_options(&blob, &options)?
    } else {
        return Err("unsupported global scope for image decoding".into());
    };
    Ok(JsFuture::from(promise))
}

fn bitmap_options() -> Result<ImageBitmapOptions, JsValue> {
    let options = ImageBitmapOptions::new();
    options.set_premultiply_alpha(PremultiplyAlpha::None);
    options.set_color_space_conversion(ColorSpaceConversion::None);
    // web-sys 0.3.107 omits "none" from its ImageOrientation enum.
    Reflect::set(&options, &"imageOrientation".into(), &"none".into())?;
    Ok(options)
}

fn read_bitmap(global: &Object, bitmap: &ImageBitmap) -> Result<DecodedImage, CoreError> {
    let (width, height) = (bitmap.width(), bitmap.height());
    check_pixel_limit(width, height)?;
    let pixels = canvas_pixels(global, bitmap, width, height).map_err(decode_error)?;
    DecodedImage::new(pixels.data().to_vec(), width, height)
}

fn canvas_pixels(
    global: &Object,
    bitmap: &ImageBitmap,
    width: u32,
    height: u32,
) -> Result<ImageData, JsValue> {
    let options = Object::new();
    Reflect::set(&options, &"willReadFrequently".into(), &JsValue::TRUE)?;
    if Reflect::has(global, &"OffscreenCanvas".into())? {
        let canvas = OffscreenCanvas::new(width, height)?;
        let context: OffscreenCanvasRenderingContext2d =
            context_2d(canvas.get_context_with_context_options("2d", &options)?)?;
        context.draw_image_with_image_bitmap(bitmap, 0.0, 0.0)?;
        context.get_image_data(0.0, 0.0, width.into(), height.into())
    } else {
        let document = global
            .dyn_ref::<Window>()
            .and_then(Window::document)
            .ok_or("document unavailable")?;
        let canvas = document
            .create_element("canvas")?
            .dyn_into::<HtmlCanvasElement>()
            .map_err(|_| "could not create HTML canvas")?;
        canvas.set_width(width);
        canvas.set_height(height);
        let context: CanvasRenderingContext2d =
            context_2d(canvas.get_context_with_context_options("2d", &options)?)?;
        context.draw_image_with_image_bitmap(bitmap, 0.0, 0.0)?;
        context.get_image_data(0.0, 0.0, width.into(), height.into())
    }
}

fn context_2d<T: JsCast>(context: Option<Object>) -> Result<T, JsValue> {
    context
        .and_then(|object| object.dyn_into().ok())
        .ok_or_else(|| "2D canvas context unavailable".into())
}

fn decode_error(error: JsValue) -> CoreError {
    let message = error
        .as_string()
        .or_else(|| Reflect::get(&error, &"message".into()).ok()?.as_string())
        .unwrap_or_else(|| format!("browser decode failed: {error:?}"));
    CoreError::Decode(std::io::Error::other(message).into())
}
