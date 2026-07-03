use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use gpui::{
    AnyWindowHandle, BorrowAppContext, Bounds, Entity, Global, TitlebarOptions,
    WindowBackgroundAppearance, WindowBounds, WindowOptions, point, px, size,
};

use crate::auth::state::{AuthState, SupabaseSession};
use crate::config::app_config::{AppConfig, FontSizePreset};
use crate::config::model_config::ModelInfo;
use crate::core::session_manager::SessionManager;
use crate::data::store::Store;
use crate::remote::RemoteController;
use crate::rpc::pi_rpc::PiBridge;
use crate::sync::settings_sync::{SyncMeta, SyncStatus};
use gpui_component::theme::Theme;

/// Which overlay, if any, is currently shown in the main window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MainOverlay {
    #[default]
    None,
    UserPanel,
    MiniApp,
}

pub struct AppStore {
    pub store: Arc<Store>,
    pub config: AppConfig,
    pub thread_windows: HashMap<String, AnyWindowHandle>,
    pub main_window: Option<AnyWindowHandle>,
    pub pi_settings_window: Option<AnyWindowHandle>,
    pub auth: AuthState,
    pub session: Option<SupabaseSession>,
    pub sync_meta: SyncMeta,
    pub sync_status: SyncStatus,
    pub main_overlay: MainOverlay,
    pub pi_bridge: Option<Arc<PiBridge>>,
    pub session_manager: SessionManager,
    streaming_thread_ids: HashSet<String>,
    pub remote_controller: Option<Entity<RemoteController>>,
    pub models: Vec<ModelInfo>,
}

impl AppStore {
    pub fn new(
        store: Arc<Store>,
        config: AppConfig,
        auth: AuthState,
        session: Option<SupabaseSession>,
        sync_meta: SyncMeta,
        pi_bridge: Option<Arc<PiBridge>>,
        remote_controller: Option<Entity<RemoteController>>,
        models: Vec<ModelInfo>,
    ) -> Self {
        Self {
            store,
            config,
            thread_windows: HashMap::new(),
            main_window: None,
            pi_settings_window: None,
            auth,
            session,
            sync_meta,
            sync_status: SyncStatus::Idle,
            main_overlay: MainOverlay::None,
            pi_bridge,
            session_manager: SessionManager::new(),
            streaming_thread_ids: HashSet::new(),
            remote_controller,
            models,
        }
    }

    pub fn set_thread_streaming(&mut self, thread_id: String, streaming: bool) {
        if streaming {
            self.streaming_thread_ids.insert(thread_id);
        } else {
            self.streaming_thread_ids.remove(&thread_id);
        }
    }

    pub fn is_thread_streaming(&self, thread_id: &str) -> bool {
        self.streaming_thread_ids.contains(thread_id)
    }

    pub fn is_thread_window_open(&self, thread_id: &str) -> bool {
        self.thread_windows.contains_key(thread_id)
    }
}

impl Global for AppStore {}

pub fn custom_window_options(bounds: Option<Bounds<gpui::Pixels>>) -> WindowOptions {
    WindowOptions {
        window_bounds: bounds.map(WindowBounds::Windowed),
        window_min_size: Some(size(px(300.0), px(300.0))),
        titlebar: Some(TitlebarOptions {
            title: None,
            appears_transparent: true,
            traffic_light_position: Some(point(px(9.0), px(9.0))),
        }),
        #[cfg(target_os = "linux")]
        window_background: gpui::WindowBackgroundAppearance::Transparent,
        #[cfg(target_os = "linux")]
        window_decorations: Some(gpui::WindowDecorations::Client),
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    }
}

pub fn apply_font_size(preset: FontSizePreset, cx: &mut gpui::App) {
    Theme::global_mut(cx).font_size = preset.to_px();
    cx.update_global(|app: &mut AppStore, _| {
        app.config.font_size = preset;
        if let Err(e) = app.config.save() {
            eprintln!("[font-size] failed to save config: {}", e);
        }
    });
    cx.refresh_windows();
}
