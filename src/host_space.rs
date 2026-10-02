//! Forward Space through AppKit's responder chain for the host's transport.
#![allow(unexpected_cfgs)] // objc 0.2's legacy macro cfg.

use cocoa::base::{id, nil};
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};
use raw_window_handle_06::{HasWindowHandle, RawWindowHandle};
use toybox::gpui::Window;

extern "C" {
    #[link_name = "object_setClass"]
    fn set_object_class(object: id, class: *const Class) -> *const Class;
}

/// Add Space forwarding to this GainSnap view without modifying shared classes.
pub(crate) fn install(window: &Window) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: GainSnap renders and AppKit sends input on the main thread. The
    // subclass adds no ivars and preserves the native view's other methods.
    unsafe {
        let view = handle.ns_view.as_ptr() as id;
        let original = (*view).class();
        if !original.name().starts_with("GainSnapGpui") || original.name().ends_with("_HostSpace") {
            return;
        }
        let name = format!("{}_HostSpace", original.name());
        let class = Class::get(&name).unwrap_or_else(|| {
            let mut class =
                ClassDecl::new(&name, original).expect("GainSnap Space forwarding class is unique");
            class.add_method(sel!(keyDown:), key_down as extern "C" fn(&Object, Sel, id));
            class.add_method(sel!(keyUp:), key_up as extern "C" fn(&Object, Sel, id));
            class.register()
        });
        set_object_class(view, class);
    }
}

extern "C" fn key_down(this: &Object, _: Sel, event: id) {
    // SAFETY: These are AppKit's keyDown:/keyUp: signatures. Space is forwarded
    // once without invoking the plug-in's keyboard or text-input handlers.
    unsafe {
        let key_code: u16 = msg_send![event, keyCode];
        if key_code == 49 {
            let next: id = msg_send![this, nextResponder];
            if next != nil {
                let _: () = msg_send![next, keyDown: event];
            }
        } else {
            let superclass = this.class().superclass().expect("native GPUI view class");
            let _: () = msg_send![super(this, superclass), keyDown: event];
        }
    }
}

extern "C" fn key_up(this: &Object, _: Sel, event: id) {
    // SAFETY: Same responder-chain routing as key_down.
    unsafe {
        let key_code: u16 = msg_send![event, keyCode];
        if key_code == 49 {
            let next: id = msg_send![this, nextResponder];
            if next != nil {
                let _: () = msg_send![next, keyUp: event];
            }
        } else {
            let superclass = this.class().superclass().expect("native GPUI view class");
            let _: () = msg_send![super(this, superclass), keyUp: event];
        }
    }
}
