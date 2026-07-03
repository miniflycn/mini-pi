use gpui::{
    AnyElement, AnyWindowHandle, AppContext as _, Context, InteractiveElement, IntoElement,
    ParentElement, Styled, Window, div,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::notification::Notification;
use gpui_component::select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState};
use gpui_component::tab::{Tab, TabBar};
use gpui_component::{ActiveTheme as _, Disableable as _, Sizable as _, Size, WindowExt as _};

use crate::auth::state;
use crate::config::model_config;
use crate::core::app::AppStore;
use crate::rpc::pi_rpc::BridgeProvider;

pub struct OnboardingPanel {
    files: Vec<(String, std::path::PathBuf)>,
    providers: Vec<BridgeProvider>,
    loading_providers: bool,
    saving: bool,
    pi_dir_exists: bool,
    api_key_input: gpui::Entity<InputState>,
    provider_dropdown: gpui::Entity<SelectState<SearchableVec<SelectProviderItem>>>,
    window_handle: AnyWindowHandle,
    active_tab: usize,
    _provider_dropdown_sub: gpui::Subscription,
    _input_sub: gpui::Subscription,
}

#[derive(Clone)]
struct SelectProviderItem {
    id: String,
    name: gpui::SharedString,
}

impl SelectItem for SelectProviderItem {
    type Value = String;

    fn title(&self) -> gpui::SharedString {
        self.name.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.id
    }
}

impl OnboardingPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let window_handle = window.window_handle();

        let api_key_input = cx.new(|cx| InputState::new(window, cx).placeholder("API key"));

        let provider_dropdown =
            cx.new(|cx| SelectState::new(SearchableVec::new(Vec::new()), None, window, cx));

        let _input_sub = cx.observe(&api_key_input, |_, _, cx| {
            cx.notify();
        });

        let _provider_dropdown_sub = cx.subscribe(
            &provider_dropdown,
            |_this, _dropdown, _event: &SelectEvent<SearchableVec<SelectProviderItem>>, cx| {
                cx.notify();
            },
        );

        let files = state::list_pi_agent_json_files();
        let pi_dir_exists = state::pi_dir_exists();
        let active_tab = if pi_dir_exists && !files.is_empty() {
            1
        } else {
            0
        };

        let mut panel = Self {
            files,
            providers: Vec::new(),
            loading_providers: false,
            saving: false,
            pi_dir_exists,
            api_key_input,
            provider_dropdown,
            window_handle,
            active_tab,
            _provider_dropdown_sub,
            _input_sub,
        };

        panel.load_providers(cx);
        panel
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.files = state::list_pi_agent_json_files();
        self.pi_dir_exists = state::pi_dir_exists();
        self.saving = false;
        self.active_tab = if self.pi_dir_exists && !self.files.is_empty() {
            1
        } else {
            0
        };
        if self.providers.is_empty() && !self.loading_providers {
            self.load_providers(cx);
        }
    }

    pub fn has_files(&self) -> bool {
        !self.files.is_empty()
    }

    pub fn run_import(&mut self) -> Result<usize, String> {
        state::import_from_pi_agent().map_err(|e| e.to_string())
    }

    fn load_providers(&mut self, cx: &mut Context<Self>) {
        let Some(bridge) = cx.global::<AppStore>().pi_bridge.clone() else {
            return;
        };

        self.loading_providers = true;
        cx.notify();

        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = smol::unblock(move || bridge.get_providers()).await;
            let _ = weak.update(cx, |this, cx| {
                this.loading_providers = false;
                match result {
                    Ok(providers) => {
                        this.providers = providers;
                        this.update_provider_dropdown(cx);
                    }
                    Err(e) => {
                        eprintln!("[onboarding] failed to load providers: {}", e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn update_provider_dropdown(&mut self, cx: &mut Context<Self>) {
        let items: Vec<SelectProviderItem> = self
            .providers
            .iter()
            .map(|p| SelectProviderItem {
                id: p.id.clone(),
                name: p.name.clone().into(),
            })
            .collect();

        let _ = cx.update_window(self.window_handle, |_, window, cx| {
            self.provider_dropdown.update(cx, |dropdown, cx| {
                dropdown.set_items(SearchableVec::new(items), window, cx);
            });
        });
    }

    fn selected_provider(&self, cx: &gpui::App) -> Option<String> {
        self.provider_dropdown.read(cx).selected_value().cloned()
    }

    fn api_key(&self, cx: &gpui::App) -> String {
        self.api_key_input.read(cx).value().to_string()
    }

    fn can_save(&self, cx: &gpui::App) -> bool {
        !self.saving && self.selected_provider(cx).is_some() && !self.api_key(cx).is_empty()
    }

    fn save_setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bridge) = cx.global::<AppStore>().pi_bridge.clone() else {
            window.push_notification(
                Notification::error("Pi bridge is not available. Please try again later."),
                cx,
            );
            return;
        };

        let provider_id = match self.selected_provider(cx) {
            Some(id) => id,
            None => return,
        };
        let key = self.api_key(cx);
        if key.is_empty() {
            return;
        }

        self.saving = true;
        cx.notify();

        let window_handle = self.window_handle;
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = smol::unblock(move || {
                bridge.set_auth(&provider_id, &key)?;
                bridge.set_default_provider(&provider_id)?;
                Ok::<_, crate::rpc::pi_rpc::PiRpcError>(())
            })
            .await;

            let _ = weak.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok(()) => {
                        this.reload_global_models(cx);
                        let _ = cx.update_window(window_handle, |_, window, cx| {
                            window.close_dialog(cx);
                            window.push_notification(
                                Notification::success("Setup complete. You're ready to chat!"),
                                cx,
                            );
                        });
                    }
                    Err(e) => {
                        eprintln!("[onboarding] failed to save setup: {}", e);
                        let _ = cx.update_window(window_handle, |_, window, cx| {
                            window.push_notification(
                                Notification::error(format!("Setup failed: {}", e)),
                                cx,
                            );
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn reload_global_models(&self, cx: &mut Context<Self>) {
        let Some(bridge) = cx.global::<AppStore>().pi_bridge.clone() else {
            return;
        };
        cx.spawn(async move |_, cx| {
            let result = smol::unblock(move || model_config::load_models(&bridge)).await;
            let _ = cx.update_global(|app: &mut AppStore, _| match result {
                Ok(models) => app.models = models,
                Err(e) => eprintln!("[onboarding] failed to reload models: {}", e),
            });
        })
        .detach();
    }

    fn bridge_available(&self, cx: &gpui::App) -> bool {
        cx.global::<AppStore>().pi_bridge.is_some()
    }

    fn set_active_tab(&mut self, ix: usize, _window: &mut Window, cx: &mut Context<Self>) {
        self.active_tab = ix;
        cx.notify();
    }

    pub fn render_dialog_content(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let mut panel = div().id("onboarding-panel").flex().flex_col().size_full();

        if self.pi_dir_exists {
            panel = panel.child(self.render_tab_bar(cx));
        }

        panel
            .child(div().flex_1().child(self.render_tab_body(window, cx)))
            .into_any_element()
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + 'static {
        TabBar::new("onboarding-tabs")
            .w_full()
            .segmented()
            .selected_index(self.active_tab)
            .on_click(cx.listener(|this, ix: &usize, window, cx| {
                this.set_active_tab(*ix, window, cx);
            }))
            .child(Tab::new().label("Set Up Provider"))
            .child(Tab::new().label("Import from Pi"))
    }

    fn render_tab_body(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.active_tab {
            0 => self.render_setup_tab(window, cx).into_any_element(),
            1 => self.render_import_tab(window, cx).into_any_element(),
            _ => self.render_setup_tab(window, cx).into_any_element(),
        }
    }

    fn render_setup_tab(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + 'static {
        let bridge_available = self.bridge_available(cx);
        let provider_count = self.providers.len();
        let can_save = self.can_save(cx);

        div()
            .id("onboarding-setup-tab")
            .px_5()
            .py_5()
            .flex()
            .flex_col()
            .items_stretch()
            .gap_4()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Choose your AI provider and enter your API key. Your key is stored only on this device."),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_stretch()
                    .gap_3()
                    .child(self.render_labeled_row(
                        "Provider",
                        Select::new(&self.provider_dropdown)
                            .w_full()
                            .placeholder(if !bridge_available {
                                "Bridge unavailable"
                            } else if self.loading_providers {
                                "Loading providers..."
                            } else if provider_count == 0 {
                                "No providers"
                            } else {
                                "Select provider"
                            })
                            .disabled(!bridge_available || self.loading_providers || provider_count == 0),
                        cx,
                    ))
                    .child(self.render_labeled_row(
                        "API Key",
                        div()
                            .w_full()
                            .child(Input::new(&self.api_key_input).w_full()),
                        cx,
                    )),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(
                        Button::new("onboarding-save-btn")
                            .label(if self.saving { "Saving..." } else { "Save" })
                            .primary()
                            .with_size(Size::Small)
                            .flex_1()
                            .disabled(!can_save)
                            .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                this.save_setup(window, cx);
                            })),
                    )
                    .child(
                        Button::new("onboarding-skip-btn")
                            .label("Close")
                            .with_size(Size::Small)
                            .flex_1()
                            .disabled(self.saving)
                            .on_click(cx.listener(|_this: &mut Self, _, window, cx| {
                                window.close_dialog(cx);
                            })),
                    ),
            )
    }

    fn render_import_tab(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + 'static {
        if self.files.is_empty() {
            return div()
                .id("onboarding-import-tab")
                .px_5()
                .py_5()
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    div()
                        .w_full()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("No Pi settings detected at ~/.pi/agent/. Use the \"Set Up Provider\" tab to configure Mini Pi."),
                )
                .child(div().flex_1())
                .child(
                    Button::new("onboarding-import-close-btn")
                        .label("Close")
                        .with_size(Size::Small)
                        .w_full()
                        .on_click(cx.listener(|_this: &mut Self, _, window, cx| {
                            window.close_dialog(cx);
                        })),
                );
        }

        let file_names: String = self
            .files
            .iter()
            .map(|(name, _)| format!("  • {}", name))
            .collect::<Vec<_>>()
            .join("\n");

        div()
            .id("onboarding-import-tab")
            .px_5()
            .py_5()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .w_full()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "Detected settings from ~/.pi/agent/.\n\nFound {} JSON file(s):\n{}",
                        self.files.len(),
                        file_names
                    )),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(
                        div().flex_1().child(
                            Button::new("onboarding-import-btn")
                                .label("Import")
                                .primary()
                                .with_size(Size::Small)
                                .w_full()
                                .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                    let result = this.run_import();
                                    match result {
                                        Ok(count) => {
                                            window.close_dialog(cx);
                                            window.push_notification(
                                                Notification::success(format!(
                                                    "Imported {} file(s) successfully",
                                                    count
                                                )),
                                                cx,
                                            );
                                        }
                                        Err(e) => window.push_notification(
                                            Notification::error(format!("Import failed: {}", e)),
                                            cx,
                                        ),
                                    }
                                })),
                        ),
                    )
                    .child(
                        div().flex_1().child(
                            Button::new("onboarding-import-close-btn")
                                .label("Close")
                                .with_size(Size::Small)
                                .w_full()
                                .on_click(cx.listener(|_this: &mut Self, _, window, cx| {
                                    window.close_dialog(cx);
                                })),
                        ),
                    ),
            )
    }

    fn render_labeled_row(
        &self,
        label: &str,
        control: impl IntoElement + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + 'static {
        div()
            .flex()
            .flex_col()
            .items_stretch()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string()),
            )
            .child(control)
    }
}
