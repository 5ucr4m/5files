//! Mouse back / forward buttons on macOS. WKWebView reports them as a middle click (or
//! not at all), so they're caught natively and sent to the UI as `mouse-nav` events.
//! On Windows and Linux the page receives them itself (`button` 3 and 4).

use tauri::{AppHandle, Runtime};

#[cfg(target_os = "macos")]
pub fn install<R: Runtime>(app: &AppHandle<R>) {
    use block2::RcBlock;
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventType};
    use std::ptr::NonNull;
    use tauri::Emitter;

    let app = app.clone();
    let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the monitor a valid event for the duration of the call.
        let ev = unsafe { event.as_ref() };
        let dir = match ev.buttonNumber() {
            3 => "back",
            4 => "forward",
            _ => return event.as_ptr(),
        };
        if ev.r#type() == NSEventType::OtherMouseDown {
            let _ = app.emit("mouse-nav", dir);
        }
        // Swallow both down and up so the page never sees a stray middle click.
        std::ptr::null_mut()
    });
    // SAFETY: called on the main thread during setup; the block lives as long as the app.
    let monitor =
        unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::OtherMouseDown | NSEventMask::OtherMouseUp, &block) };
    // The monitor stays installed for the app's lifetime.
    std::mem::forget(monitor);
    std::mem::forget(block);
}

#[cfg(not(target_os = "macos"))]
pub fn install<R: Runtime>(_app: &AppHandle<R>) {}
