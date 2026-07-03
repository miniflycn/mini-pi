use std::collections::HashMap;

use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, IntoElement, Render, ScrollHandle,
    SharedString, Window, WindowBounds, WindowDecorations, WindowOptions, div, prelude::*, px,
    rems, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::Scrollbar;
use gpui_component::select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState};
use gpui_component::switch::Switch;
use gpui_component::tab::{Tab, TabBar};
use gpui_component::{ActiveTheme, Disableable as _, Root, Sizable as _, Size, TitleBar};

use crate::config::model_config::{self, ModelInfo};
use crate::core::app::AppStore;
use crate::rpc::pi_rpc::{BridgeProvider, BridgeSettings};

/// A standalone window for managing pi agent settings. It is organized into
/// collapsible accordion sections: **API Keys** and **General**.
pub struct PiSettings {
    providers: Vec<BridgeProvider>,
    providers_loading: bool,
    provider_inputs: HashMap<String, gpui::Entity<InputState>>,
    _provider_input_subs: Vec<gpui::Subscription>,
    /// Index of the currently selected settings tab (0 = API Keys, 1 = General).
    active_tab_ix: usize,
    scroll_handle: ScrollHandle,
    window_handle: AnyWindowHandle,

    // General settings
    settings_loading: bool,
    compaction_enabled: Option<bool>,
    default_thinking_level: Option<String>,
    default_model: Option<String>,
    default_provider: Option<String>,

    thinking_dropdown: gpui::Entity<SelectState<SearchableVec<SelectThinkingItem>>>,
    model_dropdown: gpui::Entity<SelectState<SearchableVec<SelectModelItem>>>,
    provider_dropdown: gpui::Entity<SelectState<SearchableVec<SelectProviderItem>>>,
    _thinking_dropdown_sub: gpui::Subscription,
    _model_dropdown_sub: gpui::Subscription,
    _provider_dropdown_sub: gpui::Subscription,
    _global_sub: gpui::Subscription,
}

#[derive(Clone)]
struct SelectThinkingItem {
    id: String,
    name: SharedString,
}

impl SelectItem for SelectThinkingItem {
    type Value = String;

    fn title(&self) -> SharedString {
        self.name.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.id
    }
}

#[derive(Clone)]
struct SelectModelItem {
    id: String,
    name: SharedString,
}

impl SelectItem for SelectModelItem {
    type Value = String;

    fn title(&self) -> SharedString {
        self.name.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.id
    }
}

#[derive(Clone)]
struct SelectProviderItem {
    id: String,
    name: SharedString,
}

impl SelectItem for SelectProviderItem {
    type Value = String;

    fn title(&self) -> SharedString {
        self.name.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.id
    }
}

const THINKING_LEVELS: &[(&str, &str)] = &[
    ("off", "Off"),
    ("minimal", "Minimal"),
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("xhigh", "Extra High"),
];

/// Returns the thinking levels supported by the given model. If no model is
/// selected or the model does not advertise a thinking-level map, all levels
/// are returned.
fn thinking_level_items_for_model(
    models: &[ModelInfo],
    model_id: Option<&str>,
) -> Vec<SelectThinkingItem> {
    let map = model_id
        .and_then(|id| models.iter().find(|m| m.id == id))
        .and_then(|m| m.thinking_level_map.as_ref());

    THINKING_LEVELS
        .iter()
        .filter(|(id, _)| match map {
            Some(m) => !matches!(m.get(*id), Some(None)),
            None => true,
        })
        .map(|(id, label)| SelectThinkingItem {
            id: (*id).to_string(),
            name: (*label).into(),
        })
        .collect()
}

impl PiSettings {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let window_handle = window.window_handle();

        let initial_models = cx.global::<AppStore>().models.clone();
        let thinking_items = thinking_level_items_for_model(&initial_models, None);
        let thinking_dropdown =
            cx.new(|cx| SelectState::new(SearchableVec::new(thinking_items), None, window, cx));

        let initial_models = cx.global::<AppStore>().models.clone();
        let initial_model_items: Vec<SelectModelItem> = initial_models
            .iter()
            .map(|m| SelectModelItem {
                id: m.id.clone(),
                name: m.name.clone().into(),
            })
            .collect();
        let model_dropdown = cx.new(|cx| {
            SelectState::new(SearchableVec::new(initial_model_items), None, window, cx)
                .searchable(true)
        });
        let provider_dropdown =
            cx.new(|cx| SelectState::new(SearchableVec::new(Vec::new()), None, window, cx));

        let _thinking_dropdown_sub = cx.subscribe(
            &thinking_dropdown,
            |this, _dropdown, event: &SelectEvent<SearchableVec<SelectThinkingItem>>, cx| {
                if let SelectEvent::Confirm(Some(level)) = event {
                    this.set_default_thinking_level(level, cx);
                }
            },
        );

        let _model_dropdown_sub = cx.subscribe(
            &model_dropdown,
            |this, _dropdown, event: &SelectEvent<SearchableVec<SelectModelItem>>, cx| {
                if let SelectEvent::Confirm(Some(model_id)) = event {
                    this.set_default_model(model_id, cx);
                }
            },
        );

        let _provider_dropdown_sub = cx.subscribe(
            &provider_dropdown,
            |this, _dropdown, event: &SelectEvent<SearchableVec<SelectProviderItem>>, cx| {
                if let SelectEvent::Confirm(Some(provider)) = event {
                    this.set_default_provider(provider, cx);
                }
            },
        );

        let _global_sub = cx.observe_global::<AppStore>(move |this, cx| {
            this.update_model_dropdown(cx);
        });

        let mut view = Self {
            providers: Vec::new(),
            providers_loading: false,
            provider_inputs: HashMap::new(),
            _provider_input_subs: Vec::new(),
            active_tab_ix: 0,
            scroll_handle: ScrollHandle::new(),
            window_handle,
            settings_loading: false,
            compaction_enabled: None,
            default_thinking_level: None,
            default_model: None,
            default_provider: None,
            thinking_dropdown,
            model_dropdown,
            provider_dropdown,
            _thinking_dropdown_sub,
            _model_dropdown_sub,
            _provider_dropdown_sub,
            _global_sub,
        };

        view.load_providers(cx);
        view.load_settings(cx);
        view.update_model_dropdown(cx);
        view.update_thinking_dropdown(cx);
        view
    }

    fn set_active_tab(&mut self, ix: usize, _window: &mut Window, cx: &mut Context<Self>) {
        self.active_tab_ix = ix;
        cx.notify();
    }

    fn load_providers(&mut self, cx: &mut Context<Self>) {
        self.providers_loading = true;
        cx.notify();

        let bridge = cx.global::<AppStore>().pi_bridge.clone();
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = smol::unblock(move || bridge.as_ref().map(|b| b.get_providers())).await;

            let _ = weak.update(cx, |this, cx| {
                this.providers_loading = false;
                match result {
                    Some(Ok(providers)) => {
                        this.providers = providers;
                        this.update_provider_dropdown(cx);
                    }
                    Some(Err(e)) => {
                        eprintln!("[pi-settings] failed to load providers: {}", e);
                    }
                    None => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_loading = true;
        cx.notify();

        let bridge = cx.global::<AppStore>().pi_bridge.clone();
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = smol::unblock(move || bridge.as_ref().map(|b| b.get_settings())).await;

            let _ = weak.update(cx, |this, cx| {
                this.settings_loading = false;
                match result {
                    Some(Ok(settings)) => {
                        let window_handle = this.window_handle;
                        this.apply_settings(settings, window_handle, cx);
                    }
                    Some(Err(e)) => {
                        eprintln!("[pi-settings] failed to load settings: {}", e);
                    }
                    None => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_settings(
        &mut self,
        settings: BridgeSettings,
        window_handle: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        self.compaction_enabled = Some(settings.compaction_enabled);
        self.default_thinking_level = settings.default_thinking_level;
        self.default_provider = settings.default_provider.clone();

        // The SDK stores defaultModel as just the model id. Reconstruct a full
        // "provider:model" id for the dropdown, preferring the configured
        // default provider and falling back to any available match.
        let models = cx.global::<AppStore>().models.clone();
        self.default_model = model_config::resolve_full_model_id(
            &models,
            settings.default_provider.as_deref(),
            settings.default_model.as_deref(),
        );

        self.update_model_dropdown(cx);
        let _ = self.update_thinking_dropdown(cx);

        let _ = cx.update_window(window_handle, |_, window, cx| {
            let model = self.default_model.clone();
            self.model_dropdown.update(cx, |dropdown, cx| {
                if let Some(ref model) = model {
                    dropdown.set_selected_value(model, window, cx);
                } else {
                    dropdown.set_selected_index(None, window, cx);
                }
            });

            let provider = self.default_provider.clone();
            self.provider_dropdown.update(cx, |dropdown, cx| {
                if let Some(ref provider) = provider {
                    dropdown.set_selected_value(provider, window, cx);
                } else {
                    dropdown.set_selected_index(None, window, cx);
                }
            });
        });
    }

    fn update_model_dropdown(&mut self, cx: &mut Context<Self>) {
        let models = cx.global::<AppStore>().models.clone();
        let provider = self.default_provider.clone();
        let mut items: Vec<SelectModelItem> = models
            .iter()
            .filter(|m| {
                provider.as_ref().map_or(true, |p| {
                    model_config::parse_model_id(&m.id)
                        .map(|(provider, _)| provider == p)
                        .unwrap_or(false)
                })
            })
            .map(|m| SelectModelItem {
                id: m.id.clone(),
                name: m.name.clone().into(),
            })
            .collect();

        // Defensive fallback: if the selected provider has no matching models,
        // show the full model list so the control is never stuck empty.
        if provider.is_some() && items.is_empty() {
            items = models
                .iter()
                .map(|m| SelectModelItem {
                    id: m.id.clone(),
                    name: m.name.clone().into(),
                })
                .collect();
        }

        let selected = self.default_model.clone();
        let selected_valid = selected
            .as_ref()
            .map_or(false, |s| items.iter().any(|i| i.id == *s));

        // When provider filter is active but the current model isn't
        // available under it, auto-select the first model in the new list.
        let auto_select_id = if provider.is_some() && !selected_valid && !items.is_empty() {
            Some(items[0].id.clone())
        } else {
            None
        };

        let auto_select_id_for_dropdown = auto_select_id.clone();
        let _ = cx.update_window(self.window_handle, |_, window, cx| {
            self.model_dropdown.update(cx, |dropdown, cx| {
                dropdown.set_items(SearchableVec::new(items), window, cx);
                if selected_valid {
                    if let Some(ref model) = selected {
                        dropdown.set_selected_value(model, window, cx);
                    }
                } else if let Some(ref id) = auto_select_id_for_dropdown {
                    dropdown.set_selected_value(id, window, cx);
                } else {
                    dropdown.set_selected_index(None, window, cx);
                }
            });
        });

        if let Some(ref id) = auto_select_id {
            self.set_default_model(id, cx);
        }
    }

    fn update_provider_dropdown(&mut self, cx: &mut Context<Self>) {
        let items: Vec<SelectProviderItem> = self
            .providers
            .iter()
            .filter(|p| p.configured)
            .map(|p| SelectProviderItem {
                id: p.id.clone(),
                name: p.name.clone().into(),
            })
            .collect();
        let selected = self.default_provider.clone();
        let _ = cx.update_window(self.window_handle, |_, window, cx| {
            self.provider_dropdown.update(cx, |dropdown, cx| {
                dropdown.set_items(SearchableVec::new(items), window, cx);
                if let Some(ref provider) = selected {
                    dropdown.set_selected_value(provider, window, cx);
                } else {
                    dropdown.set_selected_index(None, window, cx);
                }
            });
        });
    }

    /// Updates the thinking-level dropdown to reflect the levels supported by
    /// the currently selected default model. If the current level is no longer
    /// valid, it falls back to "off" or the first available level and returns
    /// the new level so the caller can persist it when appropriate.
    fn update_thinking_dropdown(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let models = cx.global::<AppStore>().models.clone();
        let items = thinking_level_items_for_model(&models, self.default_model.as_deref());
        let valid_ids: std::collections::HashSet<String> =
            items.iter().map(|i| i.id.clone()).collect();

        // If the current thinking level is not supported by the selected model,
        // fall back to "off" or the first available level.
        let new_level = self
            .default_thinking_level
            .as_ref()
            .filter(|id| valid_ids.contains(*id))
            .cloned()
            .or_else(|| {
                items
                    .iter()
                    .find(|i| i.id == "off")
                    .or_else(|| items.first())
                    .map(|i| i.id.clone())
            });

        let level_changed = new_level != self.default_thinking_level;
        if level_changed {
            self.default_thinking_level = new_level.clone();
        }

        let selected_value = self.default_thinking_level.clone();
        let items = SearchableVec::new(items);
        let _ = cx.update_window(self.window_handle, |_, window, cx| {
            self.thinking_dropdown.update(cx, |dropdown, cx| {
                dropdown.set_items(items, window, cx);
                if let Some(ref value) = selected_value {
                    dropdown.set_selected_value(value, window, cx);
                } else {
                    dropdown.set_selected_index(None, window, cx);
                }
            });
        });

        new_level.filter(|_| level_changed)
    }

    fn set_compaction_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.compaction_enabled = Some(enabled);
        cx.notify();

        let bridge = cx.global::<AppStore>().pi_bridge.clone();
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result =
                smol::unblock(move || bridge.as_ref().map(|b| b.set_compaction_enabled(enabled)))
                    .await;

            let _ = weak.update(cx, |this, cx| {
                match result {
                    Some(Ok(())) => {}
                    Some(Err(e)) => {
                        eprintln!("[pi-settings] failed to set compaction enabled: {}", e);
                        // Reload so the switch reflects the persisted value.
                        this.load_settings(cx);
                    }
                    None => {}
                }
            });
        })
        .detach();
    }

    fn set_default_thinking_level(&mut self, level: &str, cx: &mut Context<Self>) {
        let level = level.to_string();
        self.default_thinking_level = Some(level.clone());
        cx.notify();

        let bridge = cx.global::<AppStore>().pi_bridge.clone();
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = smol::unblock(move || {
                bridge
                    .as_ref()
                    .map(|b| b.set_default_thinking_level(&level))
            })
            .await;

            let _ = weak.update(cx, |this, cx| match result {
                Some(Ok(())) => {}
                Some(Err(e)) => {
                    eprintln!("[pi-settings] failed to set default thinking level: {}", e);
                    this.load_settings(cx);
                }
                None => {}
            });
        })
        .detach();
    }

    fn set_default_model(&mut self, model_id: &str, cx: &mut Context<Self>) {
        let full_id = model_id.to_string();
        self.default_model = Some(full_id.clone());
        if let Some(level) = self.update_thinking_dropdown(cx) {
            self.set_default_thinking_level(&level, cx);
        }
        cx.notify();

        let bridge = cx.global::<AppStore>().pi_bridge.clone();
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = smol::unblock(move || {
                let model_id = model_config::parse_model_id(&full_id)
                    .map(|(_, id)| id)
                    .unwrap_or(&full_id);
                bridge.as_ref().map(|b| b.set_default_model(model_id))
            })
            .await;

            let _ = weak.update(cx, |this, cx| match result {
                Some(Ok(())) => {}
                Some(Err(e)) => {
                    eprintln!("[pi-settings] failed to set default model: {}", e);
                    this.load_settings(cx);
                }
                None => {}
            });
        })
        .detach();
    }

    fn set_default_provider(&mut self, provider: &str, cx: &mut Context<Self>) {
        let provider = provider.to_string();
        self.default_provider = Some(provider.clone());
        self.update_model_dropdown(cx);
        cx.notify();

        let bridge = cx.global::<AppStore>().pi_bridge.clone();
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result =
                smol::unblock(move || bridge.as_ref().map(|b| b.set_default_provider(&provider)))
                    .await;

            let _ = weak.update(cx, |this, cx| match result {
                Some(Ok(())) => {}
                Some(Err(e)) => {
                    eprintln!("[pi-settings] failed to set default provider: {}", e);
                    this.load_settings(cx);
                }
                None => {}
            });
        })
        .detach();
    }

    fn save_provider_key(&mut self, provider_id: &str, cx: &mut Context<Self>) {
        let bridge = cx.global::<AppStore>().pi_bridge.clone();
        let provider_id = provider_id.to_string();
        let key = self
            .provider_inputs
            .get(&provider_id)
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default();

        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result =
                smol::unblock(move || bridge.as_ref().map(|b| b.set_auth(&provider_id, &key)))
                    .await;

            let _ = weak.update(cx, |this, cx| {
                match result {
                    Some(Ok(())) => {
                        // Refresh the provider list (Configured status) and the
                        // global model list, since available models change when
                        // a provider key is added or cleared.
                        this.load_providers(cx);
                        this.reload_global_models(cx);
                    }
                    Some(Err(e)) => {
                        eprintln!("[pi-settings] failed to save provider key: {}", e);
                    }
                    None => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-fetch the available model list from the bridge and publish it into
    /// `AppStore.models`. Open chat inputs observe that global and rebuild their
    /// model dropdowns in response.
    fn reload_global_models(&self, cx: &mut Context<Self>) {
        let bridge = cx.global::<AppStore>().pi_bridge.clone();
        cx.spawn(async move |_, cx| {
            let result =
                smol::unblock(move || bridge.as_ref().map(model_config::load_models)).await;
            let _ = cx.update_global(|app: &mut AppStore, _| match result {
                Some(Ok(models)) => app.models = models,
                Some(Err(e)) => eprintln!("[pi-settings] failed to reload models: {}", e),
                None => {}
            });
        })
        .detach();
    }
}

impl Render for PiSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Ensure every provider has an input field.
        for provider in &self.providers {
            if !self.provider_inputs.contains_key(&provider.id) {
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder("API Key")
                });
                let _sub = cx.observe(&input, |_, _, cx| {
                    cx.notify();
                });
                self.provider_inputs.insert(provider.id.clone(), input);
                self._provider_input_subs.push(_sub);
            }
        }

        let theme = cx.theme().clone();

        let dialog_layer = Root::render_dialog_layer(window, cx);
        let notification_layer = Root::render_notification_layer(window, cx);
        let sheet_layer = Root::render_sheet_layer(window, cx);

        let tab_bar = render_tab_bar(self, cx);
        let body = div()
            .id("pi-settings-content")
            .flex_1()
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .flex()
            .flex_col()
            .gap_6()
            .px_6()
            .py_6()
            .child(render_tab_body(self, cx));

        div()
            .id("pi-settings")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.font_family.clone())
            .child(
                TitleBar::new().child(
                    div().flex().flex_row().items_center().px_2().child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Pi Settings"),
                    ),
                ),
            )
            .child(tab_bar)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .min_h(px(0.))
                    .child(body)
                    .child(
                        div()
                            .absolute()
                            .top(px(0.))
                            .right(px(0.))
                            .bottom(px(0.))
                            .w(px(12.))
                            .child(Scrollbar::vertical(&self.scroll_handle)),
                    ),
            )
            .children(dialog_layer)
            .children(notification_layer)
            .children(sheet_layer)
    }
}

/// Renders the tab switcher at the top of the settings window.
fn render_tab_bar(
    view: &mut PiSettings,
    cx: &mut Context<PiSettings>,
) -> impl IntoElement + 'static {
    div().w_full().px_6().pt_4().child(
        TabBar::new("pi-settings-tabs")
            .w_full()
            .segmented()
            .selected_index(view.active_tab_ix)
            .on_click(cx.listener(|this, ix: &usize, window, cx| {
                this.set_active_tab(*ix, window, cx);
            }))
            .child(Tab::new().label("API Keys"))
            .child(Tab::new().label("Agent")),
    )
}

/// Renders the body of the currently selected tab.
fn render_tab_body(view: &mut PiSettings, cx: &mut Context<PiSettings>) -> AnyElement {
    match view.active_tab_ix {
        0 => render_api_keys_body(view, cx).into_any_element(),
        1 => render_general_body(view, cx).into_any_element(),
        _ => render_api_keys_body(view, cx).into_any_element(),
    }
}

/// Body content of the **API Keys** accordion section: a short description plus
/// one row per provider.
fn render_api_keys_body(
    view: &mut PiSettings,
    cx: &mut Context<PiSettings>,
) -> impl IntoElement + 'static {
    let mut section = div().w_full().flex().flex_col().gap_2().child(
        div()
            .pb_1()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child("API keys for the model providers available to the pi agent."),
    );

    if view.providers_loading {
        section = section.child(
            div()
                .w_full()
                .px_4()
                .py_2()
                .rounded_lg()
                .bg(cx.theme().secondary)
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Loading providers..."),
                ),
        );
    } else if view.providers.is_empty() {
        section = section.child(
            div()
                .w_full()
                .px_4()
                .py_2()
                .rounded_lg()
                .bg(cx.theme().secondary)
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("No providers available."),
                ),
        );
    } else {
        for provider in &view.providers {
            let provider_id = provider.id.clone();
            let is_configured = provider.configured;
            let input_entity = view.provider_inputs.get(&provider_id).cloned();

            let mut row = div()
                .id(format!("provider-{}", provider_id))
                .w_full()
                .flex()
                .flex_col()
                .gap_2()
                .px_4()
                .py_3()
                .rounded_lg()
                .bg(cx.theme().secondary)
                .child(
                    div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .text_color(cx.theme().foreground)
                                .child(provider.name.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(if is_configured {
                                    cx.theme().success
                                } else {
                                    cx.theme().muted_foreground
                                })
                                .child(if is_configured {
                                    "Configured"
                                } else {
                                    "Not configured"
                                }),
                        ),
                );

            if let Some(input) = input_entity {
                let input_val = input.read(cx).value().to_string();
                let provider_id_for_save = provider_id.clone();
                let provider_id_for_clear = provider_id.clone();

                row = row.child(
                    div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .child(Input::new(&input).w_full().max_w_80()),
                        )
                        .child(
                            Button::new(format!("save-provider-{}", provider_id_for_save))
                                .label("Save")
                                .with_size(Size::Small)
                                .primary()
                                .disabled(input_val.is_empty())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.save_provider_key(&provider_id_for_save, cx);
                                })),
                        )
                        .when(is_configured, |this| {
                            this.child(
                                Button::new(format!("clear-provider-{}", provider_id_for_clear))
                                    .label("Clear")
                                    .with_size(Size::Small)
                                    .danger()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if let Some(input) =
                                            this.provider_inputs.get(&provider_id_for_clear)
                                        {
                                            input.update(cx, |input, cx| {
                                                input.set_value("", window, cx);
                                            });
                                        }
                                        this.save_provider_key(&provider_id_for_clear, cx);
                                    })),
                            )
                        }),
                );
            }

            section = section.child(row);
        }
    }

    section
}

/// Body content of the **General** accordion section: default agent settings.
fn render_general_body(
    view: &mut PiSettings,
    cx: &mut Context<PiSettings>,
) -> impl IntoElement + 'static {
    let mut section = div().w_full().flex().flex_col().gap_4().child(
        div()
            .pb_1()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child("Default options used when starting new pi agent sessions."),
    );

    if view.settings_loading {
        section = section.child(
            div()
                .w_full()
                .px_4()
                .py_2()
                .rounded_lg()
                .bg(cx.theme().secondary)
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Loading settings..."),
                ),
        );
        return section;
    }

    // Compaction toggle
    let compaction_enabled = view.compaction_enabled.unwrap_or(false);
    section = section.child(
        div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap_4()
            .px_4()
            .py_3()
            .rounded_lg()
            .bg(cx.theme().secondary)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child("Compaction"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Automatically compact long conversations."),
                    ),
            )
            .child(
                Switch::new("compaction-switch")
                    .checked(compaction_enabled)
                    .on_click(cx.listener(move |this, checked: &bool, _window, cx| {
                        this.set_compaction_enabled(*checked, cx);
                    })),
            ),
    );

    // Default thinking level
    section = section.child(render_settings_row(
        "Default thinking level",
        "How much reasoning the model should show by default.",
        Select::new(&view.thinking_dropdown)
            .appearance(false)
            .w_full()
            .placeholder("Select level"),
        cx,
    ));

    // Default model
    section = section.child(render_settings_row(
        "Default model",
        "The model selected when creating a new thread.",
        Select::new(&view.model_dropdown)
            .appearance(false)
            .w_full()
            .placeholder("Select model")
            .menu_width(gpui::Length::Auto)
            .menu_max_h(rems(10.)),
        cx,
    ));

    // Default provider
    section = section.child(render_settings_row(
        "Default provider",
        "The provider used when creating a new thread.",
        Select::new(&view.provider_dropdown)
            .appearance(false)
            .w_full()
            .placeholder("Select provider"),
        cx,
    ));

    section
}

/// Helper to render a labeled row with a control on the right.
fn render_settings_row(
    label: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: impl IntoElement + 'static,
    cx: &mut Context<PiSettings>,
) -> impl IntoElement + 'static {
    div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap_4()
        .px_4()
        .py_3()
        .rounded_lg()
        .bg(cx.theme().secondary)
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().foreground)
                        .child(label.into()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(description.into()),
                ),
        )
        .child(div().min_w(px(160.)).child(control))
}

pub fn open_pi_settings_window(cx: &mut App) {
    // Singleton: focus the existing window instead of opening a duplicate.
    let handle = cx.update_global::<AppStore, _>(|app, _| app.pi_settings_window);
    if let Some(handle) = handle {
        let activated = handle
            .update(cx, |_view, window, _app| {
                window.activate_window();
            })
            .is_ok();
        if activated {
            return;
        }
    }

    let width = px(560.0);
    let height = px(520.0);
    let bounds = Bounds::centered(None, size(width, height), cx);
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
            let view = cx.new(|cx| PiSettings::new(window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("failed to open the pi settings window");

    cx.update_global::<AppStore, _>(|app, _| {
        app.pi_settings_window = Some(handle.into());
    });
}
