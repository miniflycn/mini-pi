use gpui::{
    AnyElement, App, Context, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    SharedString, Styled, Window, div, prelude::*, px,
};

use crate::app::trigger_sync;
use crate::auth::state::{self, AuthState};
use crate::auth::supabase;
use crate::core::app::AppStore;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::notification::Notification;
use gpui_component::{ActiveTheme as _, Disableable as _, Sizable as _, Size, WindowExt as _};

#[derive(Clone)]
pub enum AuthDialogEvent {
    Authenticated,
    Cancelled,
}

#[derive(Clone, Copy, PartialEq)]
pub enum AuthDialogMode {
    Login,
    Signup,
}

pub struct AuthDialogView {
    pub mode: AuthDialogMode,
    pub email_input: gpui::Entity<InputState>,
    pub password_input: gpui::Entity<InputState>,
    pub confirm_password_input: gpui::Entity<InputState>,
    pub auth_error: Option<String>,
    pub _email_sub: gpui::Subscription,
    pub _password_sub: gpui::Subscription,
    pub _confirm_password_sub: gpui::Subscription,
}

impl AuthDialogView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>, mode: AuthDialogMode) -> Self {
        let email_input = cx.new(|cx| InputState::new(window, cx).placeholder("Email"));
        let password_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Password")
                .masked(true)
        });
        let confirm_password_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Confirm Password")
                .masked(true)
        });

        let _email_sub = cx.observe(&email_input, |_, _, cx| {
            cx.notify();
        });
        let _password_sub = cx.observe(&password_input, |_, _, cx| {
            cx.notify();
        });
        let _confirm_password_sub = cx.observe(&confirm_password_input, |_, _, cx| {
            cx.notify();
        });

        Self {
            mode,
            email_input,
            password_input,
            confirm_password_input,
            auth_error: None,
            _email_sub,
            _password_sub,
            _confirm_password_sub,
        }
    }

    /// Open the auth dialog as a modal on the given window.
    pub fn open(window: &mut Window, cx: &mut App, mode: AuthDialogMode) {
        let view = cx.new(|cx| Self::new(window, cx, mode));
        let title = match mode {
            AuthDialogMode::Login => "Sign In",
            AuthDialogMode::Signup => "Create Account",
        };
        let subtitle: SharedString = match mode {
            AuthDialogMode::Login => "Sign in to sync your agent settings across devices".into(),
            AuthDialogMode::Signup => "Sign up to sync your agent settings across devices".into(),
        };

        window.open_dialog(cx, move |dialog, _, _| {
            let view_for_content = view.clone();
            let view_for_ok = view.clone();
            let subtitle_for_content = subtitle.clone();
            dialog
                .overlay(true)
                .overlay_closable(true)
                .close_button(true)
                .keyboard(true)
                .w(px(400.))
                .on_ok(move |_, window, cx| {
                    view_for_ok.update(cx, |view, _cx| {
                        view.submit(window, _cx);
                    });
                    false
                })
                .content(move |content, window, cx| {
                    view_for_content.update(cx, |view, cx| {
                        content.child(view.render_dialog_content(window, cx, title, subtitle_for_content.clone()))
                    })
                })
        });
    }

    pub fn render_dialog_content(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
        title: &'static str,
        subtitle: SharedString,
    ) -> AnyElement {
        let auth = cx.global::<AppStore>().auth.clone();
        let is_logging_in = matches!(auth, AuthState::LoggingIn);
        let error_msg: Option<SharedString> = self.auth_error.clone().map(|s| s.into());
        let mode = self.mode;

        let form_fields = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_3()
            .child(render_email_field(self, cx))
            .child(render_password_field(self, cx))
            .when(mode == AuthDialogMode::Signup, |el: gpui::Div| {
                el.child(render_confirm_password_field(self, cx))
            })
            .when_some(error_msg, |this, err| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(err),
                )
            })
            .when(mode == AuthDialogMode::Login, |el: gpui::Div| {
                el.child(render_login_button(is_logging_in, cx))
            })
            .when(mode == AuthDialogMode::Signup, |el: gpui::Div| {
                el.child(render_signup_button(is_logging_in, cx))
            });

        div()
            .id("auth-dialog-content")
            .flex()
            .flex_col()
            .gap_4()
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
                    .child(subtitle),
            )
            .child(form_fields)
            .child(
                Button::new("auth-dialog-cancel-btn")
                    .label("Cancel")
                    .with_size(Size::Large)
                    .w_full()
                    .on_click(cx.listener(|_this, _, window, cx| {
                        window.close_dialog(cx);
                    })),
            )
            .when(!is_logging_in && mode == AuthDialogMode::Login, |el| {
                el.child(
                    div()
                        .id("switch-to-signup")
                        .w_full()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .child(
                            Button::new("switch-to-signup")
                                .label("Create Account")
                                .with_size(Size::Small)
                                .link()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.auth_error = None;
                                    this.mode = AuthDialogMode::Signup;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when(!is_logging_in && mode == AuthDialogMode::Signup, |el| {
                el.child(
                    div()
                        .id("switch-to-login")
                        .w_full()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .child(
                            Button::new("switch-to-login")
                                .label("Sign In")
                                .with_size(Size::Small)
                                .link()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.auth_error = None;
                                    this.mode = AuthDialogMode::Login;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .into_any_element()
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(cx.global::<AppStore>().auth, AuthState::LoggingIn) {
            return;
        }
        match self.mode {
            AuthDialogMode::Login => self.login(window, cx),
            AuthDialogMode::Signup => self.signup(window, cx),
        }
    }

    fn login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.auth_error = None;
        let email = self.email_input.read(cx).value().to_string();
        let password = self.password_input.read(cx).value().to_string();
        if email.is_empty() || password.is_empty() {
            self.auth_error = Some("Email and password are required".to_string());
            cx.notify();
            return;
        }

        cx.update_global(|app: &mut AppStore, _| {
            app.auth = AuthState::LoggingIn;
        });
        cx.notify();

        let store = cx.global::<AppStore>().store.clone();
        let window_handle = window.window_handle();
        cx.spawn(async move |weak, cx| {
            let result = smol::unblock(move || supabase::login(&email, &password)).await;
            let _ = weak.update(cx, |this, cx| {
                match result {
                    Ok(session) => {
                        let _ = state::save_session(&store, &session);
                        let user = session.user.clone();
                        let access_token = session.access_token.clone();
                        let user_id = session.user.id.clone();
                        let sync_meta = cx.global::<AppStore>().sync_meta.clone();
                        cx.update_global(|app: &mut AppStore, _| {
                            app.auth = AuthState::LoggedIn(user);
                            app.session = Some(session);
                        });
                        trigger_sync(access_token, user_id, sync_meta, cx);
                        let _ = window_handle.update(cx, |_, window, cx| {
                            window.close_dialog(cx);
                        });
                        this.auth_error = None;
                        cx.emit(AuthDialogEvent::Authenticated);
                        cx.refresh_windows();
                    }
                    Err(e) => {
                        this.auth_error = Some(e.to_string());
                        cx.update_global(|app: &mut AppStore, _| {
                            app.auth = AuthState::LoggedOut;
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn signup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.auth_error = None;
        let email = self.email_input.read(cx).value().to_string();
        let password = self.password_input.read(cx).value().to_string();
        let confirm = self.confirm_password_input.read(cx).value().to_string();
        if email.is_empty() || password.is_empty() {
            self.auth_error = Some("Email and password are required".to_string());
            cx.notify();
            return;
        }
        if password != confirm {
            self.auth_error = Some("Passwords do not match".to_string());
            cx.notify();
            return;
        }

        cx.update_global(|app: &mut AppStore, _| {
            app.auth = AuthState::LoggingIn;
        });
        cx.notify();

        let store = cx.global::<AppStore>().store.clone();
        let window_handle = window.window_handle();
        cx.spawn(async move |weak, cx| {
            let result = smol::unblock(move || supabase::signup(&email, &password)).await;
            let _ = weak.update(cx, |this, cx| {
                match result {
                    Ok(session) => {
                        let _ = state::save_session(&store, &session);
                        let user = session.user.clone();
                        let access_token = session.access_token.clone();
                        let user_id = session.user.id.clone();
                        let sync_meta = cx.global::<AppStore>().sync_meta.clone();
                        cx.update_global(|app: &mut AppStore, _| {
                            app.auth = AuthState::LoggedIn(user);
                            app.session = Some(session);
                        });
                        trigger_sync(access_token, user_id, sync_meta, cx);
                        let _ = window_handle.update(cx, |_, window, cx| {
                            window.close_dialog(cx);
                        });
                        this.auth_error = None;
                        cx.emit(AuthDialogEvent::Authenticated);
                        cx.refresh_windows();
                    }
                    Err(e) => {
                        const CONFIRM_MSG: &str =
                            "Please check your email to confirm your account, then sign in.";
                        if let supabase::SupabaseAuthError::Api { msg, status: 200 } = &e
                            && msg.as_str() == CONFIRM_MSG
                        {
                            this.auth_error = None;
                            this.mode = AuthDialogMode::Login;
                            let _ = window_handle.update(cx, |_, window, cx| {
                                window.push_notification(
                                    Notification::success(
                                        "Confirmation email sent. Please sign in.",
                                    ),
                                    cx,
                                );
                            });
                            cx.update_global(|app: &mut AppStore, _| {
                                app.auth = AuthState::LoggedOut;
                            });
                            cx.notify();
                            return;
                        }
                        this.auth_error = Some(e.to_string());
                        cx.update_global(|app: &mut AppStore, _| {
                            app.auth = AuthState::LoggedOut;
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl EventEmitter<AuthDialogEvent> for AuthDialogView {}

impl Render for AuthDialogView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // This view is intended to be rendered inside a dialog via render_dialog_content.
        div()
    }
}

fn render_email_field(view: &AuthDialogView, cx: &mut Context<AuthDialogView>) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child("EMAIL"),
        )
        .child(
            div()
                .w_full()
                .px_3()
                .py_2()
                .rounded_lg()
                .bg(cx.theme().secondary)
                .border_1()
                .border_color(cx.theme().border)
                .child(Input::new(&view.email_input).appearance(false).w_full()),
        )
}

fn render_password_field(
    view: &AuthDialogView,
    cx: &mut Context<AuthDialogView>,
) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child("PASSWORD"),
        )
        .child(
            div()
                .w_full()
                .px_3()
                .py_2()
                .rounded_lg()
                .bg(cx.theme().secondary)
                .border_1()
                .border_color(cx.theme().border)
                .child(Input::new(&view.password_input).appearance(false).w_full()),
        )
}

fn render_confirm_password_field(
    view: &AuthDialogView,
    cx: &mut Context<AuthDialogView>,
) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child("CONFIRM PASSWORD"),
        )
        .child(
            div()
                .w_full()
                .px_3()
                .py_2()
                .rounded_lg()
                .bg(cx.theme().secondary)
                .border_1()
                .border_color(cx.theme().border)
                .child(
                    Input::new(&view.confirm_password_input)
                        .appearance(false)
                        .w_full(),
                ),
        )
}

fn render_login_button(
    is_logging_in: bool,
    cx: &mut Context<AuthDialogView>,
) -> impl IntoElement {
    Button::new("login-button")
        .label("Sign In")
        .with_size(Size::Large)
        .primary()
        .disabled(is_logging_in)
        .w_full()
        .on_click(cx.listener(|this, _, window, cx| {
            this.submit(window, cx);
        }))
}

fn render_signup_button(
    is_logging_in: bool,
    cx: &mut Context<AuthDialogView>,
) -> impl IntoElement {
    Button::new("signup-submit-button")
        .label("Create Account")
        .with_size(Size::Large)
        .primary()
        .loading(is_logging_in)
        .disabled(is_logging_in)
        .w_full()
        .on_click(cx.listener(|this, _, window, cx| {
            this.submit(window, cx);
        }))
}
