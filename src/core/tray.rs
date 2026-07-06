use std::sync::OnceLock;
use std::time::Duration;

use gpui::{App, AsyncApp, BorrowAppContext};
use smol::Timer;
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuId, MenuItem},
};

use crate::core::actions::{CreateThread, Quit, ToggleMainWindow};
use crate::core::app::AppStore;

static MENU_IDS: OnceLock<TrayMenuIds> = OnceLock::new();

#[derive(Debug, Clone)]
pub struct TrayMenuIds {
    toggle: MenuId,
    create: MenuId,
    quit: MenuId,
}

pub struct TrayManager;

impl TrayManager {
    /// Create the status-bar / system-tray icon and start listening for events.
    pub fn init(cx: &mut App) {
        #[cfg(target_os = "linux")]
        {
            Self::init_linux_tray();
            Self::start_event_loop(cx);
            return;
        }

        match Self::create_tray() {
            Ok(tray_icon) => {
                cx.update_global(|app: &mut AppStore, _| {
                    app.tray_icon = Some(tray_icon);
                });
                eprintln!("[tray] tray icon created");
                Self::start_event_loop(cx);
            }
            Err(e) => {
                eprintln!("[tray] failed to create tray icon: {}", e);
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn init_linux_tray() {
        std::thread::spawn(|| {
            if let Err(e) = gtk::init() {
                eprintln!("[tray] failed to initialize GTK: {}", e);
                return;
            }

            // The tray icon must live on the GTK thread. Leak it so it persists
            // until the process exits (GPUI's AppStore is not accessible here).
            match Self::create_tray() {
                Ok(tray) => {
                    let _ = Box::leak(Box::new(tray));
                }
                Err(e) => eprintln!("[tray] failed to create Linux tray icon: {}", e),
            }

            gtk::glib::MainLoop::new(None, false).run();
        });
    }

    fn start_event_loop(cx: &mut App) {
        cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                Self::drain_events(cx);
                Timer::after(Duration::from_millis(50)).await;
            }
        })
        .detach();
    }

    fn create_tray() -> Result<TrayIcon, String> {
        let icon = load_tray_icon()?;
        let menu = Menu::new();
        let toggle_item = MenuItem::new("Show / Hide Mini Pi", true, None);
        let create_item = MenuItem::new("Create Thread", true, None);
        let quit_item = MenuItem::new("Quit", true, None);

        let ids = TrayMenuIds {
            toggle: toggle_item.id().clone(),
            create: create_item.id().clone(),
            quit: quit_item.id().clone(),
        };
        let _ = MENU_IDS.set(ids);

        menu.append(&toggle_item)
            .map_err(|e| format!("failed to append toggle menu item: {}", e))?;
        menu.append(&create_item)
            .map_err(|e| format!("failed to append create thread menu item: {}", e))?;
        menu.append(&quit_item)
            .map_err(|e| format!("failed to append quit menu item: {}", e))?;

        let builder = TrayIconBuilder::new()
            .with_tooltip("Mini Pi")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false);

        #[cfg(target_os = "macos")]
        let builder = builder.with_icon_as_template(true);

        builder
            .build()
            .map_err(|e| format!("failed to build tray icon: {}", e))
    }

    fn drain_events(cx: &mut AsyncApp) {
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Down,
                ..
            } = event
            {
                cx.update(|app| {
                    app.dispatch_action(&ToggleMainWindow);
                });
            }
        }

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let Some(ids) = MENU_IDS.get() else { continue };
            if event.id == ids.toggle {
                cx.update(|app| {
                    app.dispatch_action(&ToggleMainWindow);
                });
            } else if event.id == ids.create {
                cx.update(|app| {
                    app.dispatch_action(&CreateThread);
                });
            } else if event.id == ids.quit {
                cx.update(|app| {
                    app.dispatch_action(&Quit);
                });
            }
        }
    }
}

fn load_tray_icon() -> Result<Icon, String> {
    let path = crate::utils::paths::app_root()
        .join("assets")
        .join("icons")
        .join("tray_icon.png");

    let image = image::open(&path)
        .map_err(|e| format!("failed to load tray icon at {:?}: {}", path, e))?
        .into_rgba8();

    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height)
        .map_err(|e| format!("invalid tray icon: {:?}", e))
}
