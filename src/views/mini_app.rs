use gpui::{
    Context, EventEmitter, FontWeight, IntoElement, Render, Window, WindowOptions, div, prelude::*,
    px, rgb, size,
};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_component::{ActiveTheme, Icon, Root, Sizable as _, Size, TitleBar};
use gpui_wry::WebView;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use raw_window_handle::HasWindowHandle;
use wry::WebViewBuilder;

use crate::auth::state::SupabaseSession;
use crate::core::app::AppStore;
use crate::views::auth_dialog::{AuthDialogMode, AuthDialogView};

const MINI_APP_AUTH_BASE_URL: &str = "https://raven-ai.one/auth";

#[derive(Clone)]
pub enum MiniAppEvent {
    BackPressed,
}

/// A mini-app launcher. Hosts image-generation and journal web apps.
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
                    .justify_between()
                    .px_6()
                    .py_4()
                    .child(render_back_button(cx))
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child("Mini Apps"),
                    )
                    .child(div().w(px(40.))),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_4()
                    .child(
                        div()
                            .grid()
                            .grid_cols(2)
                            .gap_6()
                            .child(render_app_icon(
                                cx,
                                "image-gen-app",
                                "icons/wand-sparkles.svg",
                                rgb(0x38C793),
                                "Image Gen",
                                "/images",
                                "Image Generation",
                                "icons/wand-sparkles.svg",
                            ))
                            .child(render_app_icon(
                                cx,
                                "journal-app",
                                "icons/notebook-pen.svg",
                                rgb(0x6366F1),
                                "Journal",
                                "/journal",
                                "Journal",
                                "icons/notebook-pen.svg",
                            )),
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

fn render_app_icon(
    cx: &mut Context<MiniApp>,
    id: impl Into<gpui::ElementId>,
    icon_path: &'static str,
    icon_bg: gpui::Rgba,
    label: impl Into<gpui::SharedString>,
    redirect_path: &'static str,
    window_title: &'static str,
    window_icon_path: &'static str,
) -> impl IntoElement {
    let label = label.into();
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap_3()
        .cursor_pointer()
        .child(
            div()
                .id(id)
                .size(px(96.))
                .rounded(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .bg(icon_bg)
                .shadow_md()
                .child(
                    Icon::empty()
                        .path(icon_path)
                        .size(px(48.))
                        .text_color(gpui::rgb(0xFFFFFF)),
                )
                .on_click(cx.listener(move |_this, _, window, cx| {
                    let auth = cx.global::<AppStore>().auth.clone();
                    if !auth.is_logged_in() {
                        AuthDialogView::open(window, &mut *cx, AuthDialogMode::Login);
                        return;
                    }
                    let Some(session) = cx.global::<AppStore>().session.clone() else {
                        AuthDialogView::open(window, &mut *cx, AuthDialogMode::Login);
                        return;
                    };
                    let url = build_auth_url(&session, redirect_path);
                    let window_icon_path = window_icon_path.to_string();

                    cx.spawn(async move |_, cx| {
                        open_mini_app_webview(cx, window_title, &window_icon_path, &url).await;
                    })
                    .detach();
                })),
        )
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(cx.theme().foreground)
                .child(label),
        )
}

fn build_auth_url(session: &SupabaseSession, redirect_path: &str) -> String {
    let access_token = utf8_percent_encode(&session.access_token, NON_ALPHANUMERIC).to_string();
    let refresh_token = utf8_percent_encode(&session.refresh_token, NON_ALPHANUMERIC).to_string();
    let redirect = utf8_percent_encode(redirect_path, NON_ALPHANUMERIC).to_string();
    format!(
        "{}?access_token={}&refresh_token={}&redirect={}",
        MINI_APP_AUTH_BASE_URL, access_token, refresh_token, redirect
    )
}

async fn open_mini_app_webview(
    cx: &mut gpui::AsyncApp,
    title: &'static str,
    icon_path: &str,
    url: &str,
) {
    let window_options = WindowOptions {
        window_min_size: Some(size(px(300.0), px(300.0))),
        titlebar: Some(TitleBar::title_bar_options()),
        window_decorations: if cfg!(target_os = "macos") {
            None
        } else {
            Some(gpui::WindowDecorations::Client)
        },
        ..Default::default()
    };

    let url = url.to_string();
    let icon_path = icon_path.to_string();
    let _ = cx.open_window(window_options, |window, cx| {
        let webview = cx.new(|cx| {
            let builder = WebViewBuilder::new();
            #[cfg(any(debug_assertions, feature = "inspector"))]
            let builder = builder.with_devtools(true);

            let window_handle = window.window_handle().expect("No window handle");
            let webview = builder.build_as_child(&window_handle).unwrap();
            WebView::new(webview, window, cx)
        });

        webview.update(cx, |view, _| {
            view.load_url(&url);
        });

        let view = cx.new(|_cx| MiniAppWebView {
            webview,
            title,
            icon_path,
        });
        cx.new(|cx| Root::new(view, window, cx))
    });
}

struct MiniAppWebView {
    webview: gpui::Entity<WebView>,
    title: &'static str,
    icon_path: String,
}

impl Render for MiniAppWebView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                TitleBar::new().child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .child(
                            Icon::empty()
                                .path(&self.icon_path)
                                .with_size(Size::Small)
                                .text_color(cx.theme().primary),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().foreground)
                                .child(self.title),
                        ),
                ),
            )
            .child(div().flex_1().child(self.webview.clone()))
    }
}
