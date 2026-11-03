//! Reliable modifier keys for mouse gestures.
//!
//! The GUI layer learns about Cmd (and Ctrl) from key events. Inside a DAW the
//! key-*release* often never reaches the plugin window (e.g. after Cmd+S or
//! Cmd+Z went to the host), so the key stays "held" forever and plain drags
//! turn into Cmd+drags. Ctrl is also reported with every mouse event (always
//! current); Cmd is not, so on macOS we ask the system for the live state.

/// Is the Cmd key physically held right now? Always false outside macOS.
#[cfg(target_os = "macos")]
pub fn cmd_held() -> bool {
    use std::ffi::{c_char, c_void};

    #[link(name = "AppKit", kind = "framework")]
    extern "C" {}
    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    /// NSEventModifierFlagCommand
    const COMMAND: usize = 1 << 20;

    // SAFETY: `+[NSEvent modifierFlags]` is a class property taking no
    // arguments and returning an NSUInteger; objc_msgSend is called through a
    // function pointer of exactly that signature, as Apple requires.
    unsafe {
        let class = objc_getClass(c"NSEvent".as_ptr());
        if class.is_null() {
            return false;
        }
        let selector = sel_registerName(c"modifierFlags".as_ptr());
        let send: unsafe extern "C" fn(*mut c_void, *mut c_void) -> usize =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        send(class, selector) & COMMAND != 0
    }
}

#[cfg(not(target_os = "macos"))]
pub fn cmd_held() -> bool {
    false
}
