//! [`egui`] bindings for web apps (compiling to WASM).

#![allow(clippy::missing_errors_doc)] // So many `-> Result<_, JsValue>`

mod app_runner;
mod backend;
mod events;
mod input;
mod panic_handler;
mod text_agent;
mod web_logger;
mod web_runner;

/// Access to the browser screen reader.
pub mod screen_reader;

/// Access to local browser storage.
pub mod storage;

pub(crate) use app_runner::AppRunner;
pub use panic_handler::{PanicHandler, PanicSummary};
pub use web_logger::WebLogger;
pub use web_runner::WebRunner;

#[cfg(not(any(feature = "glow", feature = "wgpu")))]
compile_error!("You must enable either the 'glow' or 'wgpu' feature");

mod web_painter;

#[cfg(feature = "glow")]
mod web_painter_glow;
#[cfg(feature = "glow")]
pub(crate) type ActiveWebPainter = web_painter_glow::WebPainterGlow;

#[cfg(feature = "wgpu")]
mod web_painter_wgpu;
#[cfg(all(feature = "wgpu", not(feature = "glow")))]
pub(crate) type ActiveWebPainter = web_painter_wgpu::WebPainterWgpu;

pub use backend::*;

use egui::Vec2;
use wasm_bindgen::prelude::*;
use web_sys::MediaQueryList;

use input::*;

use crate::Theme;

// ----------------------------------------------------------------------------

pub(crate) fn string_from_js_value(value: &JsValue) -> String {
    value.as_string().unwrap_or_else(|| format!("{value:#?}"))
}

/// Current time in seconds (since undefined point in time).
///
/// Monotonically increasing.
pub fn now_sec() -> f64 {
    web_sys::window()
        .expect("should have a Window")
        .performance()
        .expect("should have a Performance")
        .now()
        / 1000.0
}

/// The native GUI scale factor, taking into account the browser zoom.
///
/// Corresponds to [`window.devicePixelRatio`](https://developer.mozilla.org/en-US/docs/Web/API/Window/devicePixelRatio) in JavaScript.
pub fn native_pixels_per_point() -> f32 {
    let pixels_per_point = web_sys::window().unwrap().device_pixel_ratio() as f32;
    if pixels_per_point > 0.0 && pixels_per_point.is_finite() {
        pixels_per_point
    } else {
        1.0
    }
}

/// Ask the browser about the preferred system theme.
///
/// `None` means unknown.
pub fn system_theme() -> Option<Theme> {
    let dark_mode = prefers_color_scheme_dark(&web_sys::window()?)
        .ok()??
        .matches();
    Some(theme_from_dark_mode(dark_mode))
}

fn prefers_color_scheme_dark(window: &web_sys::Window) -> Result<Option<MediaQueryList>, JsValue> {
    window.match_media("(prefers-color-scheme: dark)")
}

fn theme_from_dark_mode(dark_mode: bool) -> Theme {
    if dark_mode {
        Theme::Dark
    } else {
        Theme::Light
    }
}

fn canvas_element(canvas_id: &str) -> Option<web_sys::HtmlCanvasElement> {
    let document = web_sys::window()?.document()?;
    let canvas = document.get_element_by_id(canvas_id)?;
    canvas.dyn_into::<web_sys::HtmlCanvasElement>().ok()
}

fn canvas_element_or_die(canvas_id: &str) -> web_sys::HtmlCanvasElement {
    canvas_element(canvas_id)
        .unwrap_or_else(|| panic!("Failed to find canvas with id {canvas_id:?}"))
}

fn canvas_origin(canvas_id: &str) -> egui::Pos2 {
    let rect = canvas_element(canvas_id)
        .unwrap()
        .get_bounding_client_rect();
    egui::pos2(rect.left() as f32, rect.top() as f32)
}

fn canvas_size_in_points(canvas_id: &str) -> egui::Vec2 {
    let canvas = canvas_element(canvas_id).unwrap();
    let pixels_per_point = native_pixels_per_point();
    egui::vec2(
        canvas.width() as f32 / pixels_per_point,
        canvas.height() as f32 / pixels_per_point,
    )
}

fn resize_canvas_to_screen_size(canvas_id: &str, max_size_points: egui::Vec2) -> Option<()> {
    let canvas = canvas_element(canvas_id)?;
    let parent = canvas.parent_element()?;

    // Prefer the client width and height so that if the parent
    // element is resized that the egui canvas resizes appropriately.
    let width = parent.client_width();
    let height = parent.client_height();

    let canvas_real_size = Vec2 {
        x: width as f32,
        y: height as f32,
    };

    if width <= 0 || height <= 0 {
        log::error!("egui canvas parent size is {}x{}. Try adding `html, body {{ height: 100%; width: 100% }}` to your CSS!", width, height);
    }

    let pixels_per_point = native_pixels_per_point();

    let max_size_pixels = pixels_per_point * max_size_points;

    let canvas_size_pixels = pixels_per_point * canvas_real_size;
    let canvas_size_pixels = canvas_size_pixels.min(max_size_pixels);
    let canvas_size_points = canvas_size_pixels / pixels_per_point;

    // Make sure that the height and width are always even numbers.
    // otherwise, the page renders blurry on some platforms.
    // See https://github.com/emilk/egui/issues/103
    fn round_to_even(v: f32) -> f32 {
        (v / 2.0).round() * 2.0
    }

    canvas
        .style()
        .set_property(
            "width",
            &format!("{}px", round_to_even(canvas_size_points.x)),
        )
        .ok()?;
    canvas
        .style()
        .set_property(
            "height",
            &format!("{}px", round_to_even(canvas_size_points.y)),
        )
        .ok()?;
    canvas.set_width(round_to_even(canvas_size_pixels.x) as u32);
    canvas.set_height(round_to_even(canvas_size_pixels.y) as u32);

    Some(())
}

// ----------------------------------------------------------------------------

/// Set the cursor icon.
fn set_cursor_icon(cursor: egui::CursorIcon) -> Option<()> {
    let document = web_sys::window()?.document()?;
    document
        .body()?
        .style()
        .set_property("cursor", cursor_web_name(cursor))
        .ok()
}

/// Set the clipboard text.
#[cfg(web_sys_unstable_apis)]
fn set_clipboard_text(s: &str) {
    if let Some(window) = web_sys::window() {
        if let Some(clipboard) = window.navigator().clipboard() {
            let promise = clipboard.write_text(s);
            let future = wasm_bindgen_futures::JsFuture::from(promise);
            let future = async move {
                if let Err(err) = future.await {
                    log::error!("Copy/cut action failed: {}", string_from_js_value(&err));
                }
            };
            wasm_bindgen_futures::spawn_local(future);
        }
    }
}

fn cursor_web_name(cursor: egui::CursorIcon) -> &'static str {
    match cursor {
        egui::CursorIcon::Alias => "alias",
        egui::CursorIcon::AllScroll => "all-scroll",
        egui::CursorIcon::Cell => "cell",
        egui::CursorIcon::ContextMenu => "context-menu",
        egui::CursorIcon::Copy => "copy",
        egui::CursorIcon::Crosshair => "crosshair",
        egui::CursorIcon::Default => "default",
        egui::CursorIcon::Grab => "grab",
        egui::CursorIcon::Grabbing => "grabbing",
        egui::CursorIcon::Help => "help",
        egui::CursorIcon::Move => "move",
        egui::CursorIcon::NoDrop => "no-drop",
        egui::CursorIcon::None => "none",
        egui::CursorIcon::NotAllowed => "not-allowed",
        egui::CursorIcon::PointingHand => "pointer",
        egui::CursorIcon::Progress => "progress",
        egui::CursorIcon::ResizeHorizontal => "ew-resize",
        egui::CursorIcon::ResizeNeSw => "nesw-resize",
        egui::CursorIcon::ResizeNwSe => "nwse-resize",
        egui::CursorIcon::ResizeVertical => "ns-resize",

        egui::CursorIcon::ResizeEast => "e-resize",
        egui::CursorIcon::ResizeSouthEast => "se-resize",
        egui::CursorIcon::ResizeSouth => "s-resize",
        egui::CursorIcon::ResizeSouthWest => "sw-resize",
        egui::CursorIcon::ResizeWest => "w-resize",
        egui::CursorIcon::ResizeNorthWest => "nw-resize",
        egui::CursorIcon::ResizeNorth => "n-resize",
        egui::CursorIcon::ResizeNorthEast => "ne-resize",
        egui::CursorIcon::ResizeColumn => "col-resize",
        egui::CursorIcon::ResizeRow => "row-resize",

        egui::CursorIcon::Text => "text",
        egui::CursorIcon::VerticalText => "vertical-text",
        egui::CursorIcon::Wait => "wait",
        egui::CursorIcon::ZoomIn => "zoom-in",
        egui::CursorIcon::ZoomOut => "zoom-out",
    }
}

/// Open the given url in the browser.
pub fn open_url(url: &str, new_tab: bool) -> Option<()> {
    let name = if new_tab { "_blank" } else { "_self" };

    web_sys::window()?
        .open_with_url_and_target(url, name)
        .ok()?;
    Some(())
}

/// e.g. "#fragment" part of "www.example.com/index.html#fragment",
///
/// Percent decoded
pub fn location_hash() -> String {
    percent_decode(
        &web_sys::window()
            .unwrap()
            .location()
            .hash()
            .unwrap_or_default(),
    )
}

/// Percent-decodes a string.
pub fn percent_decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .to_string()
}
