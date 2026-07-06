use gpui::Window;
#[cfg(target_os = "macos")]
use objc::{msg_send, sel, sel_impl};

/// Set the platform window level so the chat window can be pinned on top.
#[cfg(target_os = "macos")]
pub fn set_window_level(window: &Window, pinned: bool) {
    use objc::runtime::Object;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    const NSFLOATING_WINDOW_LEVEL: isize = 3;
    const NSNORMAL_WINDOW_LEVEL: isize = 0;

    if let Ok(handle) = HasWindowHandle::window_handle(window)
        && let RawWindowHandle::AppKit(appkit) = handle.as_raw()
    {
        let ns_view = appkit.ns_view.as_ptr() as *mut Object;
        #[allow(unexpected_cfgs)]
        unsafe {
            let ns_window: *mut Object = msg_send![ns_view, window];
            let level = if pinned {
                NSFLOATING_WINDOW_LEVEL
            } else {
                NSNORMAL_WINDOW_LEVEL
            };
            let () = msg_send![ns_window, setLevel: level];
        }
    }
}

#[cfg(target_os = "windows")]
pub fn set_window_level(window: &Window, pinned: bool) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    type HWND = *mut std::ffi::c_void;
    const HWND_TOPMOST: HWND = -1isize as HWND;
    const HWND_NOTOPMOST: HWND = -2isize as HWND;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_SHOWWINDOW: u32 = 0x0040;

    unsafe extern "system" {
        fn SetWindowPos(
            hwnd: HWND,
            hwnd_insert_after: HWND,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            u_flags: u32,
        ) -> i32;
    }

    if let Ok(handle) = HasWindowHandle::window_handle(window) {
        if let RawWindowHandle::Win32(win32) = handle.as_raw() {
            let hwnd = win32.hwnd.get() as *mut std::ffi::c_void;
            unsafe {
                SetWindowPos(
                    hwnd,
                    if pinned { HWND_TOPMOST } else { HWND_NOTOPMOST },
                    0,
                    0,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOMOVE | SWP_SHOWWINDOW,
                );
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub fn set_window_level(_window: &Window, pinned: bool) {
    // Best-effort X11 support via wmctrl. Wayland has no standard always-on-top protocol.
    let _ = std::process::Command::new("wmctrl")
        .args([
            "-r",
            ":ACTIVE:",
            "-b",
            if pinned { "add,above" } else { "remove,above" },
        ])
        .spawn();
}

/// Hide the window from the screen (and taskbar on Windows).
pub fn hide_window(window: &Window) {
    #[cfg(target_os = "windows")]
    hide_window_windows(window);
    #[cfg(target_os = "macos")]
    hide_window_macos(window);
    #[cfg(target_os = "linux")]
    window.minimize_window();
}

#[cfg(target_os = "windows")]
fn hide_window_windows(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    type HWND = *mut std::ffi::c_void;
    const SW_HIDE: i32 = 0;

    unsafe extern "system" {
        fn ShowWindow(hwnd: HWND, n_cmd_show: i32) -> i32;
    }

    if let Ok(handle) = HasWindowHandle::window_handle(window) {
        if let RawWindowHandle::Win32(win32) = handle.as_raw() {
            let hwnd = win32.hwnd.get() as HWND;
            unsafe {
                ShowWindow(hwnd, SW_HIDE);
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn hide_window_macos(window: &Window) {
    use objc::{msg_send, sel, sel_impl};
    use objc::runtime::Object;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    if let Ok(handle) = HasWindowHandle::window_handle(window)
        && let RawWindowHandle::AppKit(appkit) = handle.as_raw()
    {
        let ns_view = appkit.ns_view.as_ptr() as *mut Object;
        unsafe {
            let ns_window: *mut Object = msg_send![ns_view, window];
            let _: () = msg_send![ns_window, orderOut: nil];
        }
    }
}

/// Show the window and bring it to the foreground.
pub fn show_and_activate_window(window: &Window) {
    #[cfg(target_os = "windows")]
    show_and_activate_window_windows(window);
    #[cfg(target_os = "macos")]
    show_and_activate_window_macos(window);
    #[cfg(target_os = "linux")]
    window.activate_window();
}

#[cfg(target_os = "windows")]
fn show_and_activate_window_windows(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    type HWND = *mut std::ffi::c_void;
    const SW_SHOW: i32 = 5;

    unsafe extern "system" {
        fn ShowWindow(hwnd: HWND, n_cmd_show: i32) -> i32;
        fn SetForegroundWindow(hwnd: HWND) -> i32;
    }

    if let Ok(handle) = HasWindowHandle::window_handle(window) {
        if let RawWindowHandle::Win32(win32) = handle.as_raw() {
            let hwnd = win32.hwnd.get() as HWND;
            unsafe {
                ShowWindow(hwnd, SW_SHOW);
                SetForegroundWindow(hwnd);
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn show_and_activate_window_macos(window: &Window) {
    use objc::{msg_send, sel, sel_impl};
    use objc::runtime::Object;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    if let Ok(handle) = HasWindowHandle::window_handle(window)
        && let RawWindowHandle::AppKit(appkit) = handle.as_raw()
    {
        let ns_view = appkit.ns_view.as_ptr() as *mut Object;
        unsafe {
            let ns_window: *mut Object = msg_send![ns_view, window];
            let _: () = msg_send![ns_window, makeKeyAndOrderFront: nil];
        }
    }
    window.activate_window();
}
