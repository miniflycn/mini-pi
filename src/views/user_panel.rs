use gpui::{
    Action, Anchor, AppContext, BorrowAppContext, ClipboardItem, Context, EventEmitter,
    InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px, rgb,
};

use crate::auth::state::{self, AuthState};
use crate::auth::supabase;
use crate::config::app_config::{DEFAULT_DARK_THEME, DEFAULT_LIGHT_THEME, FontSizePreset};
use crate::core::actions::{About, OpenPiSettingsWindow};
use crate::core::app::{AppStore, apply_font_size};
use crate::remote::RemoteStatus;
use crate::remote::cloudflared;
use crate::remote::controller::TunnelLog;
use crate::remote::qr::qr_image_source;
use crate::sync::settings_sync;
use crate::views::auth_dialog::{AuthDialogMode, AuthDialogView};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::notification::Notification;
use gpui_component::scroll::Scrollbar;
use gpui_component::select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState};
use gpui_component::switch::Switch;
use gpui_component::theme::{Theme, ThemeRegistry};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IndexPath, Sizable as _, Size, WindowExt as _,
    popover::Popover,
};

#[derive(Clone)]
pub enum UserPanelEvent {
    BackPressed,
    AuthStateChanged,
    OpenOnboarding,
}

#[derive(Clone)]
pub enum CloudflaredDialog {
    Prompt,
    Downloading,
    Error(String),
}

struct StatusLogTooltip {
    text: SharedString,
}

impl Render for StatusLogTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(320.))
            .px_3()
            .py_2()
            .rounded_md()
            .bg(cx.theme().popover)
            .border_1()
            .border_color(cx.theme().border)
            .text_xs()
            .text_color(cx.theme().popover_foreground)
            .whitespace_normal()
            .child(self.text.clone())
    }
}

pub struct UserPanel {
    pub cloudflared_dialog: Option<CloudflaredDialog>,
    pub font_size_dropdown: gpui::Entity<SelectState<SearchableVec<FontSizePreset>>>,
    pub bearer_token_input: gpui::Entity<InputState>,
    pub token_popover_open: bool,
    pub _remote_sub: Option<gpui::Subscription>,
    pub _font_size_dropdown_sub: gpui::Subscription,
    pub scroll_handle: ScrollHandle,
}

impl SelectItem for FontSizePreset {
    type Value = FontSizePreset;

    fn title(&self) -> SharedString {
        match self {
            FontSizePreset::Small => "Small",
            FontSizePreset::Medium => "Medium",
            FontSizePreset::Large => "Large",
        }
        .into()
    }

    fn value(&self) -> &Self::Value {
        self
    }
}

impl UserPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let remote_controller = cx.global::<AppStore>().remote_controller.clone();
        let remote_sub = remote_controller.as_ref().map(|controller| {
            cx.observe(controller, |_this, _controller, cx| {
                cx.notify();
            })
        });

        let presets = vec![
            FontSizePreset::Small,
            FontSizePreset::Medium,
            FontSizePreset::Large,
        ];
        let initial_preset = cx.global::<AppStore>().config.font_size;
        let initial_index = presets
            .iter()
            .position(|p| *p == initial_preset)
            .map(|row| IndexPath::default().row(row));
        let font_size_dropdown =
            cx.new(|cx| SelectState::new(SearchableVec::new(presets), initial_index, window, cx));
        let _font_size_dropdown_sub = cx.subscribe(
            &font_size_dropdown,
            |_this, _dropdown, event: &SelectEvent<SearchableVec<FontSizePreset>>, cx| {
                if let SelectEvent::Confirm(Some(preset)) = event {
                    apply_font_size(*preset, cx);
                }
            },
        );
        let bearer_token_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Cloudflare bearer token"));

        Self {
            cloudflared_dialog: None,
            font_size_dropdown,
            bearer_token_input,
            token_popover_open: false,
            _remote_sub: remote_sub,
            _font_size_dropdown_sub,
            scroll_handle: ScrollHandle::new(),
        }
    }

    fn start_cloudflared_download(&mut self, cx: &mut Context<Self>) {
        self.cloudflared_dialog = Some(CloudflaredDialog::Downloading);
        cx.notify();

        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = smol::unblock(move || cloudflared::download_and_install()).await;
            let _ = weak.update(cx, |this, cx| {
                match result {
                    Ok(path) => {
                        let controller =
                            cx.update_global(|app: &mut AppStore, _| app.remote_controller.clone());
                        if let Some(controller) = controller {
                            let command = path.to_string_lossy().to_string();
                            controller.update(cx, |c, _cx| {
                                c.config.cloudflared.command = command;
                            });
                        }
                        this.cloudflared_dialog = None;
                        this.enable_remote_control(cx);
                    }
                    Err(e) => {
                        this.cloudflared_dialog = Some(CloudflaredDialog::Error(e));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Enables remote control if cloudflared is available.
    /// Otherwise prompts to download/install it first.
    /// Uses the current value of the bearer-token input (which may be empty).
    fn enable_remote_control(&mut self, cx: &mut Context<Self>) {
        eprintln!("[user_panel] enable_remote_control called");
        let controller = match cx.global::<AppStore>().remote_controller.clone() {
            Some(c) => c,
            None => {
                eprintln!("[user_panel] enable_remote_control: remote_controller is None");
                return;
            }
        };

        let command = controller.read(cx).config.cloudflared.command.clone();
        eprintln!("[user_panel] enable_remote_control: command={}", command);
        if let Err(e) = cloudflared::resolve_cloudflared_command(&command) {
            eprintln!(
                "[user_panel] enable_remote_control: cloudflared not resolved: {}",
                e
            );
            self.cloudflared_dialog = Some(CloudflaredDialog::Prompt);
            cx.notify();
            return;
        }

        eprintln!("[user_panel] enable_remote_control: enabling with current token");
        let token = self.bearer_token_input.read(cx).value().to_string();
        controller.update(cx, |c, cx| {
            if token.is_empty() {
                c.config.cloudflared.bearer_token = None;
            } else {
                c.config.cloudflared.bearer_token = Some(token);
            }
            c.save_config(cx);
            c.set_enabled(true, cx);
        });
        cx.notify();
    }
}

impl EventEmitter<UserPanelEvent> for UserPanel {}

impl Render for UserPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let auth = cx.global::<AppStore>().auth.clone();

        let header = div()
            .id("user-panel-header")
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .px_6()
            .py_4()
            .child(render_back_button(cx));

        let content = div()
            .id("user-panel-content")
            .flex_1()
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .flex()
            .flex_col()
            .items_center()
            .px_6()
            .py_8()
            .gap_6()
            .child(
                gpui::svg()
                    .path("icons/pi.svg")
                    .size(px(48.))
                    .text_color(cx.theme().primary),
            )
            .child(render_auth_content(self, &auth, window, cx));

        div()
            .id("user-panel")
            .flex()
            .flex_col()
            .size_full()
            .relative()
            .child(header)
            .child(content)
            .child(
                div()
                    .absolute()
                    .top(px(0.))
                    .right(px(0.))
                    .bottom(px(0.))
                    .w(px(12.))
                    .child(Scrollbar::vertical(&self.scroll_handle)),
            )
            .when(self.cloudflared_dialog.is_some(), |this| {
                this.child(render_cloudflared_dialog(self, cx))
            })
    }
}

fn render_cloudflared_dialog(
    panel: &mut UserPanel,
    cx: &mut Context<UserPanel>,
) -> impl IntoElement {
    let state = panel.cloudflared_dialog.clone().unwrap();
    let (title, body, primary_label, is_downloading, error_msg): (
        SharedString,
        SharedString,
        SharedString,
        bool,
        Option<String>,
    ) = match &state {
        CloudflaredDialog::Prompt => (
            "Cloudflared required".into(),
            "Remote control needs the cloudflared tunnel binary. Download it to the app data folder now?".into(),
            "Download & Start".into(),
            false,
            None,
        ),
        CloudflaredDialog::Downloading => (
            "Downloading cloudflared".into(),
            "Downloading and installing cloudflared...".into(),
            "Downloading...".into(),
            true,
            None,
        ),
        CloudflaredDialog::Error(e) => (
            "Download failed".into(),
            "Could not download cloudflared.".into(),
            "Retry".into(),
            false,
            Some(e.clone()),
        ),
    };

    div()
        .id("cloudflared-dialog-overlay")
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .bg(gpui::rgba(0x00000099))
        .flex()
        .items_center()
        .justify_center()
        .on_click(cx.listener(|this, _, _, cx| {
            this.cloudflared_dialog = None;
            cx.notify();
        }))
        .child(
            div()
                .id("cloudflared-dialog-card")
                .mx_8()
                .w(px(360.))
                .flex()
                .flex_col()
                .gap_4()
                .px_6()
                .py_6()
                .rounded_xl()
                .bg(cx.theme().secondary)
                .border_1()
                .border_color(cx.theme().border)
                .on_click(|_, _, cx| {
                    cx.stop_propagation();
                })
                .child(
                    div()
                        .text_xl()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(cx.theme().foreground)
                        .child(title),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(body),
                )
                .when_some(error_msg, |this, err| {
                    this.child(div().text_xs().text_color(cx.theme().danger).child(err))
                })
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_3()
                        .child(
                            div().flex_1().child(
                                Button::new("cloudflared-download-btn")
                                    .label(primary_label)
                                    .primary()
                                    .disabled(is_downloading)
                                    .w_full()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.start_cloudflared_download(cx);
                                    })),
                            ),
                        )
                        .child(
                            div().flex_1().child(
                                Button::new("cloudflared-cancel-btn")
                                    .label("Cancel")
                                    .w_full()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cloudflared_dialog = None;
                                        cx.notify();
                                    })),
                            ),
                        ),
                ),
        )
}

fn render_token_popover(panel: &mut UserPanel, cx: &mut Context<UserPanel>) -> impl IntoElement {
    let input = panel.bearer_token_input.clone();
    div()
        .id("remote-token-popover")
        .w_full()
        .flex()
        .flex_col()
        .gap_3()
        .px_4()
        .py_4()
        .rounded_lg()
        .bg(cx.theme().secondary)
        .border_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(cx.theme().foreground)
                .child("Cloudflare bearer token"),
        )
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Enter a Cloudflare API token for this tunnel session, or leave empty."),
        )
        .child(Input::new(&input).w_full())
}

fn render_remote_control_section(
    panel: &mut UserPanel,
    _window: &mut Window,
    cx: &mut Context<UserPanel>,
) -> impl IntoElement {
    let Some(controller) = cx.global::<AppStore>().remote_controller.clone() else {
        return div();
    };

    let c = controller.read(cx);
    let enabled = c.is_enabled();
    let status = c.status.clone();
    let tunnel_url = c.tunnel_url.clone();
    let tunnel_log = c.tunnel_log.clone();
    let error_message = c.error_message.clone();
    let is_starting = c.is_starting();
    let is_reconnecting = c.is_reconnecting();
    let is_busy = is_starting || is_reconnecting;

    let status_text: SharedString = match &status {
        RemoteStatus::Disabled => "Off".into(),
        RemoteStatus::Starting => "Starting...".into(),
        RemoteStatus::Running => "Connected".into(),
        RemoteStatus::Reconnecting => "Reconnecting...".into(),
        RemoteStatus::Error(e) => format!("Error: {}", e).into(),
    };
    let status_color = match &status {
        RemoteStatus::Running => cx.theme().success,
        RemoteStatus::Error(_) => cx.theme().danger,
        RemoteStatus::Reconnecting => cx.theme().warning,
        _ => cx.theme().muted_foreground,
    };

    let mut section = div()
        .w_full()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .px_2()
                .py_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child("REMOTE CONTROL"),
        )
        .child(
            div()
                .id("remote-toggle-row")
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .px_4()
                .py_2()
                .rounded_lg()
                .bg(cx.theme().secondary)
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .text_color(cx.theme().foreground)
                        .child("Enable remote control"),
                )
                .child(
                    Popover::new("remote-token-popover")
                        .open(panel.token_popover_open)
                        .on_open_change(cx.listener(|this, open, _, cx| {
                            this.token_popover_open = *open;
                            cx.notify();
                        }))
                        .anchor(Anchor::TopCenter)
                        .w(px(240.))
                        .trigger(
                            Button::new("remote-token-icon")
                                .with_size(Size::Small)
                                .ghost()
                                .icon(
                                    Icon::empty()
                                        .path("icons/exclamation.svg")
                                        .size(px(16.))
                                        .text_color(if panel.token_popover_open {
                                            cx.theme().primary
                                        } else {
                                            cx.theme().muted_foreground
                                        }),
                                ),
                        )
                        .child(render_token_popover(panel, cx)),
                )
                .child(
                    div()
                        .id("remote-toggle")
                        .w(px(44.))
                        .h(px(24.))
                        .rounded_full()
                        .bg(if enabled {
                            cx.theme().primary
                        } else {
                            cx.theme().muted
                        })
                        .when(!is_busy, |s| s.cursor_pointer())
                        .when(is_busy, |s| s.opacity(0.6))
                        .child(
                            div()
                                .id("remote-toggle-knob")
                                .size(px(20.))
                                .rounded_full()
                                .bg(rgb(0xffffff))
                                .when(enabled, |s| s.ml(px(22.)))
                                .when(!enabled, |s| s.ml(px(2.)))
                                .mt(px(2.)),
                        )
                        .when(!is_busy, |s| {
                            s.on_click(cx.listener(move |this, _, _, cx| {
                                eprintln!(
                                    "[user_panel] remote toggle clicked, is_busy={}",
                                    is_busy
                                );
                                if let Some(controller) =
                                    cx.global::<AppStore>().remote_controller.clone()
                                {
                                    let enabled = controller.read(cx).is_enabled();
                                    eprintln!("[user_panel] current enabled={}", enabled);
                                    if enabled {
                                        controller.update(cx, |c, cx| c.set_enabled(false, cx));
                                    } else {
                                        this.enable_remote_control(cx);
                                    }
                                } else {
                                    eprintln!("[user_panel] remote_controller is None");
                                }
                            }))
                        }),
                ),
        )
        .child(
            div()
                .id("remote-status-row")
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .px_4()
                .py_2()
                .rounded_lg()
                .bg(cx.theme().secondary)
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Status"),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(div().text_xs().text_color(status_color).child(status_text))
                        .when_some(tunnel_log.clone(), |this, log: TunnelLog| {
                            let icon_color = match log.level.as_str() {
                                "ERR" => cx.theme().danger,
                                "WRN" => cx.theme().warning,
                                _ => cx.theme().muted_foreground,
                            };
                            let tooltip_text = format!("[{}] {}", log.level, log.message);
                            this.child(
                                div()
                                    .id("remote-status-log-icon")
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(16.))
                                    .child(
                                        gpui::svg()
                                            .path("icons/exclamation.svg")
                                            .size(px(14.))
                                            .text_color(icon_color),
                                    )
                                    .tooltip(move |_, cx| {
                                        cx.new(|_| StatusLogTooltip {
                                            text: tooltip_text.clone().into(),
                                        })
                                        .into()
                                    }),
                            )
                        }),
                ),
        );

    if let Some(tunnel) = tunnel_url {
        let qr = qr_image_source(&tunnel);
        let tunnel_for_text_copy = tunnel.clone();
        let tunnel_for_display = tunnel.clone();
        let pi_commander_url = "https://pi.raven-ai.one/".to_string();
        let pi_commander_for_open = pi_commander_url.clone();
        section = section.child(
            div()
                .id("remote-qr-card")
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .gap_3()
                .px_4()
                .py_4()
                .rounded_lg()
                .bg(cx.theme().secondary)
                .child(
                    div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Scan with ")
                        .child(
                            Button::new("pi-commander-link")
                                .label("pi-commander")
                                .with_size(Size::Small)
                                .link()
                                .on_click(cx.listener(move |_this, _, _, cx| {
                                    cx.open_url(&pi_commander_for_open);
                                })),
                        ),
                )
                .child(div().id("remote-qr-code").when_some(qr, |this, source| {
                    this.child(gpui::img(source).size(px(160.)))
                }))
                .child(
                    Button::new("remote-tunnel-url")
                        .w_full()
                        .with_size(Size::Small)
                        .icon(
                            Icon::empty()
                                .path("icons/clipboard.svg")
                                .size(px(14.))
                                .text_color(cx.theme().muted_foreground),
                        )
                        .label(tunnel_for_display)
                        .on_click(cx.listener(move |_, _, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                tunnel_for_text_copy.clone(),
                            ));
                            window.push_notification(
                                Notification::success("URL copied to clipboard"),
                                cx,
                            );
                        })),
                ),
        );
    }

    if let Some(err) = error_message {
        section = section.child(
            div()
                .id("remote-error-card")
                .w_full()
                .flex()
                .flex_col()
                .gap_2()
                .px_4()
                .py_2()
                .rounded_lg()
                .bg(cx.theme().secondary)
                .border_1()
                .border_color(cx.theme().danger)
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Remote control failed"),
                )
                .child(div().text_xs().text_color(cx.theme().danger).child(err)),
        );
    }

    section
}

fn render_back_button(cx: &mut Context<UserPanel>) -> impl IntoElement {
    Button::new("back-button")
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
            cx.emit(UserPanelEvent::BackPressed);
        }))
}

fn render_auth_content(
    panel: &mut UserPanel,
    auth: &AuthState,
    window: &mut Window,
    cx: &mut Context<UserPanel>,
) -> impl IntoElement {
    match auth {
        AuthState::LoggedIn(user) => {
            let initials: String = user
                .email
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "?".to_string());
            let threads_count = cx
                .global::<AppStore>()
                .store
                .list_threads()
                .map(|t| t.len())
                .unwrap_or(0);
            let sync_status = cx.global::<AppStore>().sync_status.clone();
            let sync_label: SharedString = match &sync_status {
                settings_sync::SyncStatus::Idle => "Not synced".into(),
                settings_sync::SyncStatus::Syncing => "Syncing...".into(),
                settings_sync::SyncStatus::Synced => "Synced".into(),
                settings_sync::SyncStatus::Error(e) => format!("Error: {}", e).into(),
            };

            div()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .gap_6()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(80.))
                        .rounded_full()
                        .bg(rgb(0x2BCF13))
                        .border_3()
                        .border_color(rgb(0x1fa824))
                        .text_color(rgb(0xffffff))
                        .text_size(px(28.))
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(initials),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().foreground)
                                .overflow_x_hidden()
                                .child(user.email.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Authenticated"),
                        ),
                )
                .child(
                    div().w_full().flex().flex_row().gap_3().child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_1()
                            .px_4()
                            .py_2()
                            .rounded_lg()
                            .bg(cx.theme().secondary)
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(cx.theme().foreground)
                                    .child(threads_count.to_string()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Threads"),
                            ),
                    ),
                )
                .child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("SYNC"),
                        )
                        .child(sync_row("Agent Settings", &sync_label, cx))
                        .child(render_sync_button(cx)),
                )
                .child(render_remote_control_section(panel, window, cx))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("PI SETTINGS"),
                        )
                        .child(render_pi_settings_row(cx))
                        .child(render_onboarding_row(cx)),
                )
                .child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("SETTINGS"),
                        )
                        .child(render_appearance_row(window, cx))
                        .child(render_font_size_row(panel, cx))
                        .child(render_about_row(cx)),
                )
                .child(render_logout_button(cx))
        }
        _ => div()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_6()
            .child(
                Button::new("login-dialog-btn")
                    .label("Sign In")
                    .with_size(Size::Large)
                    .primary()
                    .w_full()
                    .icon(
                        Icon::empty()
                            .path("icons/login.svg")
                            .size(px(16.))
                            .text_color(rgb(0xffffff)),
                    )
                    .on_click(cx.listener(|_this, _, window, cx| {
                        AuthDialogView::open(window, &mut *cx, AuthDialogMode::Login);
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Sign in to sync your agent settings"),
            )
            .child(render_remote_control_section(panel, window, cx))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("PI SETTINGS"),
                    )
                    .child(render_pi_settings_row(cx))
                    .child(render_onboarding_row(cx)),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("SETTINGS"),
                    )
                    .child(render_appearance_row(window, cx))
                    .child(render_font_size_row(panel, cx))
                    .child(render_about_row(cx)),
            ),
    }
}

fn render_pi_settings_row(cx: &mut Context<UserPanel>) -> impl IntoElement {
    div()
        .id("settings-pi-settings")
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .px_4()
        .py_2()
        .rounded_lg()
        .bg(cx.theme().secondary)
        .cursor_pointer()
        .hover(|style| style.bg(cx.theme().secondary_hover))
        .on_click(cx.listener(|_, _, window, cx| {
            window.dispatch_action(OpenPiSettingsWindow.boxed_clone(), cx);
        }))
        .child(
            div()
                .size(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    gpui::svg()
                        .path("icons/manage.svg")
                        .size(px(18.))
                        .text_color(cx.theme().muted_foreground),
                ),
        )
        .child(
            div()
                .flex_1()
                .text_sm()
                .text_color(cx.theme().foreground)
                .child("Pi Settings"),
        )
        .child(
            gpui::svg()
                .path("icons/chevron-right.svg")
                .size(px(16.))
                .text_color(cx.theme().muted_foreground),
        )
}

fn render_onboarding_row(cx: &mut Context<UserPanel>) -> impl IntoElement {
    div()
        .id("settings-onboarding")
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .px_4()
        .py_2()
        .rounded_lg()
        .bg(cx.theme().secondary)
        .cursor_pointer()
        .hover(|style| style.bg(cx.theme().secondary_hover))
        .on_click(cx.listener(|_, _, _, cx| {
            cx.emit(UserPanelEvent::OpenOnboarding);
        }))
        .child(
            div()
                .size(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    gpui::svg()
                        .path("icons/circle-check.svg")
                        .size(px(18.))
                        .text_color(cx.theme().muted_foreground),
                ),
        )
        .child(
            div()
                .flex_1()
                .text_sm()
                .text_color(cx.theme().foreground)
                .child("Onboarding"),
        )
        .child(
            gpui::svg()
                .path("icons/chevron-right.svg")
                .size(px(16.))
                .text_color(cx.theme().muted_foreground),
        )
}

fn render_about_row(cx: &mut Context<UserPanel>) -> impl IntoElement {
    div()
        .id("settings-about")
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .px_4()
        .py_2()
        .rounded_lg()
        .bg(cx.theme().secondary)
        .cursor_pointer()
        .hover(|style| style.bg(cx.theme().secondary_hover))
        .on_click(cx.listener(|_, _, window, cx| {
            window.dispatch_action(About.boxed_clone(), cx);
        }))
        .child(
            div()
                .size(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    gpui::svg()
                        .path("icons/about.svg")
                        .size(px(18.))
                        .text_color(cx.theme().muted_foreground),
                ),
        )
        .child(
            div()
                .flex_1()
                .text_sm()
                .text_color(cx.theme().foreground)
                .child("About"),
        )
        .child(
            gpui::svg()
                .path("icons/chevron-right.svg")
                .size(px(16.))
                .text_color(cx.theme().muted_foreground),
        )
}

fn render_font_size_row(panel: &UserPanel, cx: &mut Context<UserPanel>) -> impl IntoElement {
    div()
        .id("settings-font-size")
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .px_4()
        .py_2()
        .rounded_lg()
        .bg(cx.theme().secondary)
        .child(
            div()
                .size(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    gpui::svg()
                        .path("icons/appearance.svg")
                        .size(px(18.))
                        .text_color(cx.theme().muted_foreground),
                ),
        )
        .child(
            div()
                .flex_1()
                .text_sm()
                .text_color(cx.theme().foreground)
                .child("Font Size"),
        )
        .child(
            div().w(px(100.)).child(
                Select::new(&panel.font_size_dropdown)
                    .with_size(Size::Small)
                    .appearance(false)
                    .menu_width(gpui::Length::Auto),
            ),
        )
}

fn render_logout_button(cx: &mut Context<UserPanel>) -> impl IntoElement {
    Button::new("logout-button")
        .label("Sign Out")
        .danger()
        .w_full()
        .py_5()
        .on_click(cx.listener(|_this, _, _, cx| {
            let session = cx.global::<AppStore>().session.clone();
            let store = cx.global::<AppStore>().store.clone();
            if let Some(s) = session {
                let _ = supabase::logout(&s.access_token);
            }
            let _ = state::clear_session(&store);
            cx.update_global(|app: &mut AppStore, _| {
                app.auth = AuthState::LoggedOut;
                app.session = None;
            });
            cx.emit(UserPanelEvent::AuthStateChanged);
            cx.notify();
        }))
}

fn render_sync_button(cx: &mut Context<UserPanel>) -> impl IntoElement {
    let sync_status = cx.global::<AppStore>().sync_status.clone();
    let is_syncing = sync_status == settings_sync::SyncStatus::Syncing;
    let label: SharedString = if is_syncing {
        "Syncing...".into()
    } else {
        "Sync Now".into()
    };

    Button::new("sync-button")
        .label(label)
        .w_full()
        .py_5()
        .primary()
        .disabled(is_syncing)
        .on_click(cx.listener(|_this, _, _, cx| {
            let session = cx.global::<AppStore>().session.clone();
            if let Some(s) = session {
                cx.update_global(|app: &mut AppStore, _| {
                    app.sync_status = settings_sync::SyncStatus::Syncing;
                });
                cx.notify();
                let access_token = s.access_token.clone();
                let user_id = s.user.id.clone();
                let initial_meta = cx.global::<AppStore>().sync_meta.clone();
                cx.spawn(async move |_, cx| {
                    let result = smol::unblock(move || {
                        settings_sync::sync_changes(&access_token, &user_id, initial_meta)
                    })
                    .await;
                    let _ = cx.update_global(|app: &mut AppStore, _| match result {
                        Ok(meta) => {
                            let _ = settings_sync::save_sync_meta(&app.store, &meta);
                            app.sync_meta = meta;
                            app.sync_status = settings_sync::SyncStatus::Synced;
                        }
                        Err(e) => {
                            app.sync_status = settings_sync::SyncStatus::Error(e);
                        }
                    });
                })
                .detach();
            }
        }))
}

fn sync_row(
    label: impl Into<SharedString>,
    status_label: &SharedString,
    cx: &mut Context<UserPanel>,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let status_color = if status_label.as_ref() == "Synced" {
        cx.theme().success
    } else if status_label.as_ref() == "Syncing..." {
        cx.theme().warning
    } else if status_label.starts_with("Error") {
        cx.theme().danger
    } else {
        cx.theme().muted_foreground
    };
    div()
        .id(SharedString::from(format!(
            "sync-{}",
            label.to_lowercase().replace(" ", "-")
        )))
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .px_4()
        .py_2()
        .rounded_lg()
        .bg(cx.theme().secondary)
        .child(
            div().flex_1().flex().flex_row().items_center().child(
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(label),
            ),
        )
        .child(
            div()
                .text_xs()
                .text_color(status_color)
                .child(status_label.clone()),
        )
}

fn render_appearance_row(_window: &mut Window, cx: &mut Context<UserPanel>) -> impl IntoElement {
    let is_dark = cx.theme().mode.is_dark();
    div()
        .id("settings-appearance")
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .px_4()
        .py_2()
        .rounded_lg()
        .bg(cx.theme().secondary)
        .child(
            div()
                .size(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    gpui::svg()
                        .path("icons/appearance.svg")
                        .size(px(18.))
                        .text_color(cx.theme().muted_foreground),
                ),
        )
        .child(
            div()
                .flex_1()
                .text_sm()
                .text_color(cx.theme().foreground)
                .child("Dark Mode"),
        )
        .child(
            Switch::new("dark-mode-switch")
                .checked(is_dark)
                .on_click(cx.listener(move |_this, checked: &bool, window, cx| {
                    let theme_name = if *checked {
                        SharedString::from(DEFAULT_DARK_THEME)
                    } else {
                        SharedString::from(DEFAULT_LIGHT_THEME)
                    };
                    let registry = ThemeRegistry::global(cx);
                    if let Some(theme) = registry.themes().get(&theme_name).cloned() {
                        let mode = theme.mode;
                        let name = theme.name.to_string();
                        cx.update_global(|app: &mut AppStore, _| {
                            app.config.theme = Some(name.clone());
                            if let Err(e) = app.config.save() {
                                eprintln!("[theme] failed to save theme: {}", e);
                            }
                        });
                        let global_theme = Theme::global_mut(cx);
                        if mode.is_dark() {
                            global_theme.dark_theme = theme;
                        } else {
                            global_theme.light_theme = theme;
                        }
                        Theme::change(mode, Some(window), cx);
                        cx.refresh_windows();
                    }
                })),
        )
}
