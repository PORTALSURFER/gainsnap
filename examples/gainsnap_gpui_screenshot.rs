//! Render GainSnap through the live embedded GPUI host and write release PNGs.

#![allow(unexpected_cfgs)]

#[cfg(target_os = "macos")]
mod macos {
    use cocoa::appkit::{NSApp, NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
    use cocoa::base::{id, nil};
    use cocoa::foundation::{NSAutoreleasePool, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize};
    use gainsnap::gui_gpui::{new_screenshot_gui, WINDOW_HEIGHT, WINDOW_WIDTH};
    use image::{imageops::FilterType, ImageFormat, RgbaImage};
    use objc::runtime::{Object, NO, YES};
    use objc::{class, msg_send, sel, sel_impl};
    use raw_window_handle::{AppKitWindowHandle, RawWindowHandle};
    use std::ffi::CString;
    use std::path::{Path, PathBuf};
    use std::ptr::NonNull;
    use std::thread;
    use std::time::Duration;

    const OUTPUT_WIDTH: u32 = WINDOW_WIDTH;
    const OUTPUT_HEIGHT: u32 = WINDOW_HEIGHT;

    struct NativeFixture {
        window: id,
        view: id,
    }

    impl NativeFixture {
        unsafe fn new() -> Self {
            let _ = NSApp();
            let frame = NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(f64::from(OUTPUT_WIDTH), f64::from(OUTPUT_HEIGHT)),
            );
            let window = NSWindow::alloc(nil).initWithContentRect_styleMask_backing_defer_(
                frame,
                NSWindowStyleMask::NSBorderlessWindowMask,
                NSBackingStoreType::NSBackingStoreBuffered,
                false,
            );
            let view = NSView::alloc(nil).initWithFrame_(frame);
            window.setContentView_(view);
            // Match is disabled when the hosted editor becomes hidden. Keep
            // this fixture visible so the active-state captures exercise the
            // same lifecycle as a real plugin window.
            window.makeKeyAndOrderFront_(nil);
            Self { window, view }
        }

        fn parent_handle(&self) -> RawWindowHandle {
            let mut handle = AppKitWindowHandle::empty();
            handle.ns_view = NonNull::new(self.view as *mut _)
                .expect("fixture view pointer")
                .as_ptr();
            RawWindowHandle::AppKit(handle)
        }
    }

    impl Drop for NativeFixture {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.window, setContentView: nil];
                let _: () = msg_send![self.view, release];
                let _: () = msg_send![self.window, release];
            }
        }
    }

    unsafe fn pump_appkit(app: id, gui: &toybox::gpui_gui::GpuiHostedGui, seconds: f64) {
        let deadline = std::time::Instant::now() + Duration::from_secs_f64(seconds);
        while std::time::Instant::now() < deadline {
            let date: id = msg_send![
                class!(NSDate),
                dateWithTimeIntervalSinceNow: 0.005_f64
            ];
            let event: id = msg_send![
                app,
                nextEventMatchingMask: usize::MAX
                untilDate: date
                inMode: NSDefaultRunLoopMode
                dequeue: YES
            ];
            if !event.is_null() {
                let _: () = msg_send![app, sendEvent: event];
            }
            let _: () = msg_send![app, updateWindows];
            gui.pump();
            thread::sleep(Duration::from_millis(1));
        }
    }

    unsafe fn send_key(
        app: id,
        window: id,
        gui: &toybox::gpui_gui::GpuiHostedGui,
        text: &str,
        key_code: u16,
        modifiers: u64,
    ) {
        let bytes = CString::new(text).expect("key text has no nul");
        let characters: id = msg_send![
            class!(NSString),
            stringWithUTF8String: bytes.as_ptr()
        ];
        let window_number: isize = msg_send![window, windowNumber];
        let down: id = msg_send![
            class!(NSEvent),
            keyEventWithType: 10_usize
            location: NSPoint { x: 48.0, y: 28.0 }
            modifierFlags: modifiers
            timestamp: 0.0_f64
            windowNumber: window_number
            context: std::ptr::null_mut::<Object>()
            characters: characters
            charactersIgnoringModifiers: characters
            isARepeat: NO
            keyCode: key_code
        ];
        let _: () = msg_send![window, sendEvent: down];
        let up: id = msg_send![
            class!(NSEvent),
            keyEventWithType: 11_usize
            location: NSPoint { x: 48.0, y: 28.0 }
            modifierFlags: modifiers
            timestamp: 0.0_f64
            windowNumber: window_number
            context: std::ptr::null_mut::<Object>()
            characters: characters
            charactersIgnoringModifiers: characters
            isARepeat: NO
            keyCode: key_code
        ];
        let _: () = msg_send![window, sendEvent: up];
        pump_appkit(app, gui, 0.02);
    }

    unsafe fn exercise_native_target_input(fixture: &NativeFixture) {
        let app = NSApp();
        let (mut gui, params) = gainsnap::gui_gpui::new_screenshot_gui_with_params(false, false);
        gui.set_parent_raw(fixture.parent_handle());
        assert!(gui.open(), "GPUI input fixture should open");
        gui.request_resize(OUTPUT_WIDTH, OUTPUT_HEIGHT);
        let _: () = msg_send![
            fixture.window,
            makeFirstResponder: std::ptr::null_mut::<Object>()
        ];
        pump_appkit(app, &gui, 0.1);

        let window_number: isize = msg_send![fixture.window, windowNumber];
        for event_type in [1_usize, 2_usize] {
            let click: id = msg_send![
                class!(NSEvent),
                mouseEventWithType: event_type
                location: NSPoint { x: 48.0, y: 28.0 }
                modifierFlags: 0_u64
                timestamp: 0.0_f64
                windowNumber: window_number
                context: std::ptr::null_mut::<Object>()
                eventNumber: 1_isize
                clickCount: 1_isize
                pressure: 1.0_f64
            ];
            let _: () = msg_send![fixture.window, sendEvent: click];
        }
        pump_appkit(app, &gui, 0.03);

        const COMMAND: u64 = 1_u64 << 20;
        const SHIFT: u64 = 1_u64 << 17;
        send_key(app, fixture.window, &gui, "a", 0, COMMAND);
        send_key(app, fixture.window, &gui, "-", 27, 0);
        send_key(app, fixture.window, &gui, "1", 18, 0);
        send_key(app, fixture.window, &gui, "5", 23, 0);
        send_key(app, fixture.window, &gui, "\r", 36, 0);
        assert_eq!(params.target_db(), -15.0, "native text entry should commit");

        send_key(app, fixture.window, &gui, "\u{f700}", 126, 0);
        send_key(app, fixture.window, &gui, "\u{f701}", 125, SHIFT);
        assert_eq!(
            params.target_db(),
            -14.1,
            "native arrows should step the target"
        );
        eprintln!(
            "PASS native GPUI GainSnap target selection, typing, commit, arrows and Shift-arrows"
        );
        gui.close();

        // A VST3 editor instance is retained while its native child is removed
        // and attached again. Exercise that exact lifecycle before repeating
        // the text-field interaction: the first open alone cannot expose
        // stale GPUI hitbox or listener state from a previous application.
        for _ in 0..3 {
            gui.set_parent_raw(fixture.parent_handle());
            assert!(gui.open(), "reopened GPUI input fixture should open");
            gui.request_resize(OUTPUT_WIDTH, OUTPUT_HEIGHT);
            pump_appkit(app, &gui, 0.03);
            gui.close();
        }
        gui.set_parent_raw(fixture.parent_handle());
        assert!(gui.open(), "final reopened GPUI input fixture should open");
        gui.request_resize(OUTPUT_WIDTH, OUTPUT_HEIGHT);
        let _: () = msg_send![
            fixture.window,
            makeFirstResponder: std::ptr::null_mut::<Object>()
        ];
        pump_appkit(app, &gui, 0.1);

        let window_number: isize = msg_send![fixture.window, windowNumber];
        for event_type in [1_usize, 2_usize] {
            let click: id = msg_send![
                class!(NSEvent),
                mouseEventWithType: event_type
                location: NSPoint { x: 48.0, y: 28.0 }
                modifierFlags: 0_u64
                timestamp: 0.0_f64
                windowNumber: window_number
                context: std::ptr::null_mut::<Object>()
                eventNumber: 1_isize
                clickCount: 1_isize
                pressure: 1.0_f64
            ];
            let _: () = msg_send![fixture.window, sendEvent: click];
        }
        pump_appkit(app, &gui, 0.03);
        send_key(app, fixture.window, &gui, "a", 0, COMMAND);
        send_key(app, fixture.window, &gui, "-", 27, 0);
        send_key(app, fixture.window, &gui, "1", 18, 0);
        send_key(app, fixture.window, &gui, "5", 23, 0);
        send_key(app, fixture.window, &gui, "\r", 36, 0);
        assert_eq!(
            params.target_db(),
            -15.0,
            "reopened native text entry should still commit"
        );
        gui.close();
    }

    fn output_root() -> PathBuf {
        std::env::var_os("TOYBOX_UI_SCREENSHOT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target/ui-screenshots"))
            .join("gainsnap")
    }

    fn write_capture(root: &Path, name: &str, width: u32, height: u32, pixels: Vec<u8>) {
        let expected_len = usize::try_from(width)
            .expect("capture width fits usize")
            .saturating_mul(usize::try_from(height).expect("capture height fits usize"))
            .saturating_mul(4);
        assert_eq!(
            pixels.len(),
            expected_len,
            "GPUI capture has invalid row length"
        );
        let image = RgbaImage::from_raw(width, height, pixels).expect("valid GPUI RGBA capture");
        let image = if width == OUTPUT_WIDTH && height == OUTPUT_HEIGHT {
            image
        } else {
            image::imageops::resize(&image, OUTPUT_WIDTH, OUTPUT_HEIGHT, FilterType::Lanczos3)
        };
        assert_eq!(image.width(), OUTPUT_WIDTH);
        assert_eq!(image.height(), OUTPUT_HEIGHT);
        image
            .save_with_format(
                root.join(format!("{name}-{OUTPUT_WIDTH}x{OUTPUT_HEIGHT}.png")),
                ImageFormat::Png,
            )
            .expect("screenshot should be writable");
    }

    fn capture_state(
        fixture: &NativeFixture,
        root: &Path,
        name: &str,
        matching: bool,
        rms: bool,
        dim: bool,
    ) {
        let mut gui = new_screenshot_gui(matching, rms);
        gui.set_parent_raw(fixture.parent_handle());
        assert!(gui.open(), "GPUI screenshot editor should open");
        gui.request_resize(OUTPUT_WIDTH, OUTPUT_HEIGHT);
        for _ in 0..8 {
            gui.pump();
            thread::sleep(Duration::from_millis(2));
        }
        if dim {
            thread::sleep(Duration::from_millis(600));
        }
        let (width, height, pixels) = gui.capture_rgba().expect("GPUI RGBA capture");
        eprintln!("{name}: captured {width}x{height}");
        write_capture(root, name, width, height, pixels);
        gui.close();
    }

    pub fn run() {
        unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let fixture = NativeFixture::new();
            let root = output_root();
            std::fs::create_dir_all(&root).expect("screenshot directory should be writable");
            capture_state(&fixture, &root, "initial-ui", false, false, false);
            capture_state(&fixture, &root, "matching-bright", true, false, false);
            capture_state(&fixture, &root, "matching-dim", true, false, true);
            capture_state(&fixture, &root, "rms-mode", true, true, false);
            exercise_native_target_input(&fixture);
            drop(fixture);
            pool.drain();
        }
    }
}

#[cfg(target_os = "macos")]
fn main() {
    macos::run();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("GainSnap GPUI screenshot runner requires macOS");
    std::process::exit(1);
}
