//! macOS: right-click bEd in the Dock for New Window.
//!
//! winit owns the application delegate, so `applicationDockMenu:` is added to
//! its class at runtime. The menu is built once and lives for the process.
use bevy::{ecs::system::NonSendMarker, prelude::*};
use objc2::{
    MainThreadOnly, define_class, ffi, msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject, Imp, NSObject, Sel},
    sel,
};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSString, ns_string};
use std::sync::atomic::{AtomicPtr, Ordering};

pub struct DockMenuPlugin;

impl Plugin for DockMenuPlugin {
    fn build(&self, app: &mut App) {
        // Startup runs inside winit's event loop, after it set the delegate.
        app.add_systems(Startup, install);
    }
}

/// The menu handed to AppKit, leaked: it lives as long as the process.
static MENU: AtomicPtr<NSMenu> = AtomicPtr::new(std::ptr::null_mut());

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BedDockMenuTarget"]
    struct Target;

    impl Target {
        #[unsafe(method(newWindow:))]
        fn new_window(&self, _sender: Option<&AnyObject>) {
            if let Err(error) = crate::menus::spawn_window() {
                error!("{error}");
            }
        }
    }
);

extern "C-unwind" fn dock_menu(_: *mut AnyObject, _: Sel, _: *mut AnyObject) -> *mut NSMenu {
    MENU.load(Ordering::Relaxed)
}

fn install(_: NonSendMarker) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(delegate) = AnyClass::get(c"WinitApplicationDelegate") else {
        warn!("No winit application delegate; no Dock menu");
        return;
    };

    // Menu items hold their target weakly, so the target is leaked too.
    let target: Retained<Target> = unsafe { msg_send![Target::alloc(mtm), init] };
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("New Window"),
            Some(sel!(newWindow:)),
            ns_string!(""),
        )
    };
    unsafe { item.setTarget(Some(&target)) };
    let menu = NSMenu::new(mtm);
    menu.addItem(&item);
    std::mem::forget(target);
    MENU.store(Retained::into_raw(menu), Ordering::Relaxed);

    let imp: extern "C-unwind" fn(_, _, _) -> _ = dock_menu;
    unsafe {
        ffi::class_addMethod(
            (delegate as *const AnyClass).cast_mut(),
            sel!(applicationDockMenu:),
            std::mem::transmute::<_, Imp>(imp),
            c"@@:@".as_ptr(),
        );
    }
    // AppKit may have cached what the delegate responds to; set it again.
    let app = NSApplication::sharedApplication(mtm);
    app.setDelegate(app.delegate().as_deref());
}
