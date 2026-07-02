use gpui::{Context, EventEmitter, IntoElement, Render, Window, WindowOptions, div, prelude::*, px};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_component::{ActiveTheme, Icon, IconName, Root, Sizable as _, Size, TitleBar};
use gpui_wry::WebView;
use raw_window_handle::HasWindowHandle;
use wry::WebViewBuilder;

const IMAGE_GEN_URL: &str = "https://raven-ai.one/images";

#[derive(Clone)]
pub enum MiniAppEvent {
    BackPressed,
}

/// A mini-app launcher. Currently hosts a single image-generation app.
pub struct MiniApp;

impl MiniApp {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self
    }
}

impl EventEmitter<MiniAppEvent> for MiniApp {}

impl Render for MiniApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .child(
                div()
                    .id("mini-app-header")
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .px_6()
                    .py_4()
                    .child(render_back_button(cx)),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_4()
                    .child(
                        Button::new("image-gen-app")
                            .primary()
                            .with_size(Size::Large)
                            .icon(
                                Icon::new(IconName::Palette)
                                    .with_size(Size::Large)
                                    .text_color(cx.theme().primary),
                            )
                            .label("Image Generation")
                            .on_click(cx.listener(|_this, _, _, cx| {
                                cx.spawn(async move |_, cx| {
                                    let window_options = WindowOptions {
                                        titlebar: Some(TitleBar::title_bar_options()),
                                        window_decorations: if cfg!(target_os = "macos") {
                                            None
                                        } else {
                                            Some(gpui::WindowDecorations::Client)
                                        },
                                        ..Default::default()
                                    };

                                    let _ = cx.open_window(window_options, |window, cx| {
                                        let webview = cx.new(|cx| {
                                            let builder = WebViewBuilder::new();
                                            #[cfg(any(debug_assertions, feature = "inspector"))]
                                            let builder = builder.with_devtools(true);

                                            let window_handle =
                                                window.window_handle().expect("No window handle");
                                            let webview =
                                                builder.build_as_child(&window_handle).unwrap();
                                            WebView::new(webview, window, cx)
                                        });

                                        webview.update(cx, |view, _| {
                                            view.load_url(IMAGE_GEN_URL);
                                        });

                                        let view = cx.new(|_cx| ImageGenApp { webview });
                                        cx.new(|cx| Root::new(view, window, cx))
                                    });
                                })
                                .detach();
                            })),
                    ),
            )
    }
}

fn render_back_button(cx: &mut Context<MiniApp>) -> impl IntoElement {
    Button::new("mini-app-back-button")
        .with_size(Size::Large)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(cx.theme().secondary.into())
                .foreground(cx.theme().muted_foreground.into())
                .hover(cx.theme().secondary_hover.into())
                .active(cx.theme().secondary_active.into()),
        )
        .icon(
            Icon::empty()
                .path("icons/arrow-left.svg")
                .size(px(16.))
                .text_color(cx.theme().muted_foreground),
        )
        .on_click(cx.listener(|_this, _, _, cx| {
            cx.emit(MiniAppEvent::BackPressed);
        }))
}

struct ImageGenApp {
    webview: gpui::Entity<WebView>,
}

impl Render for ImageGenApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                TitleBar::new()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .child(
                                Icon::new(IconName::Palette)
                                    .with_size(Size::Small)
                                    .text_color(cx.theme().primary),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().foreground)
                                    .child("Image Generation"),
                            ),
                    ),
            )
            .child(div().flex_1().child(self.webview.clone()))
    }
}
