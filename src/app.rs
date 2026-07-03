use std::sync::Arc;

use gpui::{
    App, Application, Bounds, KeyBinding, Menu, MenuItem, MouseButton, SharedString, Window,
    WindowBounds, WindowDecorations, WindowOptions, prelude::*, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::tab::{Tab, TabBar};
use gpui_component::theme::{Theme, ThemeRegistry};
use gpui_component::{ActiveTheme, Icon, Root, Sizable, TitleBar};

use crate::auth::state::{self, AuthState};
use crate::config::app_config::{AppConfig, DEFAULT_DARK_THEME, FontSizePreset};
use crate::config::model_config;
use crate::core::actions::{
    About, Login, OpenInstallExtensionWindow, OpenPiSettingsWindow, Quit, SelectFontLarge,
    SelectFontMedium, SelectFontSmall, ShowMainWindow, SignUp,
};
use crate::core::app::{AppStore, MainOverlay};
use crate::core::app::apply_font_size;
use crate::core::assets::Assets;

use crate::data::store::Store;
use crate::remote::RemoteController;
use crate::rpc::pi_rpc::PiBridge;
use crate::sync::settings_sync;
use crate::views::about::open_about_window;
use crate::views::auth_dialog::{AuthDialogMode, AuthDialogView};
use crate::views::install_extension::open_install_extension_window;
use crate::views::mini_app::{MiniApp, MiniAppEvent};
use crate::views::pi_settings::open_pi_settings_window;
use crate::views::skills_panel::SkillsPanel;
use crate::views::thread_list::ThreadList;
use crate::views::user_panel::{UserPanel, UserPanelEvent};

pub fn run() {
    let store = Arc::new(Store::open().expect("failed to open database"));
    let mut config = AppConfig::load();
    // Remote control is always off on app startup.
    config.remote_control.enabled = false;
    if let Err(e) = config.save() {
        eprintln!("[remote] failed to save startup config: {}", e);
    }
    let assets_dir = crate::utils::paths::app_root().join("assets");

    let (auth, session) = match state::load_session(&store) {
        Some(session) => {
            if session.is_expired() {
                match crate::auth::supabase::refresh_session(&session.refresh_token) {
                    Ok(new_session) => {
                        let user = new_session.user.clone();
                        let _ = state::save_session(&store, &new_session);
                        (AuthState::LoggedIn(user), Some(new_session))
                    }
                    Err(_) => {
                        let _ = state::clear_session(&store);
                        (AuthState::LoggedOut, None)
                    }
                }
            } else {
                match crate::auth::supabase::get_user(&session.access_token) {
                    Ok(user) => (AuthState::LoggedIn(user), Some(session)),
                    Err(_) => {
                        let _ = state::clear_session(&store);
                        (AuthState::LoggedOut, None)
                    }
                }
            }
        }
        None => (AuthState::LoggedOut, None),
    };

    let sync_meta = settings_sync::load_sync_meta(&store);
    let initial_sync_meta = sync_meta.clone();

    let pi_bridge = match PiBridge::spawn() {
        Ok(bridge) => {
            eprintln!("[mini-pi] pi SDK bridge connected");
            Some(bridge)
        }
        Err(e) => {
            eprintln!("[mini-pi] failed to start pi SDK bridge: {}", e);
            None
        }
    };

    Application::with_platform(gpui_platform::current_platform(false))
        .with_assets(Assets { base: assets_dir })
        .run(move |cx: &mut App| {
            gpui_component::init(cx);

            let themes_dir = crate::utils::paths::app_root()
                .join("assets")
                .join("themes");
            let initial_theme_name = config
                .theme
                .clone()
                .unwrap_or_else(|| DEFAULT_DARK_THEME.to_string());
            if let Err(err) = ThemeRegistry::watch_dir(themes_dir, cx, move |cx| {
                let theme_name = SharedString::from(initial_theme_name.clone());
                if let Some(theme) = ThemeRegistry::global(cx).themes().get(&theme_name).cloned() {
                    let mode = theme.mode;
                    let global_theme = Theme::global_mut(cx);
                    if mode.is_dark() {
                        global_theme.dark_theme = theme;
                    } else {
                        global_theme.light_theme = theme;
                    }
                    Theme::change(mode, None, cx);
                    cx.refresh_windows();
                }
            }) {
                eprintln!("[theme] failed to watch themes directory: {}", err);
            }
            // Other theme settings
            Theme::global_mut(cx).notification.placement = gpui::Anchor::TopCenter;
            Theme::global_mut(cx).font_size = config.font_size.to_px();

            let models = pi_bridge
                .as_ref()
                .map(|bridge| match model_config::load_models(bridge) {
                    Ok(models) => models,
                    Err(e) => {
                        eprintln!("[mini-pi] failed to load model list: {}", e);
                        Vec::new()
                    }
                })
                .unwrap_or_default();
            eprintln!("[mini-pi] loaded {} models", models.len());
            for m in &models {
                eprintln!(
                    "[mini-pi]   model: id={} name={} thinking_level_map={:?}",
                    m.id, m.name, m.thinking_level_map
                );
            }

            let remote_controller =
                cx.new(|cx| RemoteController::new(cx, config.remote_control.clone()));

            cx.set_global(AppStore::new(
                store.clone(),
                config.clone(),
                auth.clone(),
                session.clone(),
                sync_meta,
                pi_bridge.clone(),
                Some(remote_controller),
                models,
            ));

            if auth.is_logged_in() {
                if let Some(ref sess) = session {
                    let _ = state::agent_dir();
                    trigger_sync(
                        sess.access_token.clone(),
                        sess.user.id.clone(),
                        initial_sync_meta.clone(),
                        cx,
                    );
                }
            }

            cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
            cx.on_action(|_: &ShowMainWindow, cx: &mut App| {
                let handle = cx.update_global::<AppStore, _>(|app, _| app.main_window);
                let needs_new_window = match handle {
                    Some(handle) => handle
                        .update(cx, |_view, window, _app| {
                            window.activate_window();
                        })
                        .is_err(),
                    None => true,
                };
                if needs_new_window {
                    open_main_window(cx);
                }
            });
            cx.on_action(|_: &About, cx: &mut App| {
                open_about_window(cx);
            });
            cx.on_action(|_: &OpenInstallExtensionWindow, cx: &mut App| {
                open_install_extension_window(cx);
            });
            cx.on_action(|_: &OpenPiSettingsWindow, cx: &mut App| {
                open_pi_settings_window(cx);
            });
            cx.on_action(|_: &Login, cx: &mut App| {
                if let Some(window) = cx.active_window() {
                    let _ = cx.update_window(window, |_, window, cx| {
                        AuthDialogView::open(window, cx, AuthDialogMode::Login);
                    });
                }
            });
            cx.on_action(|_: &SignUp, cx: &mut App| {
                if let Some(window) = cx.active_window() {
                    let _ = cx.update_window(window, |_, window, cx| {
                        AuthDialogView::open(window, cx, AuthDialogMode::Signup);
                    });
                }
            });
            cx.on_action(|_: &SelectFontSmall, cx: &mut App| {
                apply_font_size(FontSizePreset::Small, cx);
            });
            cx.on_action(|_: &SelectFontMedium, cx: &mut App| {
                apply_font_size(FontSizePreset::Medium, cx);
            });
            cx.on_action(|_: &SelectFontLarge, cx: &mut App| {
                apply_font_size(FontSizePreset::Large, cx);
            });
            let mut key_bindings = vec![
                KeyBinding::new("cmd-w", crate::core::actions::CloseWindow, None),
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("enter", crate::core::actions::SendMessage, None),
            ];
            if !cfg!(target_os = "macos") {
                key_bindings.push(KeyBinding::new(
                    "ctrl-w",
                    crate::core::actions::CloseWindow,
                    None,
                ));
            }
            cx.bind_keys(key_bindings);

            #[allow(unused_mut)]
            let mut menus = vec![
                Menu {
                    name: "Mini Pi".into(),
                    items: vec![
                        MenuItem::action("About Mini Pi", About),
                        MenuItem::separator(),
                        MenuItem::action("Quit", Quit),
                    ],
                    disabled: false,
                },
                Menu {
                    name: "Window".into(),
                    items: vec![MenuItem::action("Show Main Window", ShowMainWindow)],
                    disabled: false,
                },
            ];

            #[cfg(target_os = "macos")]
            menus.push(Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Small Font", SelectFontSmall),
                    MenuItem::action("Medium Font", SelectFontMedium),
                    MenuItem::action("Large Font", SelectFontLarge),
                ],
                disabled: false,
            });

            cx.set_menus(menus);

            cx.on_window_closed(|cx: &mut App, _window_id| {
                if !cfg!(target_os = "macos") && cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            open_main_window(cx);
            cx.activate(true);
        });
}

fn open_main_window(cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(420.0), px(600.0)), cx);
    let window_options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(300.0), px(300.0))),
        titlebar: Some(TitleBar::title_bar_options()),
        window_decorations: if cfg!(target_os = "macos") {
            None
        } else {
            Some(WindowDecorations::Client)
        },
        ..Default::default()
    };

    let handle = cx
        .open_window(window_options, |window, cx| {
            let app = cx.new(|cx| MiniPiApp::new(window, cx));
            let focus_handle = app.read(cx).thread_list.read(cx).focus_handle.clone();
            window.focus(&focus_handle, cx);
            cx.new(|cx| Root::new(app, window, cx))
        })
        .expect("failed to open the Mini Pi window");

    cx.update_global::<AppStore, _>(|app, _| {
        app.main_window = Some(handle.into());
    });
}

/// Trigger a background agent-config sync against Supabase Storage and
/// reflect the result in `AppStore::sync_status`. Used both at startup
/// (when a valid session is restored) and reactively when the user logs in
/// via the `UserPanel`. Both codepaths previously inlined this logic.
pub(crate) fn trigger_sync<C>(
    access_token: String,
    user_id: String,
    initial_meta: settings_sync::SyncMeta,
    cx: &mut C,
) where
    C: std::borrow::BorrowMut<gpui::App>,
{
    cx.update_global(|app: &mut AppStore, _| {
        app.sync_status = settings_sync::SyncStatus::Syncing;
    });
    cx.borrow_mut()
        .spawn(async move |cx: &mut gpui::AsyncApp| {
            let result = smol::unblock(move || {
                settings_sync::sync_changes(&access_token, &user_id, initial_meta)
            })
            .await;
            let _ = cx.update_global(|app: &mut AppStore, _| {
                app.sync_status = match result {
                    Ok(meta) => {
                        let _ = settings_sync::save_sync_meta(&app.store, &meta);
                        app.sync_meta = meta;
                        settings_sync::SyncStatus::Synced
                    }
                    Err(e) => settings_sync::SyncStatus::Error(e),
                };
            });
        })
        .detach();
}

/// Tabs in the main Mini Pi window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum MiniPiTab {
    #[default]
    Threads,
    Skills,
}

impl MiniPiTab {
    fn from_index(index: usize) -> Self {
        match index {
            1 => MiniPiTab::Skills,
            _ => MiniPiTab::Threads,
        }
    }
}

struct MiniPiApp {
    thread_list: gpui::Entity<ThreadList>,
    user_panel: gpui::Entity<UserPanel>,
    mini_app: gpui::Entity<MiniApp>,
    skills_panel: gpui::Entity<SkillsPanel>,
    active_tab_index: usize,
    pinned: bool,
    _user_panel_subscription: gpui::Subscription,
    _mini_app_subscription: gpui::Subscription,
}

impl MiniPiApp {
    fn new(window: &mut Window, cx: &mut gpui::Context<Self>) -> Self {
        let store = cx.global::<AppStore>().store.clone();
        let thread_list = cx.new(|cx| ThreadList::new(window, cx, store));
        let user_panel = cx.new(|cx| UserPanel::new(window, cx));
        let mini_app = cx.new(|cx| MiniApp::new(window, cx));
        let skills_panel = cx.new(|cx| SkillsPanel::new(window, cx));

        let _user_panel_subscription =
            cx.subscribe(&user_panel, move |this, _, event: &UserPanelEvent, cx| {
                let mut reset_panel = true;
                match event {
                    UserPanelEvent::AuthStateChanged => {
                        let auth = cx.global::<AppStore>().auth.clone();
                        if let AuthState::LoggedIn(_) = &auth {
                            let session = cx.global::<AppStore>().session.clone();
                            if let Some(s) = session {
                                let initial_meta = cx.global::<AppStore>().sync_meta.clone();
                                trigger_sync(
                                    s.access_token.clone(),
                                    s.user.id.clone(),
                                    initial_meta,
                                    cx,
                                );
                            }
                        }
                    }
                    UserPanelEvent::BackPressed => {}
                    UserPanelEvent::OpenOnboarding => {
                        reset_panel = false;
                        let handle = cx.update_global::<AppStore, _>(|app, _| app.main_window);
                        if let Some(handle) = handle {
                            let thread_list = this.thread_list.clone();
                            let _ = cx.update_window(handle, |_, window, cx| {
                                thread_list.update(cx, |thread_list, cx| {
                                    thread_list.open_onboarding(window, cx);
                                });
                            });
                        }
                    }
                }
                if reset_panel {
                    this.active_tab_index = 0;
                    cx.update_global(|app: &mut AppStore, _| {
                        app.main_overlay = MainOverlay::None;
                    });
                }
                cx.notify();
            });

        let _mini_app_subscription =
            cx.subscribe(&mini_app, move |_this, _, event: &MiniAppEvent, cx| {
                match event {
                    MiniAppEvent::BackPressed => {
                        cx.update_global(|app: &mut AppStore, _| {
                            app.main_overlay = MainOverlay::None;
                        });
                    }
                }
                cx.notify();
            });

        Self {
            thread_list,
            user_panel,
            mini_app,
            skills_panel,
            active_tab_index: 0,
            pinned: false,
            _user_panel_subscription,
            _mini_app_subscription,
        }
    }
}

impl gpui::Render for MiniPiApp {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        let active_tab_index = self.active_tab_index;
        let main_overlay = cx.global::<AppStore>().main_overlay;

        let dialog_layer = Root::render_dialog_layer(window, cx);
        let notification_layer = Root::render_notification_layer(window, cx);
        let sheet_layer = Root::render_sheet_layer(window, cx);

        gpui::div()
            .flex()
            .flex_col()
            .size_full()
            .relative()
            .child(
                TitleBar::new()
                    .child(
                        TabBar::new("app-tabs")
                            .mt(px(1.))
                            .segmented()
                            .px_0()
                            .py(px(2.))
                            .bg(cx.theme().title_bar)
                            .flex_1()
                            .selected_index(active_tab_index)
                            .on_click(cx.listener(|this, ix: &usize, window, cx| {
                                this.set_active_tab(*ix, window, cx);
                            }))
                            .child(Tab::new().label("Mini Pi"))
                            .child(Tab::new().label("Skills")),
                    )
                    .child(
                        gpui::div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .px_2()
                            .gap_2()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(self.pin_button(cx))
                            .child(Self::mini_app_button(cx))
                            .child(Self::user_menu_button(cx)),
                    ),
            )
            .child(
                gpui::div()
                    .id("tab-content")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_hidden()
                    .map(|this| {
                        match main_overlay {
                            MainOverlay::UserPanel => this.child(self.user_panel.clone()),
                            MainOverlay::MiniApp => this.child(self.mini_app.clone()),
                            MainOverlay::None => match MiniPiTab::from_index(active_tab_index) {
                                MiniPiTab::Threads => this.child(self.thread_list.clone()),
                                MiniPiTab::Skills => this.child(self.skills_panel.clone()),
                            },
                        }
                    }),
            )
            .children(dialog_layer)
            .children(notification_layer)
            .children(sheet_layer)
    }
}

impl MiniPiApp {
    fn pin_button(&mut self, cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
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
                crate::views::title_bar::set_window_level(window, this.pinned);
                cx.notify();
            }))
    }

    fn mini_app_button(cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        let active = cx.global::<AppStore>().main_overlay == MainOverlay::MiniApp;
        Button::new("mini-app")
            .with_size(gpui_component::Size::Small)
            .ghost()
            .icon(
                Icon::empty()
                    .path("icons/layout-grid.svg")
                    .text_color(if active {
                        gpui::rgb(0x4f46e5)
                    } else {
                        gpui::rgb(0x888888)
                    }),
            )
            .on_click(cx.listener(|_this, _, _, cx| {
                cx.update_global(|app: &mut AppStore, _| {
                    app.main_overlay = if app.main_overlay == MainOverlay::MiniApp {
                        MainOverlay::None
                    } else {
                        MainOverlay::MiniApp
                    };
                });
                cx.notify();
            }))
    }

    fn user_menu_button(cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        let active = cx.global::<AppStore>().main_overlay == MainOverlay::UserPanel;
        Button::new("user-menu")
            .with_size(gpui_component::Size::Small)
            .ghost()
            .icon(
                Icon::empty()
                    .path("icons/account.svg")
                    .text_color(if active {
                        gpui::rgb(0x4f46e5)
                    } else {
                        gpui::rgb(0x888888)
                    }),
            )
            .on_click(cx.listener(|_this, _, _, cx| {
                cx.update_global(|app: &mut AppStore, _| {
                    app.main_overlay = if app.main_overlay == MainOverlay::UserPanel {
                        MainOverlay::None
                    } else {
                        MainOverlay::UserPanel
                    };
                });
                cx.notify();
            }))
    }

    fn set_active_tab(&mut self, index: usize, _window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.active_tab_index = index;
        cx.update_global(|app: &mut AppStore, _| {
            app.main_overlay = MainOverlay::None;
        });
        if let MiniPiTab::Skills = MiniPiTab::from_index(index) {
            self.skills_panel.update(cx, |panel, cx| {
                panel.load_if_needed(cx);
            });
        }
        cx.notify();
    }
}
