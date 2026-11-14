//! What wry doesn't offer of a WKWebView: a picture of its page (asked of
//! WebKit, delivered later on the main thread), making it see-through, and
//! whether it has the keyboard.
use block2::RcBlock;
use objc2_app_kit::{NSImage, NSView};
use objc2_foundation::NSError;
use std::{
    ffi::c_void,
    sync::{Arc, Mutex},
};
use wry::WebViewExtMacOS;

/// A picture of the page: RGBA, premultiplied, top row first.
pub struct Shot {
    pub pixels: Vec<u8>,
    pub size: [usize; 2],
}

/// Where a snapshot arrives: `Some(None)` is one that couldn't be taken.
pub type Shots = Arc<Mutex<Option<Option<Shot>>>>;

#[repr(C)]
struct CGRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
    fn CGColorSpaceRelease(space: *mut c_void);
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: *mut c_void,
        bitmap_info: u32,
    ) -> *mut c_void;
    fn CGContextDrawImage(context: *mut c_void, rect: CGRect, image: *const c_void);
    fn CGContextRelease(context: *mut c_void);
    fn CGImageGetWidth(image: *const c_void) -> usize;
    fn CGImageGetHeight(image: *const c_void) -> usize;
}

/// `kCGImageAlphaPremultipliedLast`: RGBA, as egui takes it.
const RGBA_PREMULTIPLIED: u32 = 1;

/// Whether the web view (or something inside it) is the one the window
/// sends the keyboard to: it becomes so when it's clicked.
pub fn has_keyboard(webview: &wry::WebView) -> bool {
    let view = webview.webview();
    let Some(window) = view.window() else {
        return false;
    };
    let Some(responder) = window.firstResponder() else {
        return false;
    };
    responder
        .downcast_ref::<NSView>()
        .is_some_and(|responder| responder.isDescendantOf(&view))
}

/// Make the web view see-through, or not. See-through, it still takes the
/// mouse and keyboard (a hidden view doesn't) and its page keeps running.
pub fn set_transparent(webview: &wry::WebView, transparent: bool) {
    webview
        .webview()
        .setAlphaValue(if transparent { 0.0 } else { 1.0 });
}

/// Ask for a snapshot of what `webview` shows: it lands in `shots`, and
/// `arrived` is called (the app may be asleep until then).
pub fn take_snapshot(webview: &wry::WebView, shots: Shots, arrived: impl Fn() + 'static) {
    let done = RcBlock::new(move |image: *mut NSImage, _error: *mut NSError| {
        // SAFETY: WebKit hands over a valid image, or null with an error.
        *shots.lock().unwrap() = Some(unsafe { image.as_ref() }.and_then(pixels));
        arrived();
    });
    let view = webview.webview();
    // SAFETY: on the main thread (panels are drawn there), and the block
    // takes the arguments WebKit passes.
    unsafe { view.takeSnapshotWithConfiguration_completionHandler(None, &done) };
}

fn pixels(image: &NSImage) -> Option<Shot> {
    // SAFETY: null arguments ask for the image's own best representation.
    let picture =
        unsafe { image.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) }?;
    let picture = objc2::rc::Retained::as_ptr(&picture).cast::<c_void>();
    // SAFETY: plain CoreGraphics calls on a live image and a buffer of
    // exactly the size the context is told.
    unsafe {
        let (width, height) = (CGImageGetWidth(picture), CGImageGetHeight(picture));
        if width == 0 || height == 0 {
            return None;
        }
        let mut pixels = vec![0u8; width * height * 4];
        let space = CGColorSpaceCreateDeviceRGB();
        let context = CGBitmapContextCreate(
            pixels.as_mut_ptr().cast(),
            width,
            height,
            8,
            width * 4,
            space,
            RGBA_PREMULTIPLIED,
        );
        CGColorSpaceRelease(space);
        if context.is_null() {
            return None;
        }
        CGContextDrawImage(
            context,
            CGRect {
                x: 0.0,
                y: 0.0,
                width: width as f64,
                height: height as f64,
            },
            picture,
        );
        CGContextRelease(context);
        Some(Shot {
            pixels,
            size: [width, height],
        })
    }
}
