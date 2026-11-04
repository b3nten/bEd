//! macOS: the app's Dock menu (right-click its icon in the Dock), from
//! `set_dock_menu`. Each item runs its action globally, as a key would.
//!
//! winit owns the application delegate, so `applicationDockMenu:` is added
//! to its class at runtime. A click arrives on AppKit's side; it's queued and
//! submitted as an action request in `PreUpdate`. A rebuilt App (another
//! workspace) installs its menu again; the old one is leaked, as menus live
//! for the process.
use crate::actions::{ActionRegistry, ActionRequest, ActionRequests, InvocationContext};
use bevy::{ecs::system::NonSendMarker, prelude::*};
use objc2::{
    MainThreadOnly, define_class, ffi, msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject, Imp, NSObject, Sel},
    sel,
};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSString, ns_string};
use std::sync::{
    Mutex,
    atomic::{AtomicPtr, Ordering},
};

/// The menu handed to AppKit, leaked: it lives as long as the process.
static MENU: AtomicPtr<NSMenu> = AtomicPtr::new(std::ptr::null_mut());

/// Items clicked since the last poll, by index in `DockMenu`.
static CLICKED: Mutex<Vec<isize>> = Mutex::new(Vec::new());

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BedtimeDockMenuTarget"]
    struct Target;

    impl Target {
        #[unsafe(method(invoke:))]
        fn invoke(&self, sender: Option<&AnyObject>) {
            if let Some(sender) = sender {
                let tag: isize = unsafe { msg_send![sender, tag] };
                CLICKED.lock().unwrap().push(tag);
            }
        }
    }
);

extern "C-unwind" fn dock_menu(_: *mut AnyObject, _: Sel, _: *mut AnyObject) -> *mut NSMenu {
    MENU.load(Ordering::Relaxed)
}

pub(super) fn install(
    _: NonSendMarker,
    dock: Option<Res<super::DockMenu>>,
    registry: Res<ActionRegistry>,
) {
    let Some(dock) = dock.filter(|dock| !dock.0.is_empty()) else {
        return;
    };
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(delegate) = AnyClass::get(c"WinitApplicationDelegate") else {
        warn!("No winit application delegate; no Dock menu");
        return;
    };

    // Menu items hold their target weakly, so the target is leaked too.
    let target: Retained<Target> = unsafe { msg_send![Target::alloc(mtm), init] };
    let menu = NSMenu::new(mtm);
    for (index, action) in dock.0.iter().enumerate() {
        let Some(label) = registry.label(action) else {
            warn!(
                "The Dock menu's {} isn't a registered action",
                action.as_str()
            );
            continue;
        };
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(label),
                Some(sel!(invoke:)),
                ns_string!(""),
            )
        };
        unsafe { item.setTarget(Some(&target)) };
        item.setTag(index as isize);
        menu.addItem(&item);
    }
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

/// Run what was clicked.
pub(super) fn forward(dock: Option<Res<super::DockMenu>>, mut requests: ResMut<ActionRequests>) {
    let clicked = std::mem::take(&mut *CLICKED.lock().unwrap());
    let Some(dock) = dock else {
        return;
    };
    for tag in clicked {
        if let Some(action) = usize::try_from(tag)
            .ok()
            .and_then(|index| dock.0.get(index))
        {
            requests.submit(ActionRequest {
                action: action.clone(),
                context: InvocationContext::Global,
                source: None,
            });
        }
    }
}
