use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement, PathPromptOptions, Render,
    SharedString, Window, div, px,
};
use gpui::{MouseButton, prelude::*};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::popover::Popover;
use gpui_component::{ActiveTheme, Icon, Root, Sizable as _, Size, TitleBar};

use crate::data::store::{Store, ThreadMeta};
use crate::views::chat_window::ChatWindow;

pub struct ChatApp {
    chat_window: Entity<ChatWindow>,
    pinned: bool,
    title: SharedString,
    _chat_subscription: gpui::Subscription,
    title_popover_open: bool,
    title_input: gpui::Entity<InputState>,
    _title_subscription: gpui::Subscription,
}

impl ChatApp {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        thread: Option<&ThreadMeta>,
        store: Arc<Store>,
    ) -> Self {
        let chat_window = cx.new(|cx| ChatWindow::new(window, cx, thread, store));
        let title = chat_window.read(cx).title.clone();
        let _chat_subscription = cx.observe(&chat_window, |this, chat_window, cx| {
            this.title = chat_window.read(cx).title.clone();
            cx.notify();
        });

        let title_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Thread title")
                .submit_on_enter(true)
        });
        let _title_subscription = cx.subscribe_in(
            &title_input,
            window,
            |this, _state, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.commit_title_rename(cx);
                }
            },
        );

        Self {
            chat_window,
            pinned: false,
            title,
            _chat_subscription,
            title_popover_open: false,
            title_input,
            _title_subscription,
        }
    }
}

impl Render for ChatApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.font_family.clone())
            .child(self.render_titlebar(cx))
            .child(self.chat_window.clone())
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
            .children(Root::render_sheet_layer(window, cx))
    }
}

// ── helpers ──────────────────────────────────────────────────────────────
impl ChatApp {
    fn commit_title_rename(&mut self, cx: &mut Context<Self>) {
        let new_title = self.title_input.read(cx).value().to_string();
        self.chat_window.update(cx, |cw, cx| {
            cw.rename_thread(&new_title, cx);
        });
        self.title_popover_open = false;
        cx.notify();
    }

    // ── titlebar ─────────────────────────────────────────────────────
    fn render_titlebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        TitleBar::new()
            .child(self.render_title_popover(cx))
            .child(self.render_titlebar_actions(cx))
    }

    // ── title popover ────────────────────────────────────────────────
    fn render_title_popover(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.title.clone();
        let title_input = self.title_input.clone();

        div().flex().items_center().gap_2().child(
            Popover::new("rename-thread-popover")
                .open(self.title_popover_open)
                .on_open_change(cx.listener(Self::on_title_popover_toggle))
                .trigger(
                    Button::new("thread-title-trigger")
                        .ghost()
                        .label(title)
                        .text_size(px(13.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD),
                )
                .p_3()
                .gap_2()
                .child(Input::new(&title_input).w(px(200.)))
                .child(self.render_rename_buttons(cx)),
        )
    }

    fn on_title_popover_toggle(
        &mut self,
        open: &bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.title_popover_open = *open;
        if *open {
            let current = self.chat_window.read(cx).title.to_string();
            self.title_input.update(cx, |state, cx| {
                state.set_value(current, window, cx);
            });
            let handle = self.title_input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    fn render_rename_buttons(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .gap_2()
            .justify_end()
            .child(
                Button::new("rename-cancel")
                    .with_size(Size::XSmall)
                    .label("Cancel")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.title_popover_open = false;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("rename-save")
                    .with_size(Size::XSmall)
                    .primary()
                    .label("Save")
                    .on_click(cx.listener(|this, _, _, cx| this.commit_title_rename(cx))),
            )
    }

    // ── titlebar actions ─────────────────────────────────────────────
    fn render_titlebar_actions(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let reveal = self.chat_window.clone();
        let export = self.chat_window.clone();

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .pr_2()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(self.pin_button(cx))
            .child(
                Button::new("open-workspace")
                    .with_size(gpui_component::Size::Small)
                    .ghost()
                    .icon(
                        Icon::empty()
                            .path("icons/folder.svg")
                            .text_color(gpui::rgb(0x888888)),
                    )
                    .on_click(move |_, _, cx| {
                        let cw = reveal.read(cx);
                        let dir = cw
                            .selected_workspace_id
                            .as_ref()
                            .and_then(|id| cw.workspaces.iter().find(|ws| ws.id == *id))
                            .map(|ws| PathBuf::from(&ws.path));
                        if let Some(d) = dir {
                            cx.reveal_path(&d);
                        }
                    }),
            )
            .child(self.export_html_button(cx, export))
    }

    fn export_html_button(
        &mut self,
        _cx: &mut Context<Self>,
        chat_window: Entity<ChatWindow>,
    ) -> impl IntoElement {
        Button::new("export-html")
            .with_size(gpui_component::Size::Small)
            .ghost()
            .icon(
                Icon::empty()
                    .path("icons/export.svg")
                    .text_color(gpui::rgb(0x888888)),
            )
            .on_click(move |_, _, cx| {
                let rx = cx.prompt_for_paths(PathPromptOptions {
                    files: false,
                    directories: true,
                    multiple: false,
                    prompt: Some("Choose a folder to export the session HTML".into()),
                });
                let session = chat_window.read(cx).session.clone();
                let session_file = chat_window.read(cx).session_file.clone();
                cx.spawn(async move |cx| {
                    if let Ok(Ok(Some(paths))) = rx.await
                        && let Some(dir) = paths.first()
                    {
                        let name = session_file
                            .rsplit_once('.')
                            .map(|(n, _)| format!("{n}.html"))
                            .unwrap_or_else(|| "session.html".into());
                        let out = dir.join(&name);
                        let s = out.to_string_lossy().to_string();
                        if let Some(ref sess) = session {
                            let _ = sess.update(cx, |sess, _cx| sess.export_html(&s));
                        }
                    }
                })
                .detach();
            })
    }

    fn pin_button(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let pinned = self.pinned;
        Button::new("pin")
            .with_size(gpui_component::Size::Small)
            .ghost()
            .icon(
                Icon::empty()
                    .path(if pinned {
                        "icons/unpin.svg"
                    } else {
                        "icons/pin.svg"
                    })
                    .text_color(if pinned {
                        gpui::rgb(0x4f46e5)
                    } else {
                        gpui::rgb(0x888888)
                    }),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.pinned = !this.pinned;
                crate::utils::window_helpers::set_window_level(window, this.pinned);
                cx.notify();
            }))
    }
}

/// Convenience constructor used by callers: creates a `ChatApp` wrapped in a `gpui_component::Root`.
pub fn open_chat_window(
    cx: &mut gpui::App,
    thread: Option<&ThreadMeta>,
    store: Arc<Store>,
    window_options: gpui::WindowOptions,
) -> gpui::WindowHandle<Root> {
    cx.open_window(window_options, |window, cx| {
        let app = cx.new(|cx| ChatApp::new(window, cx, thread, store));
        let focus_handle: FocusHandle = app
            .read(cx)
            .chat_window
            .read(cx)
            .chat_input
            .read(cx)
            .focus_handle(cx)
            .clone();
        window.focus(&focus_handle, cx);
        cx.new(|cx| Root::new(app, window, cx))
    })
    .expect("failed to open the chat window")
}
