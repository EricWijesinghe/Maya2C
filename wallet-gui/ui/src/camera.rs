//! Camera capture for the QR scanner.
//!
//! This module only *captures* — it grabs a frame and hands the PNG bytes to
//! the Rust core, which decodes and parses them. A QR code is attacker-supplied
//! input, and decoding it here would put that payload inside the WASM sandbox
//! where the rest of the UI lives.
//!
//! ## What is not verified
//!
//! Camera capture needs a real camera and a user granting permission, so this
//! path is exercised by neither the test suite nor a headless build. The
//! decoding it feeds *is* tested, against real QR images, in the core crate.

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{HtmlCanvasElement, HtmlVideoElement, MediaStreamConstraints};

/// Starts the camera and attaches it to a `<video>` element.
///
/// # Errors
///
/// Returns a message if no camera is available or the user denies permission —
/// both of which are ordinary outcomes, not faults.
pub async fn start(video_id: &str) -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let navigator = window.navigator();
    let devices = navigator
        .media_devices()
        .map_err(|_| "this platform exposes no camera API".to_string())?;

    let constraints = MediaStreamConstraints::new();
    // Rear camera where there is a choice; on a phone the front one is not
    // where the user is pointing a payment code.
    let video = js_sys::Object::new();
    js_sys::Reflect::set(
        &video,
        &JsValue::from_str("facingMode"),
        &JsValue::from_str("environment"),
    )
    .map_err(|_| "could not build camera constraints".to_string())?;
    constraints.set_video(&video);
    constraints.set_audio(&JsValue::FALSE);

    let promise = devices
        .get_user_media_with_constraints(&constraints)
        .map_err(|_| "could not request the camera".to_string())?;

    let stream = JsFuture::from(promise)
        .await
        .map_err(|_| "camera permission was denied".to_string())?;

    let document = window.document().ok_or("no document")?;
    let element = document
        .get_element_by_id(video_id)
        .ok_or("the video element is missing")?;
    let video_element: HtmlVideoElement = element
        .dyn_into()
        .map_err(|_| "that element is not a <video>".to_string())?;

    video_element.set_src_object(Some(&stream.unchecked_into()));
    let _ = video_element.play();

    Ok(())
}

/// Stops the camera and releases the device.
///
/// Leaving a camera running behind a dismissed screen is both a privacy problem
/// and a visible one: the hardware indicator stays lit.
pub fn stop(video_id: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Some(element) = document.get_element_by_id(video_id) else {
        return;
    };
    let Ok(video) = element.dyn_into::<HtmlVideoElement>() else {
        return;
    };

    if let Some(object) = video.src_object() {
        let stream: web_sys::MediaStream = object.unchecked_into();
        let tracks = stream.get_tracks();
        for index in 0..tracks.length() {
            let track: web_sys::MediaStreamTrack = tracks.get(index).unchecked_into();
            track.stop();
        }
    }
    video.set_src_object(None);
}

/// Grabs the current frame as base64 PNG.
///
/// # Errors
///
/// Returns a message if the video is not yet producing frames, which is normal
/// for the first moments after the camera starts.
pub fn capture_frame(video_id: &str, canvas_id: &str) -> Result<String, String> {
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;

    let video: HtmlVideoElement = document
        .get_element_by_id(video_id)
        .ok_or("the video element is missing")?
        .dyn_into()
        .map_err(|_| "that element is not a <video>".to_string())?;

    let width = video.video_width();
    let height = video.video_height();
    if width == 0 || height == 0 {
        return Err("the camera has not produced a frame yet".to_string());
    }

    let canvas: HtmlCanvasElement = document
        .get_element_by_id(canvas_id)
        .ok_or("the canvas element is missing")?
        .dyn_into()
        .map_err(|_| "that element is not a <canvas>".to_string())?;

    canvas.set_width(width);
    canvas.set_height(height);

    let context = canvas
        .get_context("2d")
        .map_err(|_| "no 2d context".to_string())?
        .ok_or("no 2d context")?
        .dyn_into::<web_sys::CanvasRenderingContext2d>()
        .map_err(|_| "unexpected context type".to_string())?;

    context
        .draw_image_with_html_video_element(&video, 0.0, 0.0)
        .map_err(|_| "could not read the frame".to_string())?;

    let data_url = canvas
        .to_data_url_with_type("image/png")
        .map_err(|_| "could not encode the frame".to_string())?;

    // Strip the `data:image/png;base64,` prefix; the core wants raw base64.
    data_url
        .split_once(',')
        .map(|(_, payload)| payload.to_string())
        .ok_or_else(|| "unexpected data URL".to_string())
}
