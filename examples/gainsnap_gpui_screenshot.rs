//! Render GainSnap through the live embedded GPUI host and write release PNGs.

#![allow(unexpected_cfgs)]

#[cfg(target_os = "macos")]
mod macos {
    use cocoa::appkit::{NSApp, NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
    use cocoa::base::{id, nil};
    use cocoa::foundation::{NSAutoreleasePool, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize};
    use gainsnap::gui_gpui::{new_screenshot_gui, WINDOW_HEIGHT, WINDOW_WIDTH};
    use image::{imageops::FilterType, ImageFormat, RgbaImage};
    use objc::runtime::{Object, BOOL, NO, YES};
    use objc::{class, msg_send, sel, sel_impl};
    use raw_window_handle::{AppKitWindowHandle, RawWindowHandle};
    use std::ffi::{CStr, CString};
    use std::path::{Path, PathBuf};
    use std::ptr::NonNull;
    use std::thread;
    use std::time::Duration;
    use toybox::clack_plugin::utils::ClapId;

    const OUTPUT_WIDTH: u32 = WINDOW_WIDTH;
    const OUTPUT_HEIGHT: u32 = WINDOW_HEIGHT;
    const PARAM_TARGET_DB: ClapId = ClapId::new(1);
    const PARAM_MATCH: ClapId = ClapId::new(2);
    const PARAM_RMS_MODE: ClapId = ClapId::new(4);

    struct NativeFixture {
        window: id,
        view: id,
    }

    struct PasteboardSnapshot {
        items: Vec<PasteboardItemSnapshot>,
    }

    struct PasteboardItemSnapshot {
        types: Vec<(String, Vec<u8>)>,
    }

    struct PasteboardRestore(PasteboardSnapshot);

    impl PasteboardRestore {
        unsafe fn capture() -> Self {
            Self(PasteboardSnapshot::capture())
        }
    }

    impl Drop for PasteboardRestore {
        fn drop(&mut self) {
            unsafe {
                self.0.restore();
            }
        }
    }

    impl PasteboardSnapshot {
        unsafe fn capture() -> Self {
            let pasteboard: id = msg_send![class!(NSPasteboard), generalPasteboard];
            if pasteboard.is_null() {
                return Self { items: Vec::new() };
            }
            let pasteboard_items: id = msg_send![pasteboard, pasteboardItems];
            if pasteboard_items.is_null() {
                return Self { items: Vec::new() };
            }
            let item_count: usize = msg_send![pasteboard_items, count];
            let mut items = Vec::with_capacity(item_count);
            for item_index in 0..item_count {
                let item: id = msg_send![pasteboard_items, objectAtIndex: item_index];
                if item.is_null() {
                    continue;
                }
                let item_types: id = msg_send![item, types];
                if item_types.is_null() {
                    items.push(PasteboardItemSnapshot { types: Vec::new() });
                    continue;
                }
                let type_count: usize = msg_send![item_types, count];
                let mut types = Vec::with_capacity(type_count);
                for type_index in 0..type_count {
                    let type_object: id = msg_send![item_types, objectAtIndex: type_index];
                    let type_name =
                        ns_string(type_object).expect("pasteboard type should expose UTF-8 text");
                    let data: id = msg_send![item, dataForType: type_object];
                    let data_length = if data.is_null() {
                        0
                    } else {
                        msg_send![data, length]
                    };
                    let bytes = if data_length == 0 {
                        Vec::new()
                    } else {
                        let pointer: *const u8 = msg_send![data, bytes];
                        assert!(!pointer.is_null(), "pasteboard data should have bytes");
                        std::slice::from_raw_parts(pointer, data_length).to_vec()
                    };
                    types.push((type_name, bytes));
                }
                items.push(PasteboardItemSnapshot { types });
            }
            Self { items }
        }

        unsafe fn restore(&self) {
            let pasteboard: id = msg_send![class!(NSPasteboard), generalPasteboard];
            if pasteboard.is_null() {
                return;
            }
            let _: isize = msg_send![pasteboard, clearContents];
            if self.items.is_empty() {
                return;
            }
            let objects: id =
                msg_send![class!(NSMutableArray), arrayWithCapacity: self.items.len()];
            if objects.is_null() {
                return;
            }
            for snapshot in &self.items {
                let allocated: id = msg_send![class!(NSPasteboardItem), alloc];
                let item: id = msg_send![allocated, init];
                if item.is_null() {
                    continue;
                }
                for (type_name, bytes) in &snapshot.types {
                    let type_c_string = CString::new(type_name.as_bytes())
                        .expect("pasteboard type should not contain nul");
                    let type_object: id = msg_send![
                        class!(NSString),
                        stringWithUTF8String: type_c_string.as_ptr()
                    ];
                    if type_object.is_null() {
                        continue;
                    }
                    let data: id = msg_send![
                        class!(NSData),
                        dataWithBytes: bytes.as_ptr()
                        length: bytes.len()
                    ];
                    let _: BOOL = msg_send![item, setData: data forType: type_object];
                }
                let _: () = msg_send![objects, addObject: item];
                let _: () = msg_send![item, release];
            }
            let _: BOOL = msg_send![pasteboard, writeObjects: objects];
        }
    }

    unsafe fn ns_string(value: id) -> Option<String> {
        if value.is_null() {
            return None;
        }
        let pointer: *const i8 = msg_send![value, UTF8String];
        if pointer.is_null() {
            return None;
        }
        CStr::from_ptr(pointer).to_str().ok().map(str::to_owned)
    }

    unsafe fn pasteboard_string() -> Option<String> {
        let pasteboard: id = msg_send![class!(NSPasteboard), generalPasteboard];
        let type_name = CString::new("public.utf8-plain-text").expect("static type name");
        let type_object: id = msg_send![
            class!(NSString),
            stringWithUTF8String: type_name.as_ptr()
        ];
        let value: id = msg_send![pasteboard, stringForType: type_object];
        ns_string(value)
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

    unsafe fn send_click(window: id, x: f64, top_y: f64) {
        let window_number: isize = msg_send![window, windowNumber];
        let location = NSPoint::new(x, f64::from(OUTPUT_HEIGHT) - top_y);
        for event_type in [1_usize, 2_usize] {
            let click: id = msg_send![
                class!(NSEvent),
                mouseEventWithType: event_type
                location: location
                modifierFlags: 0_u64
                timestamp: 0.0_f64
                windowNumber: window_number
                context: std::ptr::null_mut::<Object>()
                eventNumber: 1_isize
                clickCount: 1_isize
                pressure: 1.0_f64
            ];
            let _: () = msg_send![window, sendEvent: click];
        }
    }

    unsafe fn select_target_with_mouse(window: id) {
        let window_number: isize = msg_send![window, windowNumber];
        for (event_type, x) in [(1_usize, 78.0), (6_usize, 1.0), (2_usize, 1.0)] {
            let event: id = msg_send![class!(NSEvent),
                mouseEventWithType: event_type
                location: NSPoint::new(x, f64::from(OUTPUT_HEIGHT) - 184.0)
                modifierFlags: 0_u64 timestamp: 0.0_f64 windowNumber: window_number
                context: std::ptr::null_mut::<Object>() eventNumber: 1_isize
                clickCount: 1_isize pressure: 1.0_f64];
            let _: () = msg_send![window, sendEvent: event];
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
        send_key_with_repeat(app, window, gui, text, key_code, modifiers, false);
    }

    unsafe fn send_repeated_key(
        app: id,
        window: id,
        gui: &toybox::gpui_gui::GpuiHostedGui,
        text: &str,
        key_code: u16,
        modifiers: u64,
    ) {
        send_key_with_repeat(app, window, gui, text, key_code, modifiers, true);
    }

    unsafe fn send_key_with_repeat(
        app: id,
        window: id,
        gui: &toybox::gpui_gui::GpuiHostedGui,
        text: &str,
        key_code: u16,
        modifiers: u64,
        repeat: bool,
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
        if repeat {
            let repeated_down: id = msg_send![
                class!(NSEvent),
                keyEventWithType: 10_usize
                location: NSPoint { x: 48.0, y: 28.0 }
                modifierFlags: modifiers
                timestamp: 0.0_f64
                windowNumber: window_number
                context: std::ptr::null_mut::<Object>()
                characters: characters
                charactersIgnoringModifiers: characters
                isARepeat: YES
                keyCode: key_code
            ];
            let _: () = msg_send![window, sendEvent: repeated_down];
        }
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
        // The fixture owns no clipboard data. Snapshot every item and every
        // materialized type before exercising the editor so a failed native
        // assertion cannot leave the user's clipboard changed.
        let _pasteboard_restore = PasteboardRestore::capture();
        let (mut gui, params) = gainsnap::gui_gpui::new_screenshot_gui_with_params(false, false);
        gui.set_parent_raw(fixture.parent_handle());
        assert!(gui.open(), "GPUI input fixture should open");
        gui.request_resize(OUTPUT_WIDTH, OUTPUT_HEIGHT);
        let _: () = msg_send![
            fixture.window,
            makeFirstResponder: std::ptr::null_mut::<Object>()
        ];
        pump_appkit(app, &gui, 0.1);

        const COMMAND: u64 = 1_u64 << 20;
        const SHIFT: u64 = 1_u64 << 17;
        const FUNCTION: u64 = 1_u64 << 23;
        const NUMERIC_PAD: u64 = 1_u64 << 21;

        // With no focused GPUI control, Space remains available to the host.
        send_repeated_key(app, fixture.window, &gui, " ", 49, 0);
        assert!(
            !params.match_requested(),
            "unfocused Space must not toggle Match"
        );

        send_click(fixture.window, 48.0, 184.0);
        pump_appkit(app, &gui, 0.03);

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

        // Start the clipboard checks from a committed, known value. Each
        // native key event pumps the host loop, so the editor's live meter
        // updates continue while selection and clipboard actions run.
        send_key(app, fixture.window, &gui, "a", 0, COMMAND);
        send_key(app, fixture.window, &gui, "-", 27, 0);
        send_key(app, fixture.window, &gui, "1", 18, 0);
        send_key(app, fixture.window, &gui, "5", 23, 0);
        send_key(app, fixture.window, &gui, "\r", 36, 0);
        assert_eq!(
            params.target_db(),
            -15.0,
            "clipboard test should start at -15 dB"
        );

        // Cmd+C exercises the GPUI action through the real AppKit pasteboard.
        // Readback is asserted without printing the clipboard contents.
        send_key(app, fixture.window, &gui, "a", 0, COMMAND);
        send_key(app, fixture.window, &gui, "c", 8, COMMAND);
        assert!(
            pasteboard_string().as_deref() == Some("-15.0"),
            "copy should write the selected target text"
        );

        // Cmd+X removes the selection and Cmd+V reads the same native
        // pasteboard data through Toybox's platform clipboard bridge. A
        // second copy confirms the cut did not leave duplicate text behind.
        send_key(app, fixture.window, &gui, "x", 7, COMMAND);
        send_key(app, fixture.window, &gui, "v", 9, COMMAND);
        send_key(app, fixture.window, &gui, "a", 0, COMMAND);
        send_key(app, fixture.window, &gui, "c", 8, COMMAND);
        assert!(
            pasteboard_string().as_deref() == Some("-15.0"),
            "cut then paste should restore one target value"
        );
        send_key(app, fixture.window, &gui, "\r", 36, 0);
        assert_eq!(params.target_db(), -15.0, "pasted target should commit");

        // Insert an invalid draft, then cancel it with Escape. This checks the
        // native cancel action without changing the committed parameter.
        send_key(app, fixture.window, &gui, "a", 0, COMMAND);
        send_key(app, fixture.window, &gui, "x", 7, 0);
        pump_appkit(app, &gui, 0.08);
        send_key(app, fixture.window, &gui, "\u{1b}", 53, 0);
        assert_eq!(params.target_db(), -15.0, "Escape should cancel the draft");

        // Home plus Shift+Right selects one grapheme from the caret. Copying
        // it checks that the selection is routed to the native clipboard.
        send_key(app, fixture.window, &gui, "\u{f729}", 115, 0);
        send_key(app, fixture.window, &gui, "\u{f703}", 124, SHIFT);
        send_key(app, fixture.window, &gui, "c", 8, COMMAND);
        assert!(
            pasteboard_string().as_deref() == Some("-"),
            "caret selection should copy the selected grapheme"
        );

        send_key(app, fixture.window, &gui, "\u{f703}", 124, SHIFT);
        send_key(app, fixture.window, &gui, "c", 8, COMMAND);
        assert!(
            pasteboard_string().as_deref() == Some("-1"),
            "repeated Shift+Right expands selection"
        );
        send_key(app, fixture.window, &gui, "\u{f702}", 123, SHIFT);
        send_key(app, fixture.window, &gui, "c", 8, COMMAND);
        assert!(
            pasteboard_string().as_deref() == Some("-"),
            "Shift+Left shrinks selection from its caret"
        );

        // Replace a keyboard-selected digit using real native caret events.
        send_key(app, fixture.window, &gui, "\u{f729}", 115, 0);
        send_key(app, fixture.window, &gui, "\u{f703}", 124, 0);
        send_key(app, fixture.window, &gui, "\u{f703}", 124, 0);
        send_key(app, fixture.window, &gui, "\u{f703}", 124, SHIFT);
        let (w, h, pixels) = gui.capture_rgba().expect("selected numeric field capture");
        write_capture(&output_root(), "target-selection", w, h, pixels);
        send_key(app, fixture.window, &gui, "2", 19, 0);
        send_key(app, fixture.window, &gui, "\r", 36, 0);
        assert_eq!(
            params.target_db(),
            -12.0,
            "caret selection replaces only one digit"
        );

        select_target_with_mouse(fixture.window);
        pump_appkit(app, &gui, 0.03);
        send_key(app, fixture.window, &gui, "c", 8, COMMAND);
        assert!(
            pasteboard_string().as_deref() == Some("-12.0"),
            "mouse drag selects target digits"
        );

        // Every VST3 Backspace representation edits the focused draft.
        for (character, code) in [(0, 1), (127, 0), (8, 0)] {
            send_key(app, fixture.window, &gui, "a", 0, COMMAND);
            for (text, key_code) in [("-", 27), ("1", 18), ("2", 19)] {
                send_key(app, fixture.window, &gui, text, key_code, 0);
            }
            assert!(gui.on_key_down(character, code, 0), "Backspace is consumed");
            send_key(app, fixture.window, &gui, "\r", 36, 0);
            assert_eq!(params.target_db(), -1.0, "Backspace removes the last digit");
        }

        // A long draft stays editable without resizing the numeric field.
        let (before_w, before_h, before_pixels) =
            gui.capture_rgba().expect("stable controls capture");
        send_key(app, fixture.window, &gui, "a", 0, COMMAND);
        send_key(app, fixture.window, &gui, "-", 27, 0);
        for _ in 0..40 {
            send_key(app, fixture.window, &gui, "1", 18, 0);
        }
        let (w, h, pixels) = gui.capture_rgba().expect("long numeric draft capture");
        assert_eq!((w, h), (before_w, before_h));
        let right_controls_start = 90 * w / OUTPUT_WIDTH;
        for y in 0..h {
            let start = ((y * w + right_controls_start) * 4) as usize;
            let end = (((y + 1) * w) * 4) as usize;
            assert_eq!(
                &pixels[start..end],
                &before_pixels[start..end],
                "long text must not spill into or move other controls"
            );
        }
        write_capture(&output_root(), "target-long-draft", w, h, pixels);
        send_key(app, fixture.window, &gui, "a", 0, COMMAND);
        send_key(app, fixture.window, &gui, "-", 27, 0);
        send_key(app, fixture.window, &gui, "1", 18, 0);
        send_key(app, fixture.window, &gui, "2", 19, 0);
        send_key(app, fixture.window, &gui, "\r", 36, 0);
        assert_eq!(
            params.target_db(),
            -12.0,
            "long draft can be selected and replaced"
        );

        // Selecting the triangle preserves its exact target, then arrows use
        // the same whole/fine steps as the numeric field.
        send_click(fixture.window, 66.0, 66.333333);
        pump_appkit(app, &gui, 0.03);
        assert_eq!(
            params.target_db(),
            -12.0,
            "selecting arrow must not quantize target"
        );
        let (w, h, pixels) = gui.capture_rgba().expect("selected target arrow capture");
        write_capture(&output_root(), "target-arrow-selected", w, h, pixels);
        send_key(app, fixture.window, &gui, "\u{f700}", 126, 0);
        assert_eq!(params.target_db(), -11.0);
        send_key(app, fixture.window, &gui, "\u{f701}", 125, 0);
        assert_eq!(params.target_db(), -12.0);
        send_key(app, fixture.window, &gui, "\u{f700}", 126, SHIFT);
        assert!((params.target_db() + 11.9).abs() < 0.0001);
        send_key(app, fixture.window, &gui, "\u{f701}", 125, SHIFT);
        assert!((params.target_db() + 12.0).abs() < 0.0001);
        // Native AppKit arrows can carry Function and NumericPad keyboard
        // metadata. The selected meter marker must still receive its semantic
        // whole and fine steps.
        send_key(
            app,
            fixture.window,
            &gui,
            "\u{f700}",
            126,
            FUNCTION | NUMERIC_PAD,
        );
        assert_eq!(params.target_db(), -11.0, "Fn+Up steps the selected arrow");
        send_key(
            app,
            fixture.window,
            &gui,
            "\u{f701}",
            125,
            FUNCTION | NUMERIC_PAD,
        );
        assert_eq!(
            params.target_db(),
            -12.0,
            "Fn+Down steps the selected arrow"
        );
        send_key(
            app,
            fixture.window,
            &gui,
            "\u{f700}",
            126,
            FUNCTION | NUMERIC_PAD | SHIFT,
        );
        assert!(
            (params.target_db() + 11.9).abs() < 0.0001,
            "Fn+Shift+Up fine-steps the selected arrow"
        );
        send_key(
            app,
            fixture.window,
            &gui,
            "\u{f701}",
            125,
            FUNCTION | NUMERIC_PAD | SHIFT,
        );
        assert!(
            (params.target_db() + 12.0).abs() < 0.0001,
            "Fn+Shift+Down fine-steps the selected arrow"
        );
        send_key(app, fixture.window, &gui, "5", 23, 0);
        assert!(
            (params.target_db() + 12.0).abs() < 0.0001,
            "unfocused text cannot edit target"
        );

        let window_number: isize = msg_send![fixture.window, windowNumber];
        for (event_type, y) in [(1_usize, 66.333333), (6_usize, 52.0), (2_usize, 52.0)] {
            let event: id = msg_send![class!(NSEvent),
                mouseEventWithType: event_type
                location: NSPoint::new(66.0, f64::from(OUTPUT_HEIGHT) - y)
                modifierFlags: 0_u64 timestamp: 0.0_f64 windowNumber: window_number
                context: std::ptr::null_mut::<Object>() eventNumber: 1_isize
                clickCount: 1_isize pressure: 1.0_f64];
            let _: () = msg_send![fixture.window, sendEvent: event];
        }
        pump_appkit(app, &gui, 0.03);
        assert!(
            params.target_db() > -11.5 && params.target_db() < -7.0,
            "selected arrow still supports dragging"
        );

        // Exercise native focus plus GPUI's keyboard click path for every
        // action control. A repeated Space keydown must still produce only
        // one keyup click, and modified activation keys must pass through.
        send_click(fixture.window, 144.0, 85.0);
        pump_appkit(app, &gui, 0.03);
        assert!(
            params.match_requested(),
            "native Match click should activate"
        );
        send_repeated_key(app, fixture.window, &gui, " ", 49, 0);
        assert!(
            !params.match_requested(),
            "repeated Space should toggle Match only once"
        );
        send_key(app, fixture.window, &gui, " ", 49, SHIFT);
        assert!(
            !params.match_requested(),
            "modified Space must not activate Match"
        );
        send_key(app, fixture.window, &gui, "\r", 36, 0);
        assert!(
            params.match_requested(),
            "Enter should activate the focused Match button"
        );

        send_click(fixture.window, 144.0, 51.0);
        pump_appkit(app, &gui, 0.03);
        assert!(params.rms_mode(), "native RMS click should activate");
        send_key(app, fixture.window, &gui, "\r", 36, 0);
        assert!(
            !params.rms_mode(),
            "Enter should activate the focused RMS button"
        );

        send_click(fixture.window, 144.0, 119.0);
        pump_appkit(app, &gui, 0.03);
        assert_eq!(
            params.target_db(),
            0.0,
            "native Normalize click should set 0 dB"
        );
        assert!(
            !params.rms_mode(),
            "Normalize should leave peak mode selected"
        );
        assert!(params.match_requested(), "Normalize should enable Match");

        // Change the shared state behind the still-focused Normalize control,
        // then require Enter to perform the action again. This distinguishes
        // keyboard activation from the preceding mouse click.
        params.set_param(PARAM_TARGET_DB, -15.0);
        params.set_param(PARAM_RMS_MODE, 1.0);
        params.set_param(PARAM_MATCH, 0.0);
        pump_appkit(app, &gui, 0.08);
        send_key(app, fixture.window, &gui, "\r", 36, SHIFT);
        assert_eq!(
            params.target_db(),
            -15.0,
            "modified Enter must pass through"
        );
        assert!(params.rms_mode(), "modified Enter must not normalize");
        assert!(
            !params.match_requested(),
            "modified Enter must not enable Match"
        );
        send_key(app, fixture.window, &gui, "\r", 36, 0);
        assert_eq!(params.target_db(), 0.0, "Enter should activate Normalize");
        assert!(
            !params.rms_mode(),
            "keyboard Normalize should force peak mode"
        );
        assert!(
            params.match_requested(),
            "keyboard Normalize should enable Match"
        );
        pump_appkit(app, &gui, 0.12);
        eprintln!(
            "PASS native GPUI GainSnap target selection, typing, commit, clipboard copy/cut/paste, Escape cancel, caret selection, arrows, and focused-button Space/Enter activation"
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
