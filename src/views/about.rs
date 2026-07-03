use gpui::{App, Context, IntoElement, Render, Window, prelude::*};
use gpui_component::ActiveTheme;
use gpui_component::button::Button;

pub fn open_about_window(cx: &mut App) {
    let bounds = gpui::Bounds::centered(None, gpui::size(gpui::px(360.0), gpui::px(220.0)), cx);
    let window_options = gpui::WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
        window_min_size: Some(gpui::size(gpui::px(300.0), gpui::px(300.0))),
        titlebar: Some(gpui_component::TitleBar::title_bar_options()),
        window_decorations: if cfg!(target_os = "macos") {
            None
        } else {
            Some(gpui::WindowDecorations::Client)
        },
        ..Default::default()
    };

    cx.open_window(window_options, |_window, cx| cx.new(|_cx| AboutWindow))
        .expect("failed to open the about window");
}

pub struct AboutWindow;

impl Render for AboutWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        gpui::div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_4()
            .p_6()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.font_family.clone())
            .child(
                gpui::div()
                    .text_xl()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Mini Pi"),
            )
            .child(
                gpui::div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(format!("Version {}", env!("CARGO_PKG_VERSION"))),
            )
            .child(
                gpui::div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("A desktop GUI for the pi AI coding agent SDK."),
            )
            .child(
                Button::new("about-ok")
                    .label("OK")
                    .on_click(cx.listener(|_, _, window, _cx| window.remove_window())),
            )
    }
}
