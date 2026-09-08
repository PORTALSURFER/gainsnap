//! VST3 edit controller and hosted editor creation.

#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::ffi::CStr;
use std::ffi::CString;
use std::ptr;
use std::slice;
use std::sync::Arc;

use toybox::vst3::prelude::Steinberg::*;
use toybox::vst3::prelude::*;

use crate::params::text_to_value;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use super::gui_adapter;
use super::param_bridge;
use super::shared_state::{ComponentHandlerOwner, GainSnapVst3Shared};
use super::{read_vst3_state, write_vst3_state, CONTROLLER_CID};

/// VST3 edit controller for GainSnap.
pub(super) struct GainSnapVst3Controller {
    connection: InstanceConnection<GainSnapVst3Shared>,
    /// Host handler must outlive controller shared-state adoption.
    component_handler: Arc<ComponentHandlerOwner>,
}

impl GainSnapVst3Controller {
    /// Construct an unconnected controller endpoint with default state.
    pub(super) fn new() -> Self {
        Self {
            connection: InstanceConnection::new(
                InstanceConnectionRole::Controller,
                GainSnapVst3Shared::new(),
            ),
            component_handler: Arc::new(ComponentHandlerOwner::new()),
        }
    }

    fn shared(&self) -> std::sync::Arc<GainSnapVst3Shared> {
        self.connection.shared()
    }
}

impl Class for GainSnapVst3Controller {
    type Interfaces = (IEditController, IConnectionPoint, IToyboxSharedState);
}

toybox::impl_vst3_instance_connection!(GainSnapVst3Controller, connection);

impl IPluginBaseTrait for GainSnapVst3Controller {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }

    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IEditControllerTrait for GainSnapVst3Controller {
    unsafe fn setComponentState(&self, state: *mut IBStream) -> tresult {
        let shared = self.shared();
        unsafe { read_vst3_state(state, &shared) }
    }

    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        unsafe { self.setComponentState(state) }
    }

    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let shared = self.shared();
        unsafe { write_vst3_state(state, &shared) }
    }

    unsafe fn getParameterCount(&self) -> int32 {
        crate::params::param_count() as int32
    }

    unsafe fn getParameterInfo(&self, index: int32, info: *mut ParameterInfo) -> tresult {
        if info.is_null() {
            return kInvalidArgument;
        }
        let Some(meta) = param_bridge::info(index) else {
            return kInvalidArgument;
        };
        let info = unsafe { &mut *info };
        info.id = meta.id;
        copy_wstring(meta.title, &mut info.title);
        copy_wstring(meta.short_title, &mut info.shortTitle);
        copy_wstring(meta.units, &mut info.units);
        info.stepCount = meta.step_count;
        info.defaultNormalizedValue = meta.default_normalized;
        info.unitId = 0;
        info.flags = if meta.automatable {
            ParameterInfo_::ParameterFlags_::kCanAutomate
        } else {
            0
        };
        kResultOk
    }

    unsafe fn getParamStringByValue(
        &self,
        id: ParamID,
        normalized: ParamValue,
        string: *mut String128,
    ) -> tresult {
        if string.is_null() {
            return kInvalidArgument;
        }
        let Some(plain) = param_bridge::from_normalized(id, normalized) else {
            return kInvalidArgument;
        };
        let Some(display) = param_bridge::format_value(id, plain) else {
            return kInvalidArgument;
        };
        copy_wstring(&display, unsafe { &mut *string });
        kResultOk
    }

    unsafe fn getParamValueByString(
        &self,
        id: ParamID,
        string: *mut TChar,
        normalized: *mut ParamValue,
    ) -> tresult {
        if string.is_null() || normalized.is_null() {
            return kInvalidArgument;
        }
        let Some(raw) = (unsafe { parse_tchar_string(string) }) else {
            return kInvalidArgument;
        };
        let Ok(raw) = CString::new(raw) else {
            return kInvalidArgument;
        };
        let Some(clap_id) = param_bridge::clap_id(id) else {
            return kInvalidArgument;
        };
        let Some(plain) = text_to_value(clap_id, raw.as_c_str()) else {
            return kInvalidArgument;
        };
        let Some(value) = param_bridge::to_normalized(id, plain) else {
            return kInvalidArgument;
        };
        unsafe { *normalized = value };
        kResultOk
    }

    unsafe fn normalizedParamToPlain(&self, id: ParamID, normalized: ParamValue) -> ParamValue {
        param_bridge::from_normalized(id, normalized).unwrap_or(0.0)
    }

    unsafe fn plainParamToNormalized(&self, id: ParamID, plain: ParamValue) -> ParamValue {
        param_bridge::to_normalized(id, plain).unwrap_or(0.0)
    }

    unsafe fn getParamNormalized(&self, id: ParamID) -> ParamValue {
        let shared = self.shared();
        param_bridge::read_plain(&shared.params, id)
            .and_then(|plain| param_bridge::to_normalized(id, plain))
            .unwrap_or(0.0)
    }

    unsafe fn setParamNormalized(&self, id: ParamID, normalized: ParamValue) -> tresult {
        let shared = self.shared();
        if param_bridge::apply_normalized(&shared.params, id, normalized) {
            kResultOk
        } else {
            kInvalidArgument
        }
    }

    unsafe fn setComponentHandler(&self, handler: *mut IComponentHandler) -> tresult {
        // SAFETY: the host owns the handler pointer for this callback.
        unsafe { self.component_handler.set(handler) }
    }

    unsafe fn createView(&self, name: FIDString) -> *mut IPlugView {
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = name;
            ptr::null_mut()
        }

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            if name.is_null() {
                return ptr::null_mut();
            }
            let requested = unsafe { CStr::from_ptr(name) };
            let editor = unsafe { CStr::from_ptr(ViewType::kEditor) };
            if requested.to_bytes() != editor.to_bytes() {
                return ptr::null_mut();
            }

            let shared = self.shared();
            let adapter = gui_adapter::new_gui(shared, Arc::clone(&self.component_handler));
            let view = toybox::vst3::prelude::create_gpui_view(
                adapter,
                crate::gui_gpui::WINDOW_WIDTH,
                crate::gui_gpui::WINDOW_HEIGHT,
                (
                    crate::gui_gpui::MIN_WINDOW_WIDTH,
                    crate::gui_gpui::MIN_WINDOW_HEIGHT,
                ),
                (
                    crate::gui_gpui::MAX_WINDOW_WIDTH,
                    crate::gui_gpui::MAX_WINDOW_HEIGHT,
                ),
            );
            let Some(view) = ComWrapper::new(view).to_com_ptr::<IPlugView>() else {
                return ptr::null_mut();
            };
            ComPtr::into_raw(view)
        }
    }
}

unsafe fn parse_tchar_string(string: *mut TChar) -> Option<String> {
    if string.is_null() {
        return None;
    }
    let length = unsafe { tchar_len(string) };
    let utf16 = unsafe { slice::from_raw_parts(string.cast::<u16>(), length) };
    String::from_utf16(utf16).ok()
}

#[allow(dead_code)]
const _: TUID = CONTROLLER_CID;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn custom_editor_exposes_the_native_platform_and_compact_size() {
        let controller = GainSnapVst3Controller::new();
        assert!(unsafe { controller.createView(ptr::null()) }.is_null());
        let raw = unsafe { controller.createView(ViewType::kEditor) };
        let view =
            unsafe { ComPtr::from_raw(raw) }.expect("GainSnap must expose its custom editor");
        #[cfg(target_os = "macos")]
        let platform = kPlatformTypeNSView;
        #[cfg(target_os = "windows")]
        let platform = kPlatformTypeHWND;
        assert_eq!(unsafe { view.isPlatformTypeSupported(platform) }, kResultOk);
        let mut rect = ViewRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        assert_eq!(unsafe { view.getSize(&mut rect) }, kResultOk);
        assert_eq!((rect.right, rect.bottom), (208, 212));
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_editor_attaches_handles_input_resizes_and_reopens() {
        use windows::core::w;
        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        use windows::Win32::Graphics::Gdi::UpdateWindow;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, DispatchMessageW, GetWindow, IsWindow, PeekMessageW,
            SendMessageW, ShowWindow, TranslateMessage, GW_CHILD, MSG, PM_REMOVE, SW_HIDE,
            SW_MINIMIZE, SW_RESTORE, SW_SHOW, WM_CHAR, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_TIMER,
            WS_OVERLAPPEDWINDOW,
        };

        struct Parent(HWND);
        impl Drop for Parent {
            fn drop(&mut self) {
                let _ = unsafe { DestroyWindow(self.0) };
            }
        }
        let parent = Parent(
            unsafe {
                CreateWindowExW(
                    Default::default(),
                    w!("STATIC"),
                    w!("GainSnap host test"),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    640,
                    480,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .expect("create a real Windows host parent"),
        );
        let _ = unsafe { ShowWindow(parent.0, SW_SHOW) };
        let controller = GainSnapVst3Controller::new();
        let shared = controller.shared();
        let raw = unsafe { controller.createView(ViewType::kEditor) };
        let view = unsafe { ComPtr::from_raw(raw) }.expect("native editor view");
        assert_eq!(
            unsafe { view.attached(parent.0 .0, kPlatformTypeHWND) },
            kResultOk
        );
        let child = unsafe { GetWindow(parent.0, GW_CHILD) }.expect("embedded native child");
        assert!(unsafe { IsWindow(Some(child)) }.as_bool());
        let _ = unsafe { UpdateWindow(child) };
        let mut message = MSG::default();
        for _ in 0..256 {
            if !unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                break;
            }
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }

        let mut rect = ViewRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        assert_eq!(unsafe { view.getSize(&mut rect) }, kResultOk);
        let scale = (rect.right - rect.left) as f32 / crate::gui_gpui::WINDOW_WIDTH as f32;
        let point_for = |x: f32, y: f32| -> LPARAM {
            LPARAM(
                (x.mul_add(scale, 0.0) as isize & 0xffff)
                    | ((y.mul_add(scale, 0.0) as isize & 0xffff) << 16),
            )
        };
        let point = point_for(144.0, 85.0);
        unsafe {
            SendMessageW(child, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(point));
            SendMessageW(child, WM_LBUTTONUP, Some(WPARAM(0)), Some(point));
            SendMessageW(child, WM_TIMER, Some(WPARAM(1)), Some(LPARAM(0)));
            let _ = UpdateWindow(child);
        }
        assert!(
            shared.params.match_requested(),
            "native click must activate Match"
        );
        assert_eq!(unsafe { view.onKeyDown(' ' as char16, 7, 0) }, kResultOk);
        let _ = unsafe { view.onKeyUp(' ' as char16, 7, 0) };
        assert!(
            !shared.params.match_requested(),
            "host keyboard callback must toggle once"
        );
        unsafe {
            SendMessageW(child, WM_CHAR, Some(WPARAM(32)), Some(LPARAM(0)));
        }
        assert!(
            !shared.params.match_requested(),
            "a text commit must not activate the focused Match button"
        );

        for (point, rms) in [
            (point_for(144.0, 51.0), true),
            (point_for(144.0, 119.0), false),
        ] {
            unsafe {
                SendMessageW(child, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(point));
                SendMessageW(child, WM_LBUTTONUP, Some(WPARAM(0)), Some(point));
            }
            assert_eq!(shared.params.rms_mode(), rms);
        }
        assert!(shared.params.match_requested());
        assert_eq!(shared.params.target_db(), 0.0);

        // Exercise the real Win32 text commit path as well as host-forwarded
        // editing keys. Give GPUI a frame to install the focused input handler.
        let target_point = point_for(48.0, 184.0);
        unsafe {
            SendMessageW(child, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(target_point));
            SendMessageW(child, WM_LBUTTONUP, Some(WPARAM(0)), Some(target_point));
            SendMessageW(child, WM_TIMER, Some(WPARAM(1)), Some(LPARAM(0)));
            let _ = UpdateWindow(child);
        }
        assert_eq!(unsafe { view.onKeyDown('a' as char16, 0, 4) }, kResultOk);
        let _ = unsafe { view.onKeyUp('a' as char16, 0, 4) };
        for character in ['-', '1', '5'] {
            unsafe {
                SendMessageW(
                    child,
                    WM_CHAR,
                    Some(WPARAM(character as usize)),
                    Some(LPARAM(0)),
                );
            }
        }
        assert_eq!(unsafe { view.onKeyDown(0, 4, 0) }, kResultOk);
        let _ = unsafe { view.onKeyUp(0, 4, 0) };
        assert!((shared.params.target_db() - (-15.0)).abs() < 0.001);
        assert_eq!(unsafe { view.onKeyDown(0, 12, 0) }, kResultOk);
        let _ = unsafe { view.onKeyUp(0, 12, 0) };
        assert_eq!(unsafe { view.onKeyDown(0, 14, 1) }, kResultOk);
        let _ = unsafe { view.onKeyUp(0, 14, 1) };
        assert!((shared.params.target_db() - (-14.1)).abs() < 0.001);

        // A DAW may hide or minimize the parent without removing IPlugView.
        shared
            .params
            .set_param(crate::params::PARAM_LOCKED_GAIN_DB, -7.5);
        for hide in [SW_HIDE, SW_MINIMIZE] {
            shared.params.set_param(crate::params::PARAM_MATCH, 1.0);
            let _ = unsafe { view.onFocus(0) };
            assert!(
                shared.params.match_requested(),
                "focus loss keeps Match active"
            );
            unsafe {
                let _ = ShowWindow(parent.0, hide);
                SendMessageW(child, WM_TIMER, Some(WPARAM(1)), Some(LPARAM(0)));
            }
            assert!(
                !shared.params.match_requested(),
                "hidden parent stops Match"
            );
            assert_eq!(shared.params.locked_gain_db(), -7.5);
            unsafe {
                let _ = ShowWindow(parent.0, SW_RESTORE);
                SendMessageW(child, WM_TIMER, Some(WPARAM(1)), Some(LPARAM(0)));
            }
            assert!(
                !shared.params.match_requested(),
                "restoring must not restart Match"
            );
        }
        shared.params.set_param(crate::params::PARAM_MATCH, 1.0);

        rect.right = (312.0 * scale) as i32;
        rect.bottom = (318.0 * scale) as i32;
        assert_eq!(unsafe { view.checkSizeConstraint(&mut rect) }, kResultOk);
        assert_eq!(unsafe { view.onSize(&mut rect) }, kResultOk);
        assert_eq!(unsafe { view.removed() }, kResultOk);
        assert!(!unsafe { IsWindow(Some(child)) }.as_bool());
        assert!(
            !shared.params.match_requested(),
            "removing the view stops Match"
        );
        assert_eq!(
            unsafe { view.attached(parent.0 .0, kPlatformTypeHWND) },
            kResultOk
        );
        let reopened = unsafe { GetWindow(parent.0, GW_CHILD) }.expect("reopened child");
        assert!(unsafe { IsWindow(Some(reopened)) }.as_bool());
        assert!(
            !shared.params.match_requested(),
            "reopening keeps Match off"
        );
        assert_eq!(unsafe { view.removed() }, kResultOk);
    }

    #[test]
    fn component_handler_owner_survives_shared_state_adoption() {
        let controller = GainSnapVst3Controller::new();
        let handler_owner = Arc::clone(&controller.component_handler);
        let processor_shared = GainSnapVst3Shared::new();
        let processor_connection = InstanceConnection::new(
            InstanceConnectionRole::Processor,
            Arc::clone(&processor_shared),
        );
        let handle = processor_connection.export_shared();

        assert_eq!(
            unsafe { controller.connection.adopt_shared(handle) },
            kResultOk
        );
        assert!(Arc::ptr_eq(&handler_owner, &controller.component_handler));
        assert!(Arc::ptr_eq(
            &controller.shared().params,
            &processor_shared.params
        ));
    }
}
