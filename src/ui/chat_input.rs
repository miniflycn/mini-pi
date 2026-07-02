use std::cell::Cell;
use std::ops::Range;
use std::path::PathBuf;

use gpui::{
    AnyWindowHandle, AppContext, ClipboardEntry, Context, Entity, EventEmitter, FocusHandle,
    Focusable, ImageFormat, IntoElement, ParentElement, PathPromptOptions, Render, ScrollHandle,
    SharedString, Styled, Subscription, Window, div, prelude::*, px, rems, svg,
};

use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_component::input::{
    Enter, IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp, Paste, RopeExt as _,
};
use gpui_component::notification::Notification;
use gpui_component::select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IndexPath, Sizable as _, Size, WindowExt as _,
};

use crate::config::model_config::{ModelInfo, all_models};
use crate::core::app::AppStore;
use crate::data::models::ChatState;
use crate::utils::file_scanner;
use crate::utils::voice::{VoiceRecorder, VoiceState, start_recording, transcribe};

#[derive(Clone, Debug)]
pub struct MentionItem {
    pub name: String,
    pub relative_path: String,
    pub absolute_path: PathBuf,
    pub is_dir: bool,
}

#[derive(Clone, Debug)]
pub struct CommandItem {
    pub name: String,
    pub description: Option<String>,
    pub source: String,
}

#[derive(Clone, Debug)]
pub struct AtMentionParse {
    pub query: String,
    pub replace_range: Range<usize>,
}

#[derive(Clone, Debug)]
pub enum ChatInputEvent {
    Change,
    /// Send button clicked. Enter-to-send is handled by the global
    /// `SendMessage` action registered on ChatWindow; this is for the toolbar
    /// button only.
    Submit,
    /// Stop button clicked.
    Stop,
    ModelChanged(String),
    ThinkingChanged(String),
}

#[derive(Clone)]
pub struct SelectModelItem {
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

#[derive(Clone, Debug)]
pub enum PendingAttachment {
    Image {
        path: PathBuf,
        name: String,
        mime_type: String,
        base64: String,
    },
    Text {
        path: PathBuf,
        name: String,
        content: String,
    },
}

impl PendingAttachment {
    pub fn name(&self) -> &str {
        match self {
            PendingAttachment::Image { name, .. } | PendingAttachment::Text { name, .. } => name,
        }
    }

    pub fn path(&self) -> &std::path::Path {
        match self {
            PendingAttachment::Image { path, .. } | PendingAttachment::Text { path, .. } => path,
        }
    }
}

pub struct ChatInput {
    pub focus_handle: FocusHandle,
    pub input_state: Entity<InputState>,
    _input_subscription: Subscription,

    pub enable_at_mention: bool,
    at_mention_active: bool,
    at_mention_query: String,
    at_mention_replace_range: Range<usize>,
    pub at_mention_highlighted: usize,
    pub mention_items: Vec<MentionItem>,
    file_cache: Vec<MentionItem>,
    file_cache_loaded: bool,
    file_cache_loading: bool,
    pub workspace_dir: Option<PathBuf>,
    pub workspace_name: Option<String>,
    pub just_selected_mention: bool,
    cached_workspace_id: Option<String>,

    pub enable_slash_commands: bool,
    slash_command_active: bool,
    slash_command_query: String,
    slash_command_replace_range: Range<usize>,
    pub slash_command_highlighted: usize,
    pub slash_command_items: Vec<CommandItem>,
    available_commands: Vec<CommandItem>,

    // --- Composer state ---
    composer_enabled: bool,
    window_handle: AnyWindowHandle,
    at_mention_scroll_handle: ScrollHandle,
    command_scroll_handle: ScrollHandle,
    // Set when keyboard navigation moves the highlight so the next render
    // scrolls the highlighted item into view. Cleared after scrolling so that
    // mouse-wheel scrolling is not snapped back to the highlighted item.
    at_mention_scroll_pending: Cell<bool>,
    command_scroll_pending: Cell<bool>,
    pending_attachments: Vec<PendingAttachment>,
    voice_state: VoiceState,
    voice_recorder: Option<VoiceRecorder>,
    model_dropdown: Option<Entity<SelectState<SearchableVec<SelectModelItem>>>>,
    thinking_dropdown: Option<Entity<SelectState<SearchableVec<SelectModelItem>>>>,
    selected_model: Option<String>,
    thinking_level: Option<String>,
    chat_state: ChatState,
    _model_dropdown_subscription: Option<Subscription>,
    _thinking_dropdown_subscription: Option<Subscription>,
}

impl EventEmitter<ChatInputEvent> for ChatInput {}

impl ChatInput {
    /// Create a minimal chat input (no composer toolbar). Used for inline
    /// editing where only the text input itself is rendered.
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        placeholder: impl Into<SharedString>,
    ) -> Self {
        Self::build(window, cx, placeholder, false)
    }

    /// Create a full composer chat input with toolbar, model/thinking
    /// dropdowns, attachment handling, voice input, and popup autocomplete.
    pub fn new_composer(
        window: &mut Window,
        cx: &mut Context<Self>,
        placeholder: impl Into<SharedString>,
        models: &[ModelInfo],
        selected_model: Option<String>,
        selected_thinking_level: Option<String>,
    ) -> Self {
        let mut this = Self::build(window, cx, placeholder, true);
        this.selected_model = selected_model.clone();
        this.thinking_level = selected_thinking_level.clone();

        // Build model dropdown items
        let model_items: Vec<SelectModelItem> = all_models(models)
            .iter()
            .map(|m| SelectModelItem {
                id: m.id.clone(),
                name: m.name.clone().into(),
            })
            .collect();
        let model_selected_index = selected_model
            .as_ref()
            .and_then(|id| model_items.iter().position(|m| &m.id == id))
            .map(|row| IndexPath::default().row(row));
        let model_dropdown = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(model_items),
                model_selected_index,
                window,
                cx,
            )
            .searchable(true)
        });

        // Build thinking level dropdown items based on the selected model's map
        let thinking_items =
            Self::thinking_level_items_for_model(models, selected_model.as_deref());
        let thinking_selected_index = selected_thinking_level
            .as_ref()
            .and_then(|id| thinking_items.iter().position(|m| &m.id == id))
            .map(|row| IndexPath::default().row(row));
        let thinking_dropdown = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(thinking_items),
                thinking_selected_index,
                window,
                cx,
            )
        });

        this.model_dropdown = Some(model_dropdown.clone());
        this.thinking_dropdown = Some(thinking_dropdown.clone());

        // Subscribe to model dropdown selection events
        this._model_dropdown_subscription = Some(cx.subscribe(
            &model_dropdown,
            |this, _dropdown, event: &SelectEvent<SearchableVec<SelectModelItem>>, cx| {
                if let SelectEvent::Confirm(Some(id)) = event {
                    this.selected_model = Some(id.clone());
                    this.refresh_thinking_dropdown(cx);
                    cx.emit(ChatInputEvent::ModelChanged(id.clone()));
                }
            },
        ));

        // Subscribe to thinking dropdown selection events
        this._thinking_dropdown_subscription = Some(cx.subscribe(
            &thinking_dropdown,
            |this, _dropdown, event: &SelectEvent<SearchableVec<SelectModelItem>>, cx| {
                if let SelectEvent::Confirm(Some(id)) = event {
                    this.thinking_level = Some(id.clone());
                    cx.emit(ChatInputEvent::ThinkingChanged(id.clone()));
                }
            },
        ));

        this
    }

    fn build(
        window: &mut Window,
        cx: &mut Context<Self>,
        placeholder: impl Into<SharedString>,
        composer_enabled: bool,
    ) -> Self {
        let placeholder = placeholder.into();
        let input_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .auto_grow(1, 8)
                .submit_on_enter(true)
        });

        let input_subscription = cx.subscribe_in(
            &input_state,
            window,
            |this, _state, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.update_popups(cx);
                    cx.emit(ChatInputEvent::Change);
                    cx.notify();
                }
            },
        );

        Self {
            focus_handle: input_state.focus_handle(cx),
            input_state,
            _input_subscription: input_subscription,
            enable_at_mention: true,
            at_mention_active: false,
            at_mention_query: String::new(),
            at_mention_replace_range: 0..0,
            at_mention_highlighted: 0,
            mention_items: Vec::new(),
            file_cache: Vec::new(),
            file_cache_loaded: false,
            file_cache_loading: false,
            workspace_dir: None,
            workspace_name: None,
            just_selected_mention: false,
            cached_workspace_id: None,
            enable_slash_commands: true,
            slash_command_active: false,
            slash_command_query: String::new(),
            slash_command_replace_range: 0..0,
            slash_command_highlighted: 0,
            slash_command_items: Vec::new(),
            available_commands: Vec::new(),
            composer_enabled,
            window_handle: window.window_handle(),
            at_mention_scroll_handle: ScrollHandle::new(),
            command_scroll_handle: ScrollHandle::new(),
            at_mention_scroll_pending: Cell::new(false),
            command_scroll_pending: Cell::new(false),
            pending_attachments: Vec::new(),
            voice_state: VoiceState::Idle,
            voice_recorder: None,
            model_dropdown: None,
            thinking_dropdown: None,
            selected_model: None,
            thinking_level: None,
            chat_state: ChatState::Idle,
            _model_dropdown_subscription: None,
            _thinking_dropdown_subscription: None,
        }
    }

    pub fn with_at_mention(mut self, enabled: bool) -> Self {
        self.enable_at_mention = enabled;
        self
    }

    pub fn with_slash_commands(mut self, enabled: bool) -> Self {
        self.enable_slash_commands = enabled;
        self
    }

    pub fn content(&self, cx: &gpui::App) -> SharedString {
        self.input_state.read(cx).value()
    }

    pub fn is_popup_visible(&self) -> bool {
        self.is_at_popup_visible() || self.is_command_popup_visible()
    }

    pub fn is_at_popup_visible(&self) -> bool {
        self.at_mention_active && !self.mention_items.is_empty()
    }

    pub fn is_command_popup_visible(&self) -> bool {
        self.slash_command_active && !self.slash_command_items.is_empty()
    }

    pub fn popup_items(&self) -> &[MentionItem] {
        &self.mention_items
    }

    pub fn popup_highlighted(&self) -> usize {
        self.at_mention_highlighted
    }

    pub fn slash_command_items(&self) -> &[CommandItem] {
        &self.slash_command_items
    }

    pub fn slash_command_highlighted(&self) -> usize {
        self.slash_command_highlighted
    }

    pub fn is_just_selected_mention(&self) -> bool {
        self.just_selected_mention
    }

    pub fn clear_just_selected_mention(&mut self) {
        self.just_selected_mention = false;
    }

    pub fn selected_model(&self) -> Option<&str> {
        self.selected_model.as_deref()
    }

    pub fn thinking_level(&self) -> Option<&str> {
        self.thinking_level.as_deref()
    }

    pub fn set_workspace(
        &mut self,
        id: String,
        dir: PathBuf,
        name: String,
        cx: &mut Context<Self>,
    ) {
        if self.cached_workspace_id == Some(id.clone()) {
            return;
        }
        self.cached_workspace_id = Some(id);
        self.workspace_dir = Some(dir);
        self.workspace_name = Some(name);
        self.file_cache_loaded = false;
        self.file_cache_loading = false;
        self.file_cache.clear();
        self.load_file_cache(cx);
    }

    fn load_file_cache(&mut self, cx: &mut Context<Self>) {
        if self.file_cache_loading || self.file_cache_loaded {
            return;
        }
        let Some(ref dir) = self.workspace_dir else {
            return;
        };
        let dir = dir.clone();
        self.file_cache_loading = true;
        cx.spawn(async move |this, cx| {
            let entries = smol::unblock(move || file_scanner::scan_directory(&dir)).await;
            let mention_items: Vec<MentionItem> = entries
                .into_iter()
                .map(|e| MentionItem {
                    name: e.name,
                    relative_path: e.relative_path,
                    absolute_path: e.path,
                    is_dir: e.is_dir,
                })
                .collect();
            this.update(cx, |input, cx| {
                input.file_cache = mention_items;
                input.file_cache_loaded = true;
                input.file_cache_loading = false;
                input.update_popups(cx);
            })
            .ok();
        })
        .detach();
    }

    pub fn set_commands(&mut self, commands: Vec<CommandItem>, cx: &mut Context<Self>) {
        self.available_commands = commands;
        self.update_popups(cx);
    }

    pub fn update_popups(&mut self, _cx: &mut Context<Self>) {
        let cursor = self.input_state.read(_cx).cursor();
        let content = self.input_state.read(_cx).value().to_string();

        // First check slash command
        if self.enable_slash_commands {
            if let Some(parse) = parse_slash_command(&content, cursor) {
                self.slash_command_query = parse.query.clone();
                self.slash_command_replace_range = parse.replace_range.clone();

                let filtered = filter_command_items(&self.available_commands, &parse.query);
                self.slash_command_active = !filtered.is_empty();
                self.slash_command_items = filtered;

                if self.slash_command_highlighted >= self.slash_command_items.len() {
                    self.slash_command_highlighted = 0;
                }

                // Close at mention
                self.at_mention_active = false;
                self.mention_items.clear();
                return;
            } else {
                self.slash_command_active = false;
                self.slash_command_items.clear();
            }
        }

        // Then check at mention
        if !self.enable_at_mention || self.workspace_dir.is_none() {
            self.at_mention_active = false;
            self.mention_items.clear();
            return;
        }

        if let Some(parse) = parse_at_mention(&content, cursor) {
            self.at_mention_query = parse.query.clone();
            self.at_mention_replace_range = parse.replace_range.clone();

            if self.file_cache_loaded {
                let filtered = filter_mention_items(&self.file_cache, &parse.query);
                self.at_mention_active = !filtered.is_empty();
                self.mention_items = filtered;
            } else {
                self.at_mention_active = false;
                self.mention_items.clear();
            }

            if self.at_mention_highlighted >= self.mention_items.len() {
                self.at_mention_highlighted = 0;
            }
        } else {
            self.at_mention_active = false;
            self.at_mention_query.clear();
            self.mention_items.clear();
        }
    }

    pub fn navigate_popup(&mut self, direction: i32, cx: &mut Context<Self>) {
        if !self.mention_items.is_empty() {
            let len = self.mention_items.len();
            if direction > 0 {
                self.at_mention_highlighted = (self.at_mention_highlighted + 1) % len;
            } else if direction < 0 {
                self.at_mention_highlighted = if self.at_mention_highlighted == 0 {
                    len - 1
                } else {
                    self.at_mention_highlighted - 1
                };
            }
            self.at_mention_scroll_pending.set(true);
        } else if !self.slash_command_items.is_empty() {
            let len = self.slash_command_items.len();
            if direction > 0 {
                self.slash_command_highlighted = (self.slash_command_highlighted + 1) % len;
            } else if direction < 0 {
                self.slash_command_highlighted = if self.slash_command_highlighted == 0 {
                    len - 1
                } else {
                    self.slash_command_highlighted - 1
                };
            }
            self.command_scroll_pending.set(true);
        }
        cx.notify();
    }

    pub fn select_highlighted_mention(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = self.mention_items.get(self.at_mention_highlighted).cloned() {
            let suffix = if item.is_dir { "/" } else { "" };
            let insertion = format!("@{}{} ", item.relative_path, suffix);
            let range = self.at_mention_replace_range.clone();
            self.replace_range(range, &insertion, window, cx);
            self.at_mention_active = false;
            self.mention_items.clear();
            self.just_selected_mention = true;
        } else {
            self.at_mention_active = false;
            self.mention_items.clear();
        }
        cx.notify();
    }

    pub fn select_mention_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.mention_items.len() {
            self.at_mention_highlighted = index;
            self.select_highlighted_mention(window, cx);
        }
    }

    pub fn select_highlighted_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = self
            .slash_command_items
            .get(self.slash_command_highlighted)
            .cloned()
        {
            let insertion = format!("/{} ", item.name);
            let range = self.slash_command_replace_range.clone();
            self.replace_range(range, &insertion, window, cx);
            self.slash_command_active = false;
            self.slash_command_items.clear();
        } else {
            self.slash_command_active = false;
            self.slash_command_items.clear();
        }
        cx.notify();
    }

    pub fn select_command_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.slash_command_items.len() {
            self.slash_command_highlighted = index;
            self.select_highlighted_command(window, cx);
        }
    }

    pub fn close_popup(&mut self, cx: &mut Context<Self>) {
        self.at_mention_active = false;
        self.mention_items.clear();
        self.slash_command_active = false;
        self.slash_command_items.clear();
        cx.notify();
    }

    fn replace_range(
        &mut self,
        range: Range<usize>,
        replacement: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let content = self.input_state.read(cx).value();
        let range_start = range.start.min(content.len());
        let range_end = range.end.min(content.len());
        let new_content = content[0..range_start].to_string() + replacement + &content[range_end..];
        let new_cursor = range_start + replacement.len();

        self.input_state.update(cx, |state, cx| {
            state.set_value(new_content, window, cx);
            let position = state.text().offset_to_position(new_cursor);
            state.set_cursor_position(position, window, cx);
        });
        self.update_popups(cx);
    }

    pub fn set_content(
        &mut self,
        content: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let content = content.into();
        self.input_state.update(cx, |state, cx| {
            state.set_value(content, window, cx);
        });
        self.update_popups(cx);
        cx.emit(ChatInputEvent::Change);
        cx.notify();
    }

    pub fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input_state.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });
        self.at_mention_active = false;
        self.mention_items.clear();
        self.slash_command_active = false;
        self.slash_command_items.clear();
        self.just_selected_mention = false;
        self.pending_attachments.clear();
        cx.emit(ChatInputEvent::Change);
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input_state.update(cx, |state, cx| {
            state.focus(window, cx);
        });
    }

    pub fn pending_attachments(&self) -> &[PendingAttachment] {
        &self.pending_attachments
    }

    pub fn take_pending_attachments(&mut self) -> Vec<PendingAttachment> {
        std::mem::take(&mut self.pending_attachments)
    }

    /// Single entry point for syncing all composer state from the owning
    /// window. Call this whenever the session fires (new tokens, state
    /// changes, model/thinking updates) instead of mutating ChatInput fields
    /// piecemeal — it keeps dropdown selections, chat_state, and the toolbar
    /// appearance consistent in one shot.
    ///
    /// - `model` / `thinking_level`: the session's current selections; used
    ///   to update the dropdowns (and to coerce the thinking level back into
    ///   the valid set for the active model via `refresh_thinking_dropdown`).
    /// - `state`: the live `ChatState` (Idle/Loading/Streaming/Error) so the
    ///   toolbar can swap send↔stop and disable attachments while busy.
    pub fn sync(
        &mut self,
        model: Option<String>,
        thinking_level: Option<String>,
        state: ChatState,
        cx: &mut Context<Self>,
    ) {
        self.selected_model = model.clone();
        self.thinking_level = thinking_level;
        self.chat_state = state;
        let _ = cx.update_window(self.window_handle, |_, window, cx| {
            if let Some(ref dropdown) = self.model_dropdown {
                dropdown.update(cx, |dropdown, cx| {
                    if let Some(ref value) = model {
                        dropdown.set_selected_value(value, window, cx);
                    } else {
                        dropdown.set_selected_index(None, window, cx);
                    }
                });
            }
        });
        self.refresh_thinking_dropdown(cx);
        cx.notify();
    }

    /// Lightweight state-only update for the render hot path. Use `sync`
    /// for full session synchronization (dropdowns + model + thinking); this
    /// is just to keep the toolbar's send↔stop button and disabled state in
    /// sync on every frame without re-running dropdown bookkeeping.
    pub fn set_chat_state(&mut self, state: ChatState, cx: &mut Context<Self>) {
        self.chat_state = state;
        cx.notify();
    }

    const DEFAULT_THINKING_LEVELS: [(&'static str, &'static str); 6] = [
        ("off", "Off"),
        ("minimal", "Minimal"),
        ("low", "Low"),
        ("medium", "Medium"),
        ("high", "High"),
        ("xhigh", "Extra High"),
    ];

    fn thinking_level_items_for_model(
        models: &[ModelInfo],
        model_id: Option<&str>,
    ) -> Vec<SelectModelItem> {
        let map = model_id
            .and_then(|id| models.iter().find(|m| m.id == id))
            .and_then(|m| m.thinking_level_map.as_ref());

        Self::DEFAULT_THINKING_LEVELS
            .iter()
            .filter(|(id, _)| match map {
                Some(m) => !matches!(m.get(*id), Some(None)),
                None => true,
            })
            .map(|(id, label)| SelectModelItem {
                id: (*id).to_string(),
                name: (*label).into(),
            })
            .collect()
    }

    fn refresh_thinking_dropdown(&mut self, cx: &mut Context<Self>) {
        let models = cx.global::<AppStore>().models.clone();
        let items = Self::thinking_level_items_for_model(&models, self.selected_model.as_deref());
        let valid_ids: std::collections::HashSet<String> =
            items.iter().map(|i| i.id.clone()).collect();

        let new_level = self
            .thinking_level
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

        let level_changed = new_level != self.thinking_level;
        if level_changed {
            self.thinking_level = new_level.clone();
            if let Some(ref level) = new_level {
                cx.emit(ChatInputEvent::ThinkingChanged(level.clone()));
            }
        }

        let selected_value = self.thinking_level.clone();
        let items = SearchableVec::new(items);
        let _ = cx.update_window(self.window_handle, |_, window, cx| {
            if let Some(ref dropdown) = self.thinking_dropdown {
                dropdown.update(cx, |dropdown, cx| {
                    dropdown.set_items(items.clone(), window, cx);
                    if let Some(ref value) = selected_value {
                        dropdown.set_selected_value(value, window, cx);
                    } else {
                        dropdown.set_selected_index(None, window, cx);
                    }
                });
            }
        });
        cx.notify();
    }

    // =====================================================================
    // Attachment helpers
    // =====================================================================

    fn path_to_attachment(path: PathBuf) -> Result<PendingAttachment, String> {
        let metadata =
            std::fs::metadata(&path).map_err(|e| format!("Cannot read file metadata: {}", e))?;
        if metadata.is_dir() {
            return Err(format!(
                "{}: please select a file, not a directory",
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("selected item")
            ));
        }
        let size = metadata.len();
        if size > 5 * 1024 * 1024 {
            return Err(format!(
                "{}: file is larger than 5 MB",
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("selected file")
            ));
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();

        let extension_is_text = has_text_extension(&path);
        let guessed_mime = mime_guess::from_path(&path).first();

        if let Some(ref mime) = guessed_mime {
            if is_supported_image_mime(mime) {
                let mime_type = mime.to_string();
                let bytes =
                    std::fs::read(&path).map_err(|e| format!("Failed to read file: {}", e))?;
                let base64 =
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
                return Ok(PendingAttachment::Image {
                    path,
                    name,
                    mime_type,
                    base64,
                });
            }
            if extension_is_text || is_text_mime(mime) {
                const MAX_TEXT_BYTES: usize = 100 * 1024;
                if size > MAX_TEXT_BYTES as u64 {
                    return Err(format!("{}: text file is larger than 100 KB", name));
                }
                let bytes =
                    std::fs::read(&path).map_err(|e| format!("Failed to read file: {}", e))?;
                let content = String::from_utf8(bytes)
                    .map_err(|_| format!("{}: binary files are not supported", name))?;
                return Ok(PendingAttachment::Text {
                    path,
                    name,
                    content,
                });
            }
            return Err(format!("{}: binary files are not supported", name));
        }

        const MAX_TEXT_BYTES: usize = 100 * 1024;
        if size > MAX_TEXT_BYTES as u64 {
            return Err(format!("{}: file is larger than 100 KB", name));
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("Failed to read file: {}", e))?;
        let content = String::from_utf8(bytes)
            .map_err(|_| format!("{}: binary files are not supported", name))?;
        Ok(PendingAttachment::Text {
            path,
            name,
            content,
        })
    }

    fn add_attachments(
        &mut self,
        results: Vec<Result<PendingAttachment, String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut errors = Vec::new();
        for result in results {
            match result {
                Ok(attachment) => self.pending_attachments.push(attachment),
                Err(err) => errors.push(err),
            }
        }
        if !errors.is_empty() {
            let message = if errors.len() == 1 {
                errors.into_iter().next().unwrap()
            } else {
                format!(
                    "{} files could not be attached:\n{}",
                    errors.len(),
                    errors.join("\n")
                )
            };
            window.push_notification(Notification::error(message), cx);
        }
        if !self.pending_attachments.is_empty() {
            self.focus(window, cx);
            cx.notify();
        }
    }

    pub fn pick_and_send_file(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.chat_state, ChatState::Streaming | ChatState::Loading) {
            return;
        }

        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });

        cx.spawn_in(window, async move |this, cx| {
            let paths = match rx.await {
                Ok(Ok(Some(paths))) => paths,
                _ => return,
            };
            if paths.is_empty() {
                return;
            }

            let (supported, unsupported): (Vec<PathBuf>, Vec<PathBuf>) = paths
                .into_iter()
                .partition(|p| is_supported_attachment_path(p));

            let results: Vec<Result<PendingAttachment, String>> = smol::unblock(move || {
                supported.into_iter().map(Self::path_to_attachment).collect()
            })
            .await;

            this.update_in(cx, |this, window, cx| {
                if !unsupported.is_empty() {
                    let names: Vec<String> = unsupported
                        .iter()
                        .map(|p| {
                            p.file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("selected file")
                                .to_string()
                        })
                        .collect();
                    let message = if unsupported.len() == 1 {
                        format!(
                            "{}: only images and text files can be attached",
                            names[0]
                        )
                    } else {
                        format!(
                            "{} files cannot be attached (only images and text files are allowed):\n{}",
                            names.len(),
                            names.join("\n")
                        )
                    };
                    window.push_notification(Notification::error(message), cx);
                }
                this.add_attachments(results, window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn handle_paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.chat_state, ChatState::Streaming | ChatState::Loading) {
            return;
        }

        let Some(clipboard) = cx.read_from_clipboard() else {
            return;
        };

        let mut image_attachments: Vec<PendingAttachment> = Vec::new();
        let mut file_paths: Vec<PathBuf> = Vec::new();

        for entry in clipboard.into_entries() {
            match entry {
                ClipboardEntry::Image(image) => {
                    let format = image.format;
                    let mime_type = format.mime_type().to_string();
                    let extension = extension_for_image_format(format);
                    let name = format!("pasted-image.{}", extension);
                    let base64 = base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        &image.bytes,
                    );
                    image_attachments.push(PendingAttachment::Image {
                        path: PathBuf::from(&name),
                        name,
                        mime_type,
                        base64,
                    });
                }
                ClipboardEntry::ExternalPaths(paths) => {
                    file_paths.extend(
                        paths
                            .0
                            .into_iter()
                            .filter(|p| is_supported_attachment_path(p)),
                    );
                }
                _ => {}
            }
        }

        if image_attachments.is_empty() && file_paths.is_empty() {
            return;
        }

        cx.stop_propagation();

        for attachment in image_attachments {
            self.pending_attachments.push(attachment);
        }

        if !file_paths.is_empty() {
            cx.spawn_in(window, async move |this, cx| {
                let results: Vec<Result<PendingAttachment, String>> = smol::unblock(move || {
                    file_paths
                        .into_iter()
                        .map(Self::path_to_attachment)
                        .collect()
                })
                .await;

                this.update_in(cx, |this, window, cx| {
                    this.add_attachments(results, window, cx);
                })
                .ok();
            })
            .detach();
        }

        if !self.pending_attachments.is_empty() {
            self.focus(window, cx);
            cx.notify();
        }
    }

    // =====================================================================
    // Voice input
    // =====================================================================

    pub fn toggle_voice_input(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.voice_state {
            VoiceState::Idle => self.start_voice_input(window, cx),
            VoiceState::Recording => self.stop_voice_input(window, cx),
            VoiceState::Transcribing => {}
        }
    }

    fn start_voice_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match start_recording() {
            Ok(recorder) => {
                self.voice_recorder = Some(recorder);
                self.voice_state = VoiceState::Recording;
                cx.notify();
            }
            Err(err) => {
                window.push_notification(
                    Notification::error(format!("Voice input error: {}", err)),
                    cx,
                );
                cx.notify();
            }
        }
    }

    fn stop_voice_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(recorder) = self.voice_recorder.take() else {
            return;
        };
        let wav_bytes = recorder.stop();
        self.voice_state = VoiceState::Transcribing;
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = transcribe(&wav_bytes).await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(text) if !text.is_empty() => {
                        let current = this.content(cx).to_string();
                        let new_text = if current.is_empty() {
                            text
                        } else if current.ends_with(' ') {
                            current + &text
                        } else {
                            current + " " + &text
                        };
                        this.set_content(new_text, window, cx);
                    }
                    Ok(_) => {}
                    Err(err) => {
                        window.push_notification(
                            Notification::error(format!("Transcription failed: {}", err)),
                            cx,
                        );
                    }
                }
                this.voice_state = VoiceState::Idle;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // =====================================================================
    // Rendering
    // =====================================================================

    fn render_at_mention_popup(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.popup_items();
        let highlighted = self.popup_highlighted();

        if !items.is_empty() && highlighted < items.len() && self.at_mention_scroll_pending.get() {
            self.at_mention_scroll_handle.scroll_to_item(highlighted);
            self.at_mention_scroll_pending.set(false);
        }

        div()
            .relative()
            .px_3()
            .pb_1()
            .child(
                div()
                    .id("at-mention-overlay")
                    .absolute()
                    .occlude()
                    .top(px(-5000.))
                    .left(px(-5000.))
                    .w(px(10000.))
                    .h(px(10000.))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.close_popup(cx);
                        }),
                    ),
            )
            .child(
                div()
                    .id("at-mention-popup")
                    .track_scroll(&self.at_mention_scroll_handle)
                    .absolute()
                    .occlude()
                    .bottom(px(0.))
                    .left(px(12.))
                    .right(px(12.))
                    .max_h(px(240.))
                    .overflow_y_scroll()
                    .bg(cx.theme().popover)
                    .border_1()
                    .border_color(cx.theme().primary)
                    .rounded_md()
                    .py_1()
                    .shadow(vec![gpui::BoxShadow {
                        color: cx.theme().overlay,
                        offset: gpui::point(px(0.), px(4.)),
                        blur_radius: px(12.),
                        spread_radius: px(0.),
                        inset: false,
                    }])
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .children(items.iter().enumerate().map(|(idx, item)| {
                        let is_highlighted = idx == highlighted;
                        let icon = if item.is_dir {
                            "icons/folder.svg"
                        } else {
                            "icons/file.svg"
                        };
                        let label: SharedString = item.name.clone().into();
                        let detail: SharedString = if item.relative_path != item.name {
                            item.relative_path.clone().into()
                        } else {
                            "".into()
                        };
                        let item_idx = idx;
                        div()
                            .id(SharedString::from(format!("mention-{}", idx)))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .py_1p5()
                            .cursor_pointer()
                            .when(is_highlighted, |s| s.bg(cx.theme().accent))
                            .hover(|style| style.bg(cx.theme().accent))
                            .child(
                                svg()
                                    .path(icon)
                                    .size(px(14.))
                                    .text_color(if is_highlighted {
                                        cx.theme().primary
                                    } else {
                                        cx.theme().muted_foreground
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_baseline()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(if is_highlighted {
                                                cx.theme().foreground
                                            } else {
                                                cx.theme().muted_foreground
                                            })
                                            .child(label),
                                    )
                                    .when(!detail.is_empty(), |s| {
                                        s.child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(detail),
                                        )
                                    }),
                            )
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.select_mention_at(item_idx, _window, cx);
                            }))
                    })),
            )
    }

    fn render_command_popup(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.slash_command_items();
        let highlighted = self.slash_command_highlighted();

        if !items.is_empty() && highlighted < items.len() && self.command_scroll_pending.get() {
            self.command_scroll_handle.scroll_to_item(highlighted);
            self.command_scroll_pending.set(false);
        }

        div()
            .relative()
            .px_3()
            .pb_1()
            .child(
                div()
                    .id("command-overlay")
                    .absolute()
                    .occlude()
                    .top(px(-5000.))
                    .left(px(-5000.))
                    .w(px(10000.))
                    .h(px(10000.))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.close_popup(cx);
                        }),
                    ),
            )
            .child(
                div()
                    .id("command-popup")
                    .track_scroll(&self.command_scroll_handle)
                    .absolute()
                    .occlude()
                    .bottom(px(0.))
                    .left(px(12.))
                    .right(px(12.))
                    .max_h(px(240.))
                    .overflow_y_scroll()
                    .bg(cx.theme().popover)
                    .border_1()
                    .border_color(cx.theme().primary)
                    .rounded_md()
                    .py_1()
                    .shadow(vec![gpui::BoxShadow {
                        color: cx.theme().overlay,
                        offset: gpui::point(px(0.), px(4.)),
                        blur_radius: px(12.),
                        spread_radius: px(0.),
                        inset: false,
                    }])
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .children(items.iter().enumerate().map(|(idx, item)| {
                        let is_highlighted = idx == highlighted;
                        let label: SharedString = format!("/{}", item.name).into();
                        let detail: SharedString =
                            item.description.clone().unwrap_or_default().into();
                        let source_label: SharedString = (match item.source.as_str() {
                            "extension" => "Extension",
                            "prompt" => "Prompt",
                            "skill" => "Skill",
                            _ => &item.source,
                        })
                        .to_string()
                        .into();
                        let item_idx = idx;
                        div()
                            .id(SharedString::from(format!("command-{}", idx)))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .py_1p5()
                            .cursor_pointer()
                            .when(is_highlighted, |s| s.bg(cx.theme().accent))
                            .hover(|style| style.bg(cx.theme().accent))
                            .child(
                                div()
                                    .w(px(160.))
                                    .overflow_hidden()
                                    .text_sm()
                                    .text_color(if is_highlighted {
                                        cx.theme().foreground
                                    } else {
                                        cx.theme().muted_foreground
                                    })
                                    .child(div().whitespace_nowrap().text_ellipsis().child(label)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .line_clamp(2)
                                    .when(!detail.is_empty(), |s| s.child(detail)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .px_1()
                                    .py_0p5()
                                    .rounded_sm()
                                    .bg(cx.theme().secondary)
                                    .text_color(cx.theme().secondary_foreground)
                                    .child(source_label),
                            )
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.select_command_at(item_idx, _window, cx);
                            }))
                    })),
            )
    }

    fn render_attachment_bar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        if self.pending_attachments.is_empty() {
            return div().into_any_element();
        }
        let attachments = self.pending_attachments.clone();
        div()
            .px_3()
            .pt_2()
            .pb_1()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap_2()
            .children(
                attachments
                    .into_iter()
                    .enumerate()
                    .map(|(idx, attachment)| {
                        let name = attachment.name().to_string();
                        div()
                            .id(SharedString::from(format!("pending-attachment-{}", idx)))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(cx.theme().accent)
                            .text_color(cx.theme().accent_foreground)
                            .child(
                                Icon::empty()
                                    .path("icons/file.svg")
                                    .size(px(14.))
                                    .text_color(cx.theme().accent_foreground),
                            )
                            .child(div().text_sm().child(SharedString::from(name)))
                            .child(
                                Button::new(SharedString::from(format!(
                                    "remove-attachment-{}",
                                    idx
                                )))
                                .with_size(Size::XSmall)
                                .ghost()
                                .icon(
                                    Icon::empty()
                                        .path("icons/close.svg")
                                        .size(px(12.))
                                        .text_color(cx.theme().accent_foreground),
                                )
                                .on_click(cx.listener(
                                    move |this, _, _window, cx| {
                                        this.pending_attachments.remove(idx);
                                        cx.notify();
                                    },
                                )),
                            )
                    }),
            )
            .into_any_element()
    }

    fn render_toolbar(&self, cx: &mut Context<Self>, is_disabled: bool) -> impl IntoElement {
        let is_streaming = matches!(self.chat_state, ChatState::Streaming);
        let is_busy = matches!(self.chat_state, ChatState::Streaming | ChatState::Loading);
        div()
            .flex()
            .flex_row()
            .gap_1()
            .items_center()
            .when_some(self.model_dropdown.as_ref(), |this, dropdown| {
                this.child(
                    div().max_w_full().child(
                        Select::new(dropdown)
                            .with_size(Size::Small)
                            .appearance(false)
                            .w(px(180.))
                            .placeholder("LLM Model")
                            .menu_width(gpui::Length::Auto)
                            .menu_max_h(rems(10.)),
                    ),
                )
            })
            .when_some(self.thinking_dropdown.as_ref(), |this, dropdown| {
                this.child(
                    div().max_w_full().child(
                        Select::new(dropdown)
                            .with_size(Size::Small)
                            .appearance(false)
                            .w(px(140.))
                            .placeholder("Thinking effort")
                            .menu_width(gpui::Length::Auto)
                            .menu_max_h(rems(10.)),
                    ),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("attach-file-btn")
                    .with_size(Size::Small)
                    .ghost()
                    .disabled(is_busy)
                    .icon(
                        Icon::empty()
                            .path("icons/plus.svg")
                            .size(px(14.))
                            .text_color(cx.theme().muted_foreground),
                    )
                    .on_click(cx.listener(Self::pick_and_send_file))
                    .into_any_element(),
            )
            .child({
                let is_recording = self.voice_state == VoiceState::Recording;
                let is_transcribing = self.voice_state == VoiceState::Transcribing;

                if is_recording {
                    Button::new("voice-btn")
                        .with_size(Size::Small)
                        .custom(
                            ButtonCustomVariant::new(cx)
                                .color(cx.theme().danger.into())
                                .foreground(cx.theme().danger_foreground.into())
                                .hover(cx.theme().danger_hover.into())
                                .active(cx.theme().danger_active.into()),
                        )
                        .icon(
                            Icon::empty()
                                .path("icons/mic.svg")
                                .size(px(14.))
                                .text_color(cx.theme().danger_foreground),
                        )
                        .on_click(cx.listener(Self::toggle_voice_input))
                        .into_any_element()
                } else {
                    Button::new("voice-btn")
                        .with_size(Size::Small)
                        .loading(is_transcribing)
                        .ghost()
                        .icon(
                            Icon::empty()
                                .path("icons/mic.svg")
                                .size(px(14.))
                                .text_color(cx.theme().muted_foreground),
                        )
                        .on_click(cx.listener(Self::toggle_voice_input))
                        .into_any_element()
                }
            })
            .child(if is_streaming {
                Button::new("stop-btn")
                    .with_size(Size::Small)
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(cx.theme().danger.into())
                            .foreground(cx.theme().danger_foreground.into())
                            .hover(cx.theme().danger_hover.into())
                            .active(cx.theme().danger_active.into()),
                    )
                    .icon(
                        Icon::empty()
                            .path("icons/stop.svg")
                            .size(px(14.))
                            .text_color(cx.theme().danger_foreground),
                    )
                    .on_click(cx.listener(|_, _, _window, cx| {
                        cx.emit(ChatInputEvent::Stop);
                    }))
                    .into_any_element()
            } else {
                Button::new("send-btn")
                    .with_size(Size::Small)
                    .primary()
                    .icon(
                        Icon::empty()
                            .path("icons/send.svg")
                            .size(px(14.))
                            .text_color(cx.theme().primary_foreground),
                    )
                    .disabled(is_disabled)
                    .on_click(cx.listener(|_, _, _window, cx| {
                        cx.emit(ChatInputEvent::Submit);
                    }))
                    .into_any_element()
            })
    }

    fn render_composer(&mut self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let input_focused = self.focus_handle.is_focused(window);
        let input_empty = self.content(cx).is_empty();
        let is_streaming = matches!(self.chat_state, ChatState::Streaming);
        let is_loading = matches!(self.chat_state, ChatState::Loading);
        let is_disabled =
            is_streaming || is_loading || (input_empty && self.pending_attachments.is_empty());

        div()
            .px_3()
            .pb_3()
            .when(self.is_at_popup_visible(), |this| {
                this.child(self.render_at_mention_popup(cx))
            })
            .when(self.is_command_popup_visible(), |this| {
                this.child(self.render_command_popup(cx))
            })
            .child(
                div()
                    .bg(cx.theme().secondary)
                    .rounded_xl()
                    .border_1()
                    .border_color(if input_focused {
                        cx.theme().primary
                    } else {
                        cx.theme().border
                    })
                    .shadow_sm()
                    .px_3()
                    .pb_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when(!self.pending_attachments.is_empty(), |this| {
                        this.child(self.render_attachment_bar(cx))
                    })
                    .child(
                        div()
                            .flex()
                            .capture_action(cx.listener(|this, _action: &MoveUp, _window, cx| {
                                if this.is_popup_visible() {
                                    this.navigate_popup(-1, cx);
                                    cx.stop_propagation();
                                }
                            }))
                            .capture_action(cx.listener(|this, _action: &MoveDown, _window, cx| {
                                if this.is_popup_visible() {
                                    this.navigate_popup(1, cx);
                                    cx.stop_propagation();
                                }
                            }))
                            .capture_action(cx.listener(|this, _action: &Enter, window, cx| {
                                if this.is_at_popup_visible() {
                                    this.select_highlighted_mention(window, cx);
                                    cx.stop_propagation();
                                } else if this.is_command_popup_visible() {
                                    this.select_highlighted_command(window, cx);
                                    cx.stop_propagation();
                                }
                            }))
                            .capture_action(cx.listener(
                                |this, _action: &IndentInline, window, cx| {
                                    if this.is_at_popup_visible() {
                                        this.select_highlighted_mention(window, cx);
                                        cx.stop_propagation();
                                    } else if this.is_command_popup_visible() {
                                        this.select_highlighted_command(window, cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .capture_action(cx.listener(|this, _action: &Paste, window, cx| {
                                this.handle_paste(_action, window, cx);
                            }))
                            .child(Input::new(&self.input_state).appearance(false).w_full()),
                    )
                    .child(self.render_toolbar(cx, is_disabled)),
            )
    }
}

impl Focusable for ChatInput {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ChatInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.composer_enabled {
            self.render_composer(window, cx).into_any_element()
        } else {
            div()
                .child(Input::new(&self.input_state).appearance(false).w_full())
                .into_any_element()
        }
    }
}

// =====================================================================
// Attachment classification helpers
// =====================================================================

fn is_supported_image_mime(mime: &mime_guess::Mime) -> bool {
    mime.type_() == "image" && matches!(mime.subtype().as_str(), "png" | "jpeg" | "gif" | "webp")
}

fn is_text_mime(mime: &mime_guess::Mime) -> bool {
    if mime.type_() == "text" {
        return true;
    }
    matches!(
        (mime.type_().as_str(), mime.subtype().as_str()),
        ("application", "json")
            | ("application", "xml")
            | ("application", "javascript")
            | ("application", "x-javascript")
            | ("application", "typescript")
    )
}

const TEXT_FILE_EXTENSIONS: &[&str] = &[
    "txt",
    "md",
    "markdown",
    "json",
    "yaml",
    "yml",
    "toml",
    "csv",
    "tsv",
    "log",
    "rs",
    "py",
    "js",
    "ts",
    "jsx",
    "tsx",
    "mjs",
    "cjs",
    "html",
    "htm",
    "css",
    "scss",
    "sass",
    "less",
    "sql",
    "sh",
    "bash",
    "zsh",
    "fish",
    "c",
    "cpp",
    "cc",
    "cxx",
    "h",
    "hpp",
    "hh",
    "go",
    "java",
    "kt",
    "kts",
    "swift",
    "rb",
    "php",
    "cs",
    "fs",
    "fsx",
    "ml",
    "clj",
    "cljs",
    "scala",
    "r",
    "lua",
    "pl",
    "pm",
    "vim",
    "ex",
    "exs",
    "erl",
    "hrl",
    "elm",
    "hs",
    "lhs",
    "cl",
    "lisp",
    "scm",
    "rkt",
    "dart",
    "groovy",
    "jl",
    "m",
    "wl",
    "xml",
    "xsl",
    "xsd",
    "graphql",
    "gql",
    "prisma",
    "proto",
    "env",
    "ini",
    "conf",
    "cfg",
    "properties",
    "gitignore",
    "dockerfile",
    "tf",
    "hcl",
    "nomad",
    "pkl",
    "nix",
    "vue",
    "svelte",
    "astro",
];

fn has_text_extension(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| TEXT_FILE_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn is_image_file(path: &std::path::Path) -> bool {
    mime_guess::from_path(path)
        .first()
        .map(|m| is_supported_image_mime(&m))
        .unwrap_or(false)
}

fn is_supported_attachment_path(path: &std::path::Path) -> bool {
    if is_image_file(path) {
        return true;
    }
    if has_text_extension(path) {
        return true;
    }
    match mime_guess::from_path(path).first() {
        Some(mime) => is_text_mime(&mime),
        None => true,
    }
}

fn extension_for_image_format(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
        ImageFormat::Ico => "ico",
        ImageFormat::Pnm => "pnm",
    }
}

// =====================================================================
// Mention / command parsing helpers
// =====================================================================

fn parse_at_mention(content: &str, cursor: usize) -> Option<AtMentionParse> {
    if cursor == 0 {
        return None;
    }

    let text_before_cursor = &content[..cursor];

    let at_pos = text_before_cursor.rfind('@')?;
    if at_pos > 0 {
        let ch_before = content.as_bytes().get(at_pos - 1).copied();
        match ch_before {
            Some(b' ') | Some(b'\n') | Some(b'\t') | Some(b'(') | Some(b'[') | Some(b'{') => {}
            _ => return None,
        }
    }

    let after_at = &text_before_cursor[at_pos + 1..];
    if after_at.starts_with(' ') || after_at.contains(char::is_whitespace) {
        return None;
    }

    let query = if after_at.is_empty() {
        String::new()
    } else {
        let end = after_at
            .find(|c: char| c.is_whitespace())
            .unwrap_or(after_at.len());
        after_at[..end].to_string()
    };

    let replace_start = at_pos;
    let replace_end = at_pos + 1 + query.len();

    Some(AtMentionParse {
        query,
        replace_range: replace_start..replace_end,
    })
}

fn filter_mention_items(items: &[MentionItem], query: &str) -> Vec<MentionItem> {
    if query.is_empty() {
        return items.iter().take(50).cloned().collect();
    }
    let q = query.to_lowercase();
    let mut matches: Vec<(MentionItem, usize)> = items
        .iter()
        .filter_map(|item| {
            let name_lower = item.name.to_lowercase();
            let path_lower = item.relative_path.to_lowercase();
            if name_lower.contains(&q) || path_lower.contains(&q) {
                let score = if name_lower.starts_with(&q) {
                    100
                } else if name_lower.contains(&q) {
                    50
                } else {
                    10
                };
                Some((item.clone(), score))
            } else {
                None
            }
        })
        .collect();
    matches.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.relative_path.cmp(&b.0.relative_path))
    });
    matches.into_iter().take(50).map(|(item, _)| item).collect()
}

fn parse_slash_command(content: &str, cursor: usize) -> Option<AtMentionParse> {
    if cursor == 0 {
        return None;
    }

    let text_before_cursor = &content[..cursor];

    let slash_pos = text_before_cursor.rfind('/')?;

    if slash_pos > 0 {
        let ch_before = content.as_bytes().get(slash_pos - 1).copied()?;
        if ch_before != b'\n' && ch_before != b'\r' {
            return None;
        }
    }

    let after_slash = &text_before_cursor[slash_pos + 1..];
    if after_slash.starts_with(' ') || after_slash.contains(char::is_whitespace) {
        return None;
    }

    let query = if after_slash.is_empty() {
        String::new()
    } else {
        let end = after_slash
            .find(|c: char| c.is_whitespace())
            .unwrap_or(after_slash.len());
        after_slash[..end].to_string()
    };

    let replace_start = slash_pos;
    let replace_end = slash_pos + 1 + query.len();

    Some(AtMentionParse {
        query,
        replace_range: replace_start..replace_end,
    })
}

fn filter_command_items(items: &[CommandItem], query: &str) -> Vec<CommandItem> {
    if query.is_empty() {
        return items.iter().take(50).cloned().collect();
    }
    let q = query.to_lowercase();
    let mut matches: Vec<(CommandItem, usize)> = items
        .iter()
        .filter_map(|item| {
            let name_lower = item.name.to_lowercase();
            let desc_lower = item.description.as_deref().unwrap_or("").to_lowercase();
            if name_lower.contains(&q) || desc_lower.contains(&q) {
                let score = if name_lower.starts_with(&q) {
                    100
                } else if name_lower.contains(&q) {
                    50
                } else {
                    10
                };
                Some((item.clone(), score))
            } else {
                None
            }
        })
        .collect();
    matches.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name.cmp(&b.0.name)));
    matches.into_iter().take(50).map(|(item, _)| item).collect()
}
