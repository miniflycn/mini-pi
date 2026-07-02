fn main() {
    // Required on Windows for the gpui-wry WebView to render correctly.
    #[cfg(target_os = "windows")]
    unsafe {
        std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "true");
    }

    mini_pi::app::run();
}
