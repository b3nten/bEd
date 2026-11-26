//! What wry doesn't offer of a WKWebView: a picture of its page (asked of
//! WebKit, delivered later on the main thread), making it see-through,
//! whether it has the keyboard, and an item of ours in its right-click menu.
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

/// An item at the end of the page's right-click menu, which hides or shows
/// the panel's address bar.
///
/// The menu is the web view's own (AppKit's, not the app's), and wry has no
/// say in it. AppKit tells a view its menu is about to open
/// (`willOpenMenu:withEvent:`), so wry's web view class is given that
/// method, once, and it adds the item. The item's action is a second method
/// on the class: it notes that the item was chosen, for the panel to find
/// when it next draws (`menu_chose_toggle`).
mod menu {
    use objc2::rc::{Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
    use objc2::{ffi, msg_send, sel};
    use objc2_foundation::NSString;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::{Once, OnceLock};

    /// What a web view's menu item says, and whether it's been chosen.
    pub struct Item {
        pub hidden: bool,
        pub chosen: bool,
        pub wake: Box<dyn Fn()>,
    }

    thread_local! {
        /// By web view (its address): views and their menus are the main
        /// thread's.
        pub static ITEMS: RefCell<HashMap<usize, Item>> = RefCell::new(HashMap::new());
    }

    /// The class the methods were added to, to reach what it inherits.
    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();

    pub fn add_methods(view: &AnyObject) {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            // The class it was made as (`-class`), not the one it has at run
            // time: an observed object is quietly given a subclass of its
            // own, which other web views wouldn't share and AppKit doesn't
            // ask about what it responds to.
            // SAFETY: every object answers `class`.
            let class: &'static AnyClass = unsafe { msg_send![view, class] };
            let _ = CLASS.set(class);
            let class = std::ptr::from_ref(class).cast_mut();
            let will_open: unsafe extern "C-unwind" fn(_, _, _, _) = will_open_menu;
            let toggle: unsafe extern "C-unwind" fn(_, _, _) = toggle_address_bar;
            // SAFETY: the methods take what their selectors pass (an
            // object, a selector, then one or two objects) and return
            // nothing, as their type strings say. A class that already has
            // one keeps its own (nothing is added).
            unsafe {
                ffi::class_addMethod(
                    class,
                    sel!(willOpenMenu:withEvent:),
                    std::mem::transmute::<_, Imp>(will_open),
                    c"v@:@@".as_ptr(),
                );
                ffi::class_addMethod(
                    class,
                    sel!(bedToggleAddressBar:),
                    std::mem::transmute::<_, Imp>(toggle),
                    c"v@:@".as_ptr(),
                );
            }
        });
    }

    unsafe extern "C-unwind" fn will_open_menu(
        this: &AnyObject,
        _: Sel,
        menu: &AnyObject,
        event: *mut AnyObject,
    ) {
        // What the web view would have done with it first.
        if let Some(parent) = CLASS.get().and_then(|class| class.superclass()) {
            // SAFETY: the inherited method of the same selector.
            let _: () =
                unsafe { msg_send![super(this, parent), willOpenMenu: menu, withEvent: event] };
        }
        let key = std::ptr::from_ref(this) as usize;
        let Some(hidden) = ITEMS.with_borrow(|items| items.get(&key).map(|item| item.hidden))
        else {
            // Not a panel's web view.
            return;
        };
        let Some(items) = AnyClass::get(c"NSMenuItem") else {
            return;
        };
        let title = NSString::from_str(if hidden {
            "Show Address Bar"
        } else {
            "Hide Address Bar"
        });
        let no_key = NSString::from_str("");
        // SAFETY: AppKit's own calls, with the arguments they take, on the
        // main thread (a menu is opening).
        unsafe {
            let separator: Retained<AnyObject> = msg_send![items, separatorItem];
            let _: () = msg_send![menu, addItem: &*separator];
            let item: Allocated<AnyObject> = msg_send![items, alloc];
            let item: Retained<AnyObject> = msg_send![
                item,
                initWithTitle: &*title,
                action: sel!(bedToggleAddressBar:),
                keyEquivalent: &*no_key,
            ];
            let _: () = msg_send![&*item, setTarget: this];
            let _: () = msg_send![menu, addItem: &*item];
        }
    }

    unsafe extern "C-unwind" fn toggle_address_bar(this: &AnyObject, _: Sel, _: *mut AnyObject) {
        let key = std::ptr::from_ref(this) as usize;
        ITEMS.with_borrow_mut(|items| {
            if let Some(item) = items.get_mut(&key) {
                item.chosen = true;
                (item.wake)();
            }
        });
    }
}

fn menu_key(webview: &wry::WebView) -> usize {
    objc2::rc::Retained::as_ptr(&webview.webview()) as usize
}

/// Give `webview`'s right-click menu the item that hides or shows the
/// address bar. `wake` is called when it's chosen (the app may be asleep).
pub fn add_menu_item(webview: &wry::WebView, wake: impl Fn() + 'static) {
    let view = webview.webview();
    menu::add_methods(&view);
    menu::ITEMS.with_borrow_mut(|items| {
        items.insert(
            menu_key(webview),
            menu::Item {
                hidden: false,
                chosen: false,
                wake: Box::new(wake),
            },
        );
    });
}

/// `webview` is going: its menu item is forgotten.
pub fn forget_menu_item(webview: &wry::WebView) {
    menu::ITEMS.with_borrow_mut(|items| items.remove(&menu_key(webview)));
}

/// Tell `webview`'s menu item whether the address bar is hidden (what it
/// offers is the other), and whether it has been chosen since last asked.
pub fn menu_chose_toggle(webview: &wry::WebView, hidden: bool) -> bool {
    menu::ITEMS.with_borrow_mut(|items| {
        items.get_mut(&menu_key(webview)).is_some_and(|item| {
            item.hidden = hidden;
            std::mem::take(&mut item.chosen)
        })
    })
}
