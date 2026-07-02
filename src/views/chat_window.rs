use std::{path::PathBuf, sync::Arc};

use base64::Engine as _;
use gpui::{
    Anchor, AnyElement, AnyWindowHandle, ClipboardItem, Context, Entity, FocusHandle, Image,
    ImageFormat, ImageSource, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, PathPromptOptions, Pixels, Render, ScrollHandle, SharedString, Styled, Window,
    div, img, prelude::*, px, svg,
};

use crate::config::model_config;
use crate::core::actions::{
    CancelInlineEdit, CloseWindow, ConfirmInlineEdit, SendMessage, StopStreaming,
};
use crate::core::app::AppStore;
use crate::core::session_handle::{SessionEvent, SessionHandle, SessionStats, WorkspaceInfo};
use crate::data::models::{ChatState, Message, MessagePart, PartState, Role};
use crate::data::store::{Store, ThreadMeta, WorkspaceMeta};
use crate::rpc::pi_rpc::ImageContent;
use crate::ui::chat_input::{ChatInput, ChatInputEvent, PendingAttachment};
use crate::ui::loader::{loader, text_loader};
use crate::utils::color::{workspace_color, workspace_foreground};
use crate::views::reasoning::Reasoning;
use crate::views::tool_call::ToolCall;
use crate::views::workspace_manager::{WorkspaceManager, WorkspaceManagerEvent};
use gpui_component::button::{Button, ButtonVariants, Toggle, ToggleVariants};
use gpui_component::dialog::{DialogAction, DialogClose, DialogFooter, DialogHeader, DialogTitle};
use gpui_component::input::{Input, InputState};
use gpui_component::notification::Notification;
use gpui_component::tag::Tag;
use gpui_component::text::{TextView, TextViewState};
use gpui_component::{
    ActiveTheme as _, Icon, Sizable as _, Size, WindowExt as _, h_flex, hover_card::HoverCard,
    scroll::Scrollbar, status_bar::StatusBar, v_flex,
};

type ReasoningEntities = Vec<Vec<Option<Entity<Reasoning>>>>;
type MarkdownEntities = Vec<Vec<Option<Entity<TextViewState>>>>;

fn image_format_for_mime(mime: &str) -> Option<ImageFormat> {
    match mime.to_lowercase().as_str() {
        "image/png" => Some(ImageFormat::Png),
        "image/jpeg" | "image/jpg" => Some(ImageFormat::Jpeg),
        "image/webp" => Some(ImageFormat::Webp),
        "image/gif" => Some(ImageFormat::Gif),
        "image/svg+xml" => Some(ImageFormat::Svg),
        "image/bmp" => Some(ImageFormat::Bmp),
        "image/tiff" => Some(ImageFormat::Tiff),
        "image/x-icon" | "image/vnd.microsoft.icon" => Some(ImageFormat::Ico),
        _ => None,
    }
}

fn image_source_from_base64(data: &str, mime: &str) -> Option<ImageSource> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    let format = image_format_for_mime(mime)?;
    Some(ImageSource::Image(Arc::new(Image::from_bytes(
        format, bytes,
    ))))
}

pub struct ChatWindow {
    pub thread_id: Option<String>,
    pub session_file: String,
    pub title: SharedString,
    pub messages: Vec<Message>,
    pub chat_input: gpui::Entity<ChatInput>,
    pub focus_handle: FocusHandle,
    pub state: ChatState,
    pub store: Arc<Store>,
    pub session: Option<Entity<SessionHandle>>,
    pub session_subscription: Option<gpui::Subscription>,
    pub reasoning_displays: ReasoningEntities,
    pub markdown_displays: MarkdownEntities,
    /// (msg_idx, part_idx, last_text_len) for the single currently-streaming
    /// markdown part.  Used to compute the delta for `push_str` so we only
    /// re-parse new tokens rather than the entire document on every event.
    pub streaming_md_pos: Option<(usize, usize, usize)>,
    pub scroll_handle: ScrollHandle,
    pub scroll_locked: bool,
    pub workspaces: Vec<WorkspaceMeta>,
    pub selected_workspace_id: Option<String>,
    pub workspace_manager: gpui::Entity<WorkspaceManager>,
    pub editing_message_id: Option<String>,
    pub inline_edit_input: Option<gpui::Entity<ChatInput>>,
    pub window_handle: AnyWindowHandle,
    pub pending_extension_request: Option<(String, String, serde_json::Value)>,
    pub extension_select_selected: Option<usize>,
    pub extension_status: std::collections::HashMap<String, SharedString>,
    pub extension_widgets: std::collections::HashMap<String, (Vec<SharedString>, String)>,
    pub session_stats: Option<SessionStats>,
}

impl ChatWindow {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        thread: Option<&ThreadMeta>,
        store: Arc<Store>,
    ) -> Self {
        let title: SharedString = thread
            .map(|t| {
                if t.title.is_empty() {
                    "New Thread".into()
                } else {
                    t.title.clone().into()
                }
            })
            .unwrap_or_else(|| "New Thread".into());

        let thread_id = thread.map(|t| t.id.clone());
        let models = cx.global::<AppStore>().models.clone();

        // For new threads, fall back to the pi agent's own default settings
        // instead of the (now-removed) app-level defaults.
        let pi_defaults = if thread.is_some() {
            None
        } else {
            cx.global::<AppStore>()
                .pi_bridge
                .as_ref()
                .and_then(|b| b.get_settings().ok())
        };

        let selected_model: Option<String> = if let Some(t) = thread {
            t.model.clone()
        } else {
            pi_defaults.as_ref().and_then(|s| {
                model_config::resolve_full_model_id(
                    &models,
                    s.default_provider.as_deref(),
                    s.default_model.as_deref(),
                )
            })
        };
        let selected_thinking_level: Option<String> = if let Some(t) = thread {
            t.thinking_level.clone()
        } else {
            pi_defaults
                .as_ref()
                .and_then(|s| s.default_thinking_level.clone())
                .or(Some("off".to_string()))
        };

        let mut workspaces = store.list_workspaces().unwrap_or_default();
        if workspaces.is_empty() {
            let default_dir = store.default_workspace_dir();
            std::fs::create_dir_all(&default_dir).ok();
            let default_path_str = default_dir.to_string_lossy().to_string();
            if let Ok(ws) = store.create_workspace("Default", &default_path_str) {
                workspaces.push(ws);
            }
        }
        Self::sort_workspaces(&mut workspaces);
        // Prefer the workspace saved on this thread's metadata (e.g. set by the
        // remote controller) when reopening it, so the bridged session runs in
        // the originalcwd instead of the always-default workspace.
        let saved_workspace_id = thread.and_then(|t| {
            t.metadata
                .as_ref()
                .and_then(|md| md.get("workspace_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });
        let selected_workspace_id = saved_workspace_id
            .as_ref()
            .and_then(|id| {
                workspaces
                    .iter()
                    .find(|ws| ws.id == *id)
                    .map(|ws| ws.id.clone())
            })
            .or_else(|| {
                workspaces
                    .iter()
                    .find(|ws| ws.name == "Default")
                    .map(|ws| ws.id.clone())
            })
            .or_else(|| workspaces.first().map(|ws| ws.id.clone()));

        let workspace_info = selected_workspace_id
            .as_ref()
            .and_then(|id| workspaces.iter().find(|ws| ws.id == *id))
            .map(|ws| WorkspaceInfo {
                id: ws.id.clone(),
                path: PathBuf::from(&ws.path),
                name: ws.name.clone(),
            });

        let chat_input = cx.new(|cx| {
            ChatInput::new_composer(
                window,
                cx,
                "Type a message...",
                &models,
                selected_model.clone(),
                selected_thinking_level.clone(),
            )
        });

        let workspace_manager = cx.new(|_| WorkspaceManager::new(workspaces.clone()));
        let window_handle = window.window_handle();

        let mut chat_window = Self {
            thread_id,
            session_file: String::new(),
            title: title.clone(),
            messages: vec![],
            chat_input,
            focus_handle: cx.focus_handle(),
            state: ChatState::Idle,
            store: store.clone(),
            session: None,
            session_subscription: None,
            reasoning_displays: vec![],
            markdown_displays: vec![],
            streaming_md_pos: None,
            scroll_handle: ScrollHandle::new(),
            scroll_locked: true,
            workspaces,
            selected_workspace_id,
            workspace_manager: workspace_manager.clone(),
            editing_message_id: None,
            inline_edit_input: None,
            window_handle,
            pending_extension_request: None,
            extension_select_selected: None,
            extension_status: std::collections::HashMap::new(),
            extension_widgets: std::collections::HashMap::new(),
            session_stats: None,
        };

        // Only create/attach a session immediately for restored threads so
        // history + slash commands are available right away. For new threads
        // leave the session uncreated so the workspace picker is shown; the
        // session is created lazily in `ensure_session` once the user picks a
        // workspace and sends the first message.
        if thread.is_some() {
            let session =
                Self::get_or_create_session(thread, workspace_info.clone(), None, None, cx);
            chat_window.attach_session(session, cx);
        }

        // Set initial workspace on chat input
        if let Some(ref ws) = workspace_info {
            chat_window.chat_input.update(cx, |ci, cx| {
                ci.set_workspace(ws.id.clone(), ws.path.clone(), ws.name.clone(), cx);
            });
        }

        // Subscribe to chat input events
        cx.subscribe_in(
            &chat_window.chat_input,
            window,
            |this, _input, event: &ChatInputEvent, window, cx| match event {
                ChatInputEvent::Change => cx.notify(),
                ChatInputEvent::Submit => {
                    this.send_message(&SendMessage, window, cx);
                }
                ChatInputEvent::Stop => {
                    this.stop_streaming(&StopStreaming, window, cx);
                }
                ChatInputEvent::ModelChanged(id) => {
                    if let Some(ref session) = this.session {
                        session.update(cx, |session, cx| {
                            session.set_model(Some(id.clone()), cx);
                        });
                    }
                    cx.notify();
                }
                ChatInputEvent::ThinkingChanged(id) => {
                    if let Some(ref session) = this.session {
                        session.update(cx, |session, cx| {
                            session.set_thinking_level(Some(id.clone()), cx);
                        });
                    }
                    cx.notify();
                }
            },
        )
        .detach();

        cx.subscribe(
            &workspace_manager,
            |this, _manager, event: &WorkspaceManagerEvent, cx| match event {
                WorkspaceManagerEvent::AddRequested => this.add_workspace(cx),
                WorkspaceManagerEvent::CloseRequested => {}
                WorkspaceManagerEvent::DeleteRequested { workspace_id } => {
                    this.delete_workspace(workspace_id.clone(), cx);
                }
            },
        )
        .detach();

        chat_window
    }

    fn attach_session(&mut self, session: Entity<SessionHandle>, cx: &mut Context<Self>) {
        self.session = Some(session.clone());
        self.sync_from_session(cx);
        self.sync_display_entities(cx);
        if self.scroll_locked {
            self.scroll_handle.scroll_to_bottom();
        }
        self.session_subscription = Some(cx.subscribe(
            &session,
            |this, _session, event: &SessionEvent, cx| {
                this.sync_from_session(cx);
                match event {
                    SessionEvent::ExportHtmlSucceeded { path } => {
                        let reveal_path = path.clone();
                        let _ = this.window_handle.update(cx, |_, window, cx| {
                            window.push_notification(
                                Notification::success("Session exported to HTML").action({
                                    let reveal_path = reveal_path.clone();
                                    move |_, _, _| {
                                        let reveal_path = reveal_path.clone();
                                        Button::new("reveal").label("Reveal").on_click(
                                            move |_, _, cx| {
                                                cx.reveal_path(&reveal_path);
                                            },
                                        )
                                    }
                                }),
                                cx,
                            );
                        });
                    }
                    SessionEvent::ExtensionUiRequest {
                        id,
                        method,
                        payload,
                    } => {
                        this.pending_extension_request =
                            Some((id.clone(), method.clone(), payload.clone()));
                    }
                    SessionEvent::Changed => {}
                }
                // Keep display entities in sync whenever the session fires
                // an event (new tokens, state changes, etc.).
                this.sync_display_entities(cx);
                cx.notify();
            },
        ));
        if self.thread_id.is_some() {
            if let Some(ref session) = self.session {
                session.update(cx, |session, cx| {
                    session.clear_new_activity(cx);
                });
            }
            cx.update_global(|_: &mut AppStore, _| {});
        }

        // Ask the SDK for the current session stats so the status bar is
        // populated immediately when opening or switching to a thread.
        if let Some(ref session) = self.session {
            session.update(cx, |session, _cx| {
                session.request_session_stats();
            });
        }
    }

    fn sync_from_session(&mut self, cx: &mut Context<Self>) {
        let Some(ref session) = self.session else {
            return;
        };
        let s = session.read(cx);
        let messages = s.messages.clone();
        let state = s.state.clone();
        let session_file = s.session_file.clone();
        let selected_model = s.selected_model.clone();
        let thinking_level = s.thinking_level.clone();
        let title = s.title.clone();
        let commands = s.commands.clone();

        self.messages = messages;
        self.state = state.clone();
        if matches!(self.state, ChatState::Streaming) && self.scroll_locked {
            self.scroll_handle.scroll_to_bottom();
        }
        self.session_file = session_file;
        self.title = title;
        self.session_stats = s.session_stats.clone();

        self.chat_input.update(cx, |ci, cx| {
            ci.set_commands(commands, cx);
            ci.sync(selected_model, thinking_level, state, cx);
        });
    }

    fn sort_workspaces(workspaces: &mut Vec<WorkspaceMeta>) {
        workspaces.sort_by(|a, b| {
            if a.name == "Default" {
                std::cmp::Ordering::Less
            } else if b.name == "Default" {
                std::cmp::Ordering::Greater
            } else {
                a.name.cmp(&b.name)
            }
        });
    }

    fn sync_workspace_manager(&mut self, cx: &mut Context<Self>) {
        let workspaces = self.workspaces.clone();
        self.workspace_manager.update(cx, |manager, _cx| {
            manager.set_workspaces(workspaces);
        });
    }

    fn open_workspace_manager(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_workspace_manager(cx);
        let manager = self.workspace_manager.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let manager_for_content = manager.clone();
            dialog
                .title("Workspaces")
                .content(move |content, window, cx| {
                    manager_for_content.update(cx, |manager, cx| {
                        content.child(manager.render_dialog_content(window, cx))
                    })
                })
        });
    }

    fn add_workspace(&mut self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |weak, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.first()
            {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Workspace")
                    .to_string();
                let path_str = path.to_string_lossy().to_string();
                match store.create_workspace(&name, &path_str) {
                    Ok(workspace) => {
                        let ws_id = workspace.id.clone();
                        let _ = weak.update(cx, |window, cx| {
                            window.workspaces.push(workspace);
                            Self::sort_workspaces(&mut window.workspaces);
                            window.selected_workspace_id = Some(ws_id);
                            window.sync_workspace_manager(cx);
                            cx.notify();
                        });
                    }
                    Err(e) => {
                        eprintln!("[mini-pi] failed to create workspace: {}", e);
                    }
                }
            }
        })
        .detach();
    }

    fn delete_workspace(&mut self, workspace_id: String, cx: &mut Context<Self>) {
        if let Err(e) = self.store.delete_workspace(&workspace_id) {
            eprintln!("[mini-pi] failed to delete workspace: {}", e);
            return;
        }

        self.workspaces
            .retain(|workspace| workspace.id != workspace_id);
        if self.selected_workspace_id == Some(workspace_id.clone()) {
            self.selected_workspace_id = self
                .workspaces
                .first()
                .map(|workspace| workspace.id.clone());
        }
        self.sync_workspace_manager(cx);
        cx.notify();
    }

    fn session_file_from_thread(thread: Option<&ThreadMeta>) -> String {
        thread
            .and_then(|t| t.session_file.clone())
            .unwrap_or_else(|| {
                use std::time::{SystemTime, UNIX_EPOCH};
                let ns = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                format!("session_{}.jsonl", ns)
            })
    }

    fn get_or_create_session(
        thread: Option<&ThreadMeta>,
        workspace: Option<WorkspaceInfo>,
        default_model: Option<String>,
        default_thinking_level: Option<String>,
        cx: &mut Context<Self>,
    ) -> Entity<SessionHandle> {
        let session_file = Self::session_file_from_thread(thread);

        if let Some(session) =
            cx.update_global(|app: &mut AppStore, _| app.session_manager.get(&session_file))
        {
            return session;
        }

        let thread_id = thread.map(|t| t.id.clone());
        let model = thread.and_then(|t| t.model.clone()).or(default_model);
        let thinking_level = thread
            .and_then(|t| t.thinking_level.clone())
            .or(default_thinking_level);
        let store = cx.global::<AppStore>().store.clone();
        let restore_history = thread.is_some();

        let handle = cx.new(|cx| {
            SessionHandle::new(
                cx,
                thread_id,
                session_file.clone(),
                workspace,
                model,
                thinking_level,
                store,
                restore_history,
            )
        });

        cx.update_global(|app: &mut AppStore, _| {
            app.session_manager.register(session_file, handle.clone());
        });

        handle
    }

    fn ensure_session(&mut self, cx: &mut Context<Self>) -> bool {
        if self.session.is_some() {
            return true;
        }
        let workspace_info = self
            .selected_workspace_id
            .as_ref()
            .and_then(|id| self.workspaces.iter().find(|ws| ws.id == *id))
            .map(|ws| WorkspaceInfo {
                id: ws.id.clone(),
                path: PathBuf::from(&ws.path),
                name: ws.name.clone(),
            });
        let model = self
            .chat_input
            .read(cx)
            .selected_model()
            .map(|s| s.to_string());
        let thinking_level = self
            .chat_input
            .read(cx)
            .thinking_level()
            .map(|s| s.to_string());
        let session = Self::get_or_create_session(None, workspace_info, model, thinking_level, cx);
        self.attach_session(session, cx);
        self.session.is_some()
    }

    pub fn send_message(&mut self, _: &SendMessage, _window: &mut Window, cx: &mut Context<Self>) {
        if self.chat_input.read(cx).is_just_selected_mention() {
            self.chat_input
                .update(cx, |ci, _| ci.clear_just_selected_mention());
            return;
        }
        if self.chat_input.read(cx).is_popup_visible() {
            return;
        }
        // When inline-editing a message, read from the inline input instead of
        // the bottom chat input.
        let content = if self.editing_message_id.is_some() {
            self.inline_edit_input
                .as_ref()
                .map(|i| i.read(cx).content(cx).clone())
                .unwrap_or_else(|| self.chat_input.read(cx).content(cx).clone())
        } else {
            self.chat_input.read(cx).content(cx).clone()
        };
        eprintln!("[mini-pi] send_message: {} chars", content.len());
        let has_attachment = !self.chat_input.read(cx).pending_attachments().is_empty();
        if content.is_empty() && !has_attachment {
            return;
        }

        // Handle editing an existing user message: fork from it and send the
        // edited prompt into the new branch. Attachments are not carried over
        // when editing; discard them to keep the flow simple.
        if let Some(editing_id) = self.editing_message_id.take() {
            self.chat_input.update(cx, |ci, cx| ci.reset(_window, cx));
            let Some(edit_idx) = self.messages.iter().position(|m| m.id == editing_id) else {
                eprintln!("[mini-pi] edited message {} not found", editing_id);
                self.clear_inline_edit_state(cx);
                return;
            };
            if !matches!(self.messages[edit_idx].role, Role::User) {
                eprintln!("[mini-pi] cannot edit non-user message");
                self.clear_inline_edit_state(cx);
                return;
            }
            // Update the edited message text.
            self.messages[edit_idx].parts = vec![MessagePart::Text {
                text: content.clone(),
                state: Some(PartState::Done),
            }];
            // Truncate any messages that came after the edited one.
            self.messages.truncate(edit_idx + 1);
            // Clear stale display entities.
            self.reasoning_displays.truncate(edit_idx + 1);
            self.markdown_displays.truncate(edit_idx + 1);
            self.streaming_md_pos = None;
            self.clear_inline_edit_state(cx);
            self.send_edited_prompt(editing_id, content, cx);
            return;
        }

        if !self.ensure_session(cx) {
            return;
        }

        self.chat_input.update(cx, |ci, cx| ci.reset(_window, cx));
        self.scroll_locked = true;
        let attachments = self
            .chat_input
            .update(cx, |ci, _cx| ci.take_pending_attachments());

        let mut media: Vec<ImageContent> = Vec::new();
        let mut file_parts: Vec<String> = Vec::new();
        for attachment in attachments {
            match attachment {
                PendingAttachment::Image {
                    mime_type, base64, ..
                } => {
                    media.push(ImageContent {
                        data: base64,
                        mime_type,
                    });
                }
                PendingAttachment::Text { content, path, .. } => {
                    file_parts.push(format!(
                        "<file path=\"{}\">\n{}\n</file>",
                        path.to_string_lossy(),
                        content
                    ));
                }
            }
        }

        let combined_content: SharedString = if file_parts.is_empty() {
            content
        } else {
            let files_block = file_parts.join("\n\n");
            if content.is_empty() {
                files_block
            } else {
                format!("{}\n\n{}", content, files_block)
            }
            .into()
        };

        let session = self.session.clone().unwrap();
        if !media.is_empty() {
            session.update(cx, |session, cx| {
                session.send_message_with_media(combined_content, media, cx);
            });
        } else {
            session.update(cx, |session, cx| {
                session.send_message(combined_content, cx);
            });
        }

        let thread_id = session.read(cx).thread_id.clone();
        if let Some(ref tid) = thread_id {
            if self.thread_id.is_none() {
                self.thread_id = Some(tid.clone());
            }
            cx.update_global(|app: &mut AppStore, _| {
                app.set_thread_streaming(tid.clone(), true);
            });
        }

        cx.notify();
    }

    pub fn stop_streaming(
        &mut self,
        _: &StopStreaming,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.state, ChatState::Streaming) {
            return;
        }

        eprintln!("[mini-pi] stop_streaming requested");

        if let Some(ref session) = self.session {
            session.update(cx, |session, cx| {
                session.abort(cx);
            });
        }

        self.state = ChatState::Idle;
        if let Some(ref tid) = self.thread_id {
            cx.update_global(|app: &mut AppStore, _| {
                app.set_thread_streaming(tid.clone(), false);
            });
        }

        cx.notify();
    }

    fn send_edited_prompt(
        &mut self,
        message_id: String,
        content: SharedString,
        cx: &mut Context<Self>,
    ) {
        if !self.ensure_session(cx) {
            return;
        }

        let Some(edit_idx) = self.messages.iter().position(|m| m.id == message_id) else {
            eprintln!("[mini-pi] edited message {} not found", message_id);
            return;
        };

        self.messages[edit_idx].parts = vec![MessagePart::Text {
            text: content.clone(),
            state: Some(PartState::Done),
        }];
        self.messages.truncate(edit_idx + 1);
        self.reasoning_displays.truncate(edit_idx + 1);
        self.markdown_displays.truncate(edit_idx + 1);
        self.streaming_md_pos = None;
        self.scroll_locked = true;

        let session = self.session.clone().unwrap();
        session.update(cx, |session, cx| {
            session.send_edited_prompt(message_id, content, cx);
        });

        let thread_id = session.read(cx).thread_id.clone();
        if let Some(ref tid) = thread_id {
            if self.thread_id.is_none() {
                self.thread_id = Some(tid.clone());
            }
            cx.update_global(|app: &mut AppStore, _| {
                app.set_thread_streaming(tid.clone(), true);
            });
        }

        cx.notify();
    }

    pub fn confirm_inline_edit(
        &mut self,
        _: &ConfirmInlineEdit,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editing_id) = self.editing_message_id.take() else {
            self.clear_inline_edit_state(cx);
            return;
        };
        let Some(edit_idx) = self.messages.iter().position(|m| m.id == editing_id) else {
            eprintln!("[mini-pi] inline edited message {} not found", editing_id);
            self.clear_inline_edit_state(cx);
            return;
        };
        if !matches!(self.messages[edit_idx].role, Role::User) {
            eprintln!("[mini-pi] cannot edit non-user message");
            self.clear_inline_edit_state(cx);
            return;
        }
        let content = self
            .inline_edit_input
            .as_ref()
            .map(|i| i.read(cx).content(cx).clone())
            .unwrap_or_default();
        if content.is_empty() {
            self.clear_inline_edit_state(cx);
            return;
        }
        let entry_id = self.messages[edit_idx].id.clone();
        self.messages[edit_idx].parts = vec![MessagePart::Text {
            text: content.clone(),
            state: Some(PartState::Done),
        }];
        self.messages.truncate(edit_idx + 1);
        self.reasoning_displays.truncate(edit_idx + 1);
        self.markdown_displays.truncate(edit_idx + 1);
        self.streaming_md_pos = None;
        self.clear_inline_edit_state(cx);
        self.send_edited_prompt(entry_id, content, cx);
    }

    pub fn cancel_inline_edit(
        &mut self,
        _: &CancelInlineEdit,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_inline_edit_state(cx);
    }

    /// Rename the current thread title, persisting to the store and session.
    pub fn rename_thread(&mut self, new_title: &str, cx: &mut Context<Self>) {
        let title: SharedString = if new_title.is_empty() {
            "New Thread".into()
        } else {
            new_title.into()
        };
        self.title = title.clone();
        if let Some(ref session) = self.session {
            session.update(cx, |session, _cx| {
                session.title = title.clone();
            });
        }
        if let Some(ref tid) = self.thread_id {
            let _ = self.store.update_thread(
                tid,
                Some(&title),
                None,
                None,
                None,
                None,
                None,
                None,
                true,
            );
        }
        cx.update_global(|_: &mut AppStore, _| {});
        cx.notify();
    }

    fn clear_inline_edit_state(&mut self, cx: &mut Context<Self>) {
        self.editing_message_id = None;
        self.inline_edit_input = None;
        cx.notify();
    }

    // ------------------------------------------------------------------
    // Extension UI
    // ------------------------------------------------------------------

    fn handle_extension_ui_request(
        &mut self,
        id: String,
        method: String,
        payload: serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match method.as_str() {
            "select" => {
                let title = payload
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("Select")
                    .to_string();
                let options: Vec<SharedString> = payload
                    .get("options")
                    .and_then(|o| o.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|o| o.as_str().map(SharedString::from))
                            .collect()
                    })
                    .unwrap_or_default();
                if options.is_empty() {
                    self.extension_select_selected = None;
                    if let Some(ref session) = self.session {
                        session.update(cx, |s, cx| {
                            s.respond_extension_ui(
                                &id,
                                &serde_json::json!({ "cancelled": true }),
                                cx,
                            );
                        });
                    }
                    return;
                }

                self.extension_select_selected = Some(0);

                let session = self.session.clone();
                let answered = std::rc::Rc::new(std::cell::Cell::new(false));
                let options_for_content = options.clone();
                let options_for_ok = options.clone();
                let view = cx.entity();

                let dialog_title: SharedString = title.into();
                window.open_dialog(cx, move |dialog, _, _| {
                    let header_title = dialog_title.clone();
                    let content_options = options_for_content.clone();
                    let content_view = view.clone();
                    let ok_view = view.clone();
                    let cancel_view = view.clone();
                    let ok_session = session.clone();
                    let ok_id = id.clone();
                    let ok_answered = answered.clone();
                    let cancel_session = session.clone();
                    let cancel_id = id.clone();
                    let cancel_answered = answered.clone();

                    let ok_options = options_for_ok.clone();
                    dialog
                        .overlay(true)
                        .overlay_closable(true)
                        .close_button(true)
                        .keyboard(true)
                        .content(move |content, _, cx| {
                            let selected =
                                content_view.read(cx).extension_select_selected.unwrap_or(0);
                            content
                                .p_4()
                                .gap_3()
                                .child(
                                    DialogHeader::new()
                                        .child(DialogTitle::new().child(header_title.clone())),
                                )
                                .child(v_flex().gap_2().children(
                                    content_options.iter().enumerate().map(|(idx, opt)| {
                                        let view = content_view.clone();
                                        Toggle::new(format!("ext-select-opt-{}", idx))
                                            .label(opt.clone())
                                            .checked(idx == selected)
                                            .outline()
                                            .on_click(move |_, _, cx| {
                                                view.update(cx, |this, cx| {
                                                    this.extension_select_selected = Some(idx);
                                                    cx.notify();
                                                });
                                            })
                                    }),
                                ))
                                .child(
                                    DialogFooter::new()
                                        .child(
                                            DialogClose::new().child(
                                                Button::new("cancel").outline().label("Cancel"),
                                            ),
                                        )
                                        .child(
                                            DialogAction::new().child(
                                                Button::new("confirm").primary().label("OK"),
                                            ),
                                        ),
                                )
                        })
                        .on_ok(move |_, _window, cx| {
                            if !ok_answered.replace(true)
                                && let Some(session) = ok_session.as_ref()
                            {
                                let selected =
                                    ok_view.read(cx).extension_select_selected.unwrap_or(0);
                                let response = ok_options
                                    .get(selected)
                                    .map(|v| serde_json::json!({ "value": v }))
                                    .unwrap_or_else(|| serde_json::json!({ "cancelled": true }));
                                session.update(cx, |s, cx| {
                                    s.respond_extension_ui(&ok_id, &response, cx);
                                });
                            }
                            ok_view.update(cx, |this, cx| {
                                this.extension_select_selected = None;
                                cx.notify();
                            });
                            true
                        })
                        .on_cancel(move |_, _window, cx| {
                            if !cancel_answered.replace(true)
                                && let Some(session) = cancel_session.as_ref()
                            {
                                session.update(cx, |s, cx| {
                                    s.respond_extension_ui(
                                        &cancel_id,
                                        &serde_json::json!({ "cancelled": true }),
                                        cx,
                                    );
                                });
                            }
                            cancel_view.update(cx, |this, cx| {
                                this.extension_select_selected = None;
                                cx.notify();
                            });
                            true
                        })
                });
            }
            "confirm" => {
                let title = payload
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("Confirm")
                    .to_string();
                let message = payload
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("")
                    .to_string();
                let session = self.session.clone();
                // Shared one-shot guard so the response is sent exactly once,
                // whichever close path fires first (OK / Cancel / Esc).
                let answered = std::rc::Rc::new(std::cell::Cell::new(false));
                window.open_alert_dialog(cx, move |alert, _window, _cx| {
                    let ok_session = session.clone();
                    let ok_id = id.clone();
                    let ok_answered = answered.clone();
                    let cancel_session = session.clone();
                    let cancel_id = id.clone();
                    let cancel_answered = answered.clone();
                    alert
                        .title(title.clone())
                        .description(message.clone())
                        .show_cancel(true)
                        .on_ok(move |_, _window, cx| {
                            if !ok_answered.replace(true)
                                && let Some(session) = ok_session.as_ref()
                            {
                                session.update(cx, |s, cx| {
                                    s.respond_extension_ui(
                                        &ok_id,
                                        &serde_json::json!({ "confirmed": true }),
                                        cx,
                                    );
                                });
                            }
                            true
                        })
                        .on_cancel(move |_, _window, cx| {
                            if !cancel_answered.replace(true)
                                && let Some(session) = cancel_session.as_ref()
                            {
                                session.update(cx, |s, cx| {
                                    s.respond_extension_ui(
                                        &cancel_id,
                                        &serde_json::json!({ "cancelled": true }),
                                        cx,
                                    );
                                });
                            }
                            true
                        })
                });
            }
            "input" => {
                let title = payload
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("Input")
                    .to_string();
                let placeholder = payload
                    .get("placeholder")
                    .and_then(|p| p.as_str())
                    .unwrap_or("")
                    .to_string();

                let session = self.session.clone();
                let answered = std::rc::Rc::new(std::cell::Cell::new(false));
                let input_state = cx.new(|cx| {
                    let mut state = InputState::new(window, cx);
                    if !placeholder.is_empty() {
                        state = state.placeholder(placeholder);
                    }
                    state
                });
                input_state.update(cx, |state, cx| {
                    state.focus(window, cx);
                });

                let dialog_title: SharedString = title.into();
                window.open_dialog(cx, move |dialog, _, _| {
                    let header_title = dialog_title.clone();
                    let content_state = input_state.clone();
                    let ok_state = input_state.clone();
                    let ok_session = session.clone();
                    let ok_id = id.clone();
                    let ok_answered = answered.clone();
                    let cancel_session = session.clone();
                    let cancel_id = id.clone();
                    let cancel_answered = answered.clone();

                    dialog
                        .overlay(true)
                        .overlay_closable(true)
                        .close_button(true)
                        .keyboard(true)
                        .content(move |content, _, _| {
                            content
                                .p_4()
                                .gap_3()
                                .child(
                                    DialogHeader::new()
                                        .child(DialogTitle::new().child(header_title.clone())),
                                )
                                .child(Input::new(&content_state).w_full())
                                .child(
                                    DialogFooter::new()
                                        .child(
                                            DialogClose::new().child(
                                                Button::new("cancel").outline().label("Cancel"),
                                            ),
                                        )
                                        .child(
                                            DialogAction::new().child(
                                                Button::new("confirm").primary().label("OK"),
                                            ),
                                        ),
                                )
                        })
                        .on_ok(move |_, _window, cx| {
                            if !ok_answered.replace(true)
                                && let Some(session) = ok_session.as_ref()
                            {
                                let value = ok_state.read(cx).value();
                                session.update(cx, |s, cx| {
                                    s.respond_extension_ui(
                                        &ok_id,
                                        &serde_json::json!({ "value": value }),
                                        cx,
                                    );
                                });
                            }
                            true
                        })
                        .on_cancel(move |_, _window, cx| {
                            if !cancel_answered.replace(true)
                                && let Some(session) = cancel_session.as_ref()
                            {
                                session.update(cx, |s, cx| {
                                    s.respond_extension_ui(
                                        &cancel_id,
                                        &serde_json::json!({ "cancelled": true }),
                                        cx,
                                    );
                                });
                            }
                            true
                        })
                });
            }
            "editor" => {
                let title = payload
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("Editor")
                    .to_string();
                let prefill = payload
                    .get("prefill")
                    .and_then(|p| p.as_str())
                    .unwrap_or("")
                    .to_string();

                let session = self.session.clone();
                let answered = std::rc::Rc::new(std::cell::Cell::new(false));
                let input_state = cx.new(|cx| {
                    let mut state = InputState::new(window, cx).auto_grow(3, 20);
                    if !prefill.is_empty() {
                        state.set_value(prefill, window, cx);
                    }
                    state
                });
                input_state.update(cx, |state, cx| {
                    state.focus(window, cx);
                });

                let dialog_title: SharedString = title.into();
                window.open_dialog(cx, move |dialog, _, _| {
                    let header_title = dialog_title.clone();
                    let content_state = input_state.clone();
                    let ok_state = input_state.clone();
                    let ok_session = session.clone();
                    let ok_id = id.clone();
                    let ok_answered = answered.clone();
                    let cancel_session = session.clone();
                    let cancel_id = id.clone();
                    let cancel_answered = answered.clone();

                    dialog
                        .overlay(true)
                        .overlay_closable(true)
                        .close_button(true)
                        .keyboard(true)
                        .content(move |content, _, _| {
                            content
                                .p_4()
                                .gap_3()
                                .child(
                                    DialogHeader::new()
                                        .child(DialogTitle::new().child(header_title.clone())),
                                )
                                .child(Input::new(&content_state).w_full())
                                .child(
                                    DialogFooter::new()
                                        .child(
                                            DialogClose::new().child(
                                                Button::new("cancel").outline().label("Cancel"),
                                            ),
                                        )
                                        .child(
                                            DialogAction::new().child(
                                                Button::new("confirm").primary().label("OK"),
                                            ),
                                        ),
                                )
                        })
                        .on_ok(move |_, _window, cx| {
                            if !ok_answered.replace(true)
                                && let Some(session) = ok_session.as_ref()
                            {
                                let value = ok_state.read(cx).value();
                                session.update(cx, |s, cx| {
                                    s.respond_extension_ui(
                                        &ok_id,
                                        &serde_json::json!({ "value": value }),
                                        cx,
                                    );
                                });
                            }
                            true
                        })
                        .on_cancel(move |_, _window, cx| {
                            if !cancel_answered.replace(true)
                                && let Some(session) = cancel_session.as_ref()
                            {
                                session.update(cx, |s, cx| {
                                    s.respond_extension_ui(
                                        &cancel_id,
                                        &serde_json::json!({ "cancelled": true }),
                                        cx,
                                    );
                                });
                            }
                            true
                        })
                });
            }
            "notify" => {
                let message = payload
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("")
                    .to_string();
                let notify_type = payload
                    .get("notifyType")
                    .and_then(|t| t.as_str())
                    .unwrap_or("info");
                let notification = match notify_type {
                    "warning" => Notification::warning(message),
                    "error" => Notification::error(message),
                    _ => Notification::info(message),
                };
                window.push_notification(notification, cx);
            }
            "setEditorText" => {
                let text = payload
                    .get("text")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                self.chat_input
                    .update(cx, |ci, cx| ci.set_content(text, window, cx));
            }
            "setStatus" => {
                let key = payload
                    .get("statusKey")
                    .and_then(|k| k.as_str())
                    .unwrap_or("")
                    .to_string();
                if let Some(text) = payload.get("statusText").and_then(|t| t.as_str()) {
                    self.extension_status.insert(key, SharedString::from(text));
                } else {
                    self.extension_status.remove(&key);
                }
            }
            "setWidget" => {
                let key = payload
                    .get("widgetKey")
                    .and_then(|k| k.as_str())
                    .unwrap_or("")
                    .to_string();
                if let Some(lines_arr) = payload.get("widgetLines").and_then(|w| w.as_array()) {
                    let lines: Vec<SharedString> = lines_arr
                        .iter()
                        .filter_map(|l| l.as_str().map(SharedString::from))
                        .collect();
                    let placement = payload
                        .get("widgetPlacement")
                        .and_then(|p| p.as_str())
                        .unwrap_or("aboveEditor")
                        .to_string();
                    self.extension_widgets.insert(key, (lines, placement));
                } else {
                    self.extension_widgets.remove(&key);
                }
            }
            "setTitle" => {
                let title = payload
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                if !title.is_empty() {
                    self.title = SharedString::from(title);
                }
            }
            _ => {
                eprintln!(
                    "[mini-pi] unknown extension UI method: {}, cancelling",
                    method
                );
                if let Some(ref session) = self.session {
                    session.update(cx, |s, cx| {
                        s.respond_extension_ui(&id, &serde_json::json!({ "cancelled": true }), cx);
                    });
                }
            }
        }
    }

    fn start_edit_message(&mut self, msg_id: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(msg) = self.messages.iter().find(|m| m.id == msg_id)
            && let Some(MessagePart::Text { text, .. }) = msg.parts.first()
        {
            self.editing_message_id = Some(msg_id);
            let text = text.clone();
            let inline_input = cx.new(|cx| {
                ChatInput::new(window, cx, "Edit message...")
                    .with_at_mention(false)
                    .with_slash_commands(false)
            });
            inline_input.update(cx, |ci, cx| {
                ci.set_content(text, window, cx);
                ci.focus(window, cx);
            });
            self.inline_edit_input = Some(inline_input);
            cx.notify();
        }
    }

    fn render_messages_scrollbar(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .child(Scrollbar::vertical(&self.scroll_handle))
    }

    fn render_scroll_to_bottom_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("scroll-to-bottom-btn")
            .absolute()
            .bottom(px(12.))
            .left(px(0.))
            .right(px(0.))
            .flex()
            .items_center()
            .justify_center()
            .when(!self.scroll_locked, |this| {
                this.child(
                    div()
                        .rounded_full()
                        .bg(cx.theme().popover)
                        .border_1()
                        .border_color(cx.theme().border)
                        .shadow(vec![gpui::BoxShadow {
                            color: cx.theme().overlay,
                            offset: gpui::point(px(0.), px(4.)),
                            blur_radius: px(12.),
                            spread_radius: px(0.),
                            inset: false,
                        }])
                        .child(
                            Button::new("scroll-to-bottom")
                                .with_size(Size::Small)
                                .ghost()
                                .icon(
                                    Icon::empty()
                                        .path("icons/chevron-down.svg")
                                        .size(px(16.))
                                        .text_color(cx.theme().muted_foreground),
                                )
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.scroll_locked = true;
                                    this.scroll_handle.scroll_to_bottom();
                                    cx.notify();
                                })),
                        ),
                )
            })
    }

    /// Resolve the active workspace directory, used by `ToolCall` to resolve
    /// relative `send_file` paths. Returns `None` when no workspace is
    /// selected.
    fn workspace_dir_for_send_file(&self) -> Option<PathBuf> {
        self.selected_workspace_id
            .as_ref()
            .and_then(|id| self.workspaces.iter().find(|ws| ws.id == *id))
            .map(|ws| PathBuf::from(&ws.path))
    }

    /// Sync the reasoning and markdown display entities with the current
    /// messages.  Call this whenever the message list changes (e.g. from
    /// `sync_from_session`) rather than from inside `render()` so entity
    /// creation does not happen on every frame.
    ///
    /// During streaming the markdown entity for the last (streaming) text part
    /// is deliberately skipped — markdown parsing is too expensive to run on
    /// every token.  The render path falls back to plain text for streaming
    /// parts, and the full `TextViewState` is created once the part reaches
    /// `Done`.
    fn sync_display_entities(&mut self, cx: &mut Context<Self>) {
        // --- reasoning displays ---
        for (msg_idx, msg) in self.messages.iter().enumerate() {
            let part_count = msg.parts.len();
            if let Some(row) = self.reasoning_displays.get_mut(msg_idx) {
                row.truncate(part_count);
            }
            for (part_idx, part) in msg.parts.iter().enumerate() {
                if let MessagePart::Reasoning { text, state, .. } = part {
                    if msg_idx >= self.reasoning_displays.len() {
                        self.reasoning_displays
                            .resize_with(msg_idx + 1, std::vec::Vec::new);
                    }
                    let row = &mut self.reasoning_displays[msg_idx];
                    if part_idx >= row.len() {
                        row.resize_with(part_idx + 1, || None);
                    }
                    let reasoning_state = state.clone();
                    if let Some(Some(existing)) = row.get(part_idx) {
                        existing.update(cx, |display, _cx| {
                            display.set_content(text, reasoning_state);
                        });
                    } else {
                        let new = cx.new(|_cx| {
                            Reasoning::new(
                                format!("{}-{}", msg_idx, part_idx),
                                text,
                                reasoning_state,
                            )
                        });
                        row[part_idx] = Some(new);
                    }
                } else {
                    if let Some(row) = self.reasoning_displays.get_mut(msg_idx)
                        && part_idx < row.len()
                    {
                        row[part_idx] = None;
                    }
                }
            }
        }
        self.reasoning_displays.truncate(self.messages.len());

        // --- markdown displays (assistant text parts only) ---
        // Uses push_str for incremental streaming so markdown is only
        // re-parsed for new tokens rather than from scratch on every event.
        for (msg_idx, msg) in self.messages.iter().enumerate() {
            let is_assistant = matches!(msg.role, Role::Assistant);
            let part_count = msg.parts.len();
            if let Some(row) = self.markdown_displays.get_mut(msg_idx) {
                row.truncate(part_count);
                if !is_assistant {
                    row.clear();
                }
            }
            for (part_idx, part) in msg.parts.iter().enumerate() {
                if is_assistant && matches!(part, MessagePart::Text { .. }) {
                    if let MessagePart::Text { text, state, .. } = part {
                        if msg_idx >= self.markdown_displays.len() {
                            self.markdown_displays
                                .resize_with(msg_idx + 1, std::vec::Vec::new);
                        }
                        let row = &mut self.markdown_displays[msg_idx];
                        if part_idx >= row.len() {
                            row.resize_with(part_idx + 1, || None);
                        }

                        // Only one message streams at a time.  Track its
                        // (msg,part,len) so we can push_str just the delta.
                        let is_streaming = matches!(state, Some(PartState::Streaming));
                        let same_pos = self
                            .streaming_md_pos
                            .map_or(false, |(m, p, _)| m == msg_idx && p == part_idx);
                        let old_len = self
                            .streaming_md_pos
                            .and_then(|(m, p, len)| (m == msg_idx && p == part_idx).then_some(len))
                            .unwrap_or(0);

                        if let Some(Some(existing)) = row.get(part_idx) {
                            if same_pos && text.len() > old_len {
                                // Continuing stream — push only the delta.
                                let delta: &str = &text[old_len..];
                                existing.update(cx, |display, cx| {
                                    display.push_str(delta, cx);
                                });
                            } else {
                                // New or non-streaming part — rebuild.
                                existing.update(cx, |display, cx| {
                                    display.set_text(text, cx);
                                });
                            }
                        } else {
                            // First time — parse full text.
                            let new = cx.new(|cx| TextViewState::markdown(text, cx));
                            row[part_idx] = Some(new);
                        }

                        self.streaming_md_pos = if is_streaming {
                            Some((msg_idx, part_idx, text.len()))
                        } else {
                            None
                        };
                    }
                } else {
                    if let Some(row) = self.markdown_displays.get_mut(msg_idx)
                        && part_idx < row.len()
                    {
                        row[part_idx] = None;
                    }
                }
            }
        }
        self.markdown_displays.truncate(self.messages.len());
    }

    fn render_workspace_selector(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .pb_1()
            .flex()
            .flex_row()
            .gap_1()
            .items_center()
            .child(
                Button::new("manage-workspace-btn")
                    .with_size(Size::XSmall)
                    .compact()
                    .secondary()
                    .icon(
                        Icon::empty()
                            .path("icons/manage.svg")
                            .size(px(10.))
                            .text_color(cx.theme().muted_foreground),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_workspace_manager(window, cx);
                    })),
            )
            .children(self.workspaces.iter().map(|ws| {
                let is_selected = self.selected_workspace_id == Some(ws.id.clone());
                let ws_id = ws.id.clone();
                let ws_name = ws.name.clone();
                let name: SharedString = ws_name.clone().into();
                let bg = workspace_color(&ws.name);
                let fg = workspace_foreground(bg);
                let tag = if is_selected {
                    Tag::custom(bg, fg, bg)
                } else {
                    Tag::custom(bg.opacity(0.5), fg, bg.opacity(0.5))
                };
                div()
                    .cursor_pointer()
                    .child(tag.with_size(Size::XSmall).child(name))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.selected_workspace_id = Some(ws_id.clone());
                            let ws_dir = this
                                .workspaces
                                .iter()
                                .find(|w| w.id == ws_id)
                                .map(|w| PathBuf::from(&w.path));
                            if let Some(dir) = ws_dir {
                                this.chat_input.update(cx, |ci, cx| {
                                    ci.set_workspace(ws_id.clone(), dir, ws_name.clone(), cx);
                                });
                            }
                            cx.notify();
                        }),
                    )
            }))
    }

    #[allow(clippy::too_many_arguments)]
    fn render_messages(
        &mut self,
        cx: &mut Context<Self>,
        reasoning_entities: &ReasoningEntities,
        markdown_entities: &MarkdownEntities,
        assistant_text_width: Pixels,
        status: Option<SharedString>,
        is_error: bool,
        is_loading: bool,
        is_streaming: bool,
    ) -> impl IntoElement {
        div()
            .id("messages")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .on_scroll_wheel(
                cx.listener(|this, event: &gpui::ScrollWheelEvent, window, cx| {
                    let delta = event.delta.pixel_delta(window.line_height());
                    if this.scroll_locked && delta.y > gpui::px(0.) {
                        this.scroll_locked = false;
                    }
                    if !this.scroll_locked {
                        let offset_y = this.scroll_handle.offset().y;
                        let max_y = this.scroll_handle.max_offset().y;
                        if offset_y.abs() >= max_y - gpui::px(5.) {
                            this.scroll_locked = true;
                        }
                    }
                    cx.notify();
                }),
            )
            .flex()
            .flex_col()
            .p_3()
            .pr_4()
            .gap_2()
            .children(self.messages.iter().enumerate().map(|(msg_idx, msg)| {
                let msg_reasoning = reasoning_entities.get(msg_idx).cloned().unwrap_or_default();
                let msg_markdown = markdown_entities.get(msg_idx).cloned().unwrap_or_default();
                self.render_message(
                    cx,
                    msg_idx,
                    msg,
                    msg_reasoning,
                    msg_markdown,
                    assistant_text_width,
                )
            }))
            .when(self.messages.is_empty(), |el| {
                el.items_center().justify_center().child(
                    svg()
                        .path("icons/pi.svg")
                        .text_color(cx.theme().muted)
                        .size(px(180.)),
                )
            })
            .when(is_loading, |el| {
                el.child(
                    div().flex().justify_center().child(
                        div()
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(cx.theme().secondary)
                            .text_color(cx.theme().muted_foreground)
                            .text_xs()
                            .child(loader()),
                    ),
                )
            })
            .when(is_streaming, |el| {
                el.child(
                    div()
                        .flex()
                        .w_full()
                        .px_3()
                        .py_1()
                        .text_color(cx.theme().muted_foreground)
                        .text_xs()
                        .child(text_loader()),
                )
            })
            .when(is_error, |el| {
                el.child(
                    div().flex().justify_center().child(
                        div()
                            .w_full()
                            .max_w(px(520.))
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(cx.theme().danger)
                            .text_color(cx.theme().danger_foreground)
                            .text_xs()
                            .whitespace_normal()
                            .child(status.unwrap_or_default()),
                    ),
                )
            })
    }

    fn render_message(
        &self,
        cx: &mut Context<Self>,
        msg_idx: usize,
        msg: &Message,
        msg_reasoning: Vec<Option<Entity<Reasoning>>>,
        msg_markdown: Vec<Option<Entity<TextViewState>>>,
        assistant_text_width: Pixels,
    ) -> impl IntoElement + use<> {
        let is_user = matches!(msg.role, Role::User);
        let msg_id = msg.id.clone();
        let media_element = if is_user && !msg.media.is_empty() {
            Some(self.render_message_media(cx, &msg.media))
        } else {
            None
        };
        div().block().w_full().child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .when(is_user, |this| this.items_end())
                .when(!is_user, |this| this.items_start())
                .gap_1()
                .children({
                    let n = msg.parts.len();
                    let mut paired: Vec<bool> = vec![false; n];
                    let mut pairs: Vec<(usize, usize)> = Vec::new();

                    // First pass: pair each ToolCall with the earliest unpaired
                    // ToolResult that follows it. Loaded sessions often place all
                    // tool calls before all tool results in the same message.
                    for i in 0..n {
                        if matches!(msg.parts[i], MessagePart::ToolCall { .. }) && !paired[i] {
                            if let Some(j) = (i + 1..n).find(|&k| {
                                matches!(msg.parts[k], MessagePart::ToolResult { .. }) && !paired[k]
                            }) {
                                paired[i] = true;
                                paired[j] = true;
                                pairs.push((i, j));
                            }
                        }
                    }

                    // Second pass: also pair any remaining ToolCall with the
                    // immediately following Text part (some tools emit results as
                    // regular assistant text).
                    for i in 0..n {
                        if matches!(msg.parts[i], MessagePart::ToolCall { .. }) && !paired[i] {
                            if let Some(j) = (i + 1..n).find(|&k| {
                                matches!(msg.parts[k], MessagePart::Text { .. }) && !paired[k]
                            }) {
                                // Only pair if there is no unpaired ToolResult in between.
                                let has_result_between = (i + 1..j).any(|k| {
                                    matches!(msg.parts[k], MessagePart::ToolResult { .. })
                                        && !paired[k]
                                });
                                if !has_result_between {
                                    paired[i] = true;
                                    paired[j] = true;
                                    pairs.push((i, j));
                                }
                            }
                        }
                    }

                    let mut children: Vec<AnyElement> = Vec::new();
                    let mut i = 0;
                    while i < n {
                        if paired[i] {
                            if let Some(&(call_idx, result_idx)) =
                                pairs.iter().find(|(call_idx, _)| *call_idx == i)
                            {
                                if let (
                                    MessagePart::ToolCall {
                                        name, args, state, ..
                                    },
                                    MessagePart::ToolResult {
                                        output, details, ..
                                    },
                                ) = (&msg.parts[call_idx], &msg.parts[result_idx])
                                {
                                    let tool = ToolCall::paired(
                                        name.clone(),
                                        args.clone(),
                                        Some(output.clone()),
                                        details.clone(),
                                        None,
                                        self.workspace_dir_for_send_file(),
                                        assistant_text_width,
                                        state.clone(),
                                    );
                                    children.push(tool.render(cx, msg_idx));
                                } else if let (
                                    MessagePart::ToolCall {
                                        name, args, state, ..
                                    },
                                    MessagePart::Text { text, .. },
                                ) = (&msg.parts[call_idx], &msg.parts[result_idx])
                                {
                                    let markdown_entity =
                                        msg_markdown.get(result_idx).and_then(|e| e.clone());
                                    let tool = ToolCall::paired(
                                        name.clone(),
                                        args.clone(),
                                        Some(text.clone()),
                                        None,
                                        markdown_entity,
                                        self.workspace_dir_for_send_file(),
                                        assistant_text_width,
                                        state.clone(),
                                    );
                                    children.push(tool.render(cx, msg_idx));
                                }
                                i = result_idx + 1;
                                continue;
                            }
                            // This index is a paired result; skip it.
                            i += 1;
                            continue;
                        }

                        let markdown_entity = msg_markdown.get(i).and_then(|e| e.clone());
                        let reasoning_entity = msg_reasoning.get(i).and_then(|e| e.clone());
                        children.push(self.render_message_part(
                            cx,
                            msg_idx,
                            i,
                            &msg.parts[i],
                            markdown_entity,
                            reasoning_entity,
                            assistant_text_width,
                            is_user,
                            msg_id.clone(),
                        ));
                        i += 1;
                    }
                    children
                })
                .when_some(media_element, |this, el| this.child(el)),
        )
    }

    fn render_message_media(&self, cx: &mut Context<Self>, media: &[ImageContent]) -> AnyElement {
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .justify_end()
            .gap_2()
            .children(
                media
                    .iter()
                    .enumerate()
                    .map(|(idx, item)| self.render_media_item(cx, idx, item)),
            )
            .into_any_element()
    }

    fn render_media_item(
        &self,
        cx: &mut Context<Self>,
        idx: usize,
        item: &ImageContent,
    ) -> AnyElement {
        self.render_image_media_item(cx, idx, item)
    }

    fn render_image_media_item(
        &self,
        cx: &mut Context<Self>,
        idx: usize,
        item: &ImageContent,
    ) -> AnyElement {
        let source = image_source_from_base64(&item.data, &item.mime_type);
        let hover_source = source.clone();
        let thumb = div()
            .id(SharedString::from(format!(
                "media-image-{}-{}",
                idx, item.mime_type
            )))
            .size(px(64.))
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .overflow_hidden()
            .when_some(source.clone(), |this, source| {
                this.child(img(source).size_full().object_fit(gpui::ObjectFit::Cover))
            })
            .when(source.is_none(), |this| {
                this.flex()
                    .items_center()
                    .justify_center()
                    .bg(cx.theme().muted)
                    .child(
                        Icon::empty()
                            .path("icons/file.svg")
                            .size(px(20.))
                            .text_color(cx.theme().muted_foreground),
                    )
            });

        HoverCard::new(format!("image-hover-{}", idx))
            .anchor(Anchor::TopRight)
            .open_delay(std::time::Duration::from_millis(200))
            .close_delay(std::time::Duration::from_millis(100))
            .trigger(thumb)
            .content(move |_, _, _cx| {
                div()
                    .max_w(px(320.))
                    .max_h(px(240.))
                    .rounded_md()
                    .overflow_hidden()
                    .when_some(hover_source.clone(), |this, source| {
                        this.child(
                            img(source)
                                .max_w(px(320.))
                                .max_h(px(240.))
                                .object_fit(gpui::ObjectFit::Contain),
                        )
                    })
                    .when(hover_source.is_none(), |this| {
                        this.flex()
                            .items_center()
                            .justify_center()
                            .size(px(120.))
                            .child("Image")
                    })
                    .into_any_element()
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_message_part(
        &self,
        cx: &mut Context<Self>,
        msg_idx: usize,
        _part_idx: usize,
        part: &MessagePart,
        markdown_entity: Option<Entity<TextViewState>>,
        reasoning_entity: Option<Entity<Reasoning>>,
        assistant_text_width: Pixels,
        is_user: bool,
        msg_id: String,
    ) -> AnyElement {
        match part {
            MessagePart::Text { text, state } => self.render_text_part(
                cx,
                msg_idx,
                text.clone(),
                state.clone(),
                markdown_entity,
                assistant_text_width,
                is_user,
                msg_id,
            ),
            MessagePart::Reasoning { .. } => div()
                .flex()
                .flex_col()
                .w_full()
                .min_w_0()
                .self_stretch()
                .when(reasoning_entity.is_some(), |this| {
                    this.child(reasoning_entity.unwrap())
                })
                .into_any_element(),
            MessagePart::ToolCall {
                name, args, state, ..
            } => ToolCall::call_only(name.clone(), args.clone(), state.clone()).render(cx, msg_idx),
            MessagePart::ToolResult {
                name,
                output,
                details,
                state,
                ..
            } => ToolCall::result_only(
                name.clone(),
                output.clone(),
                details.clone(),
                state.clone(),
                self.workspace_dir_for_send_file(),
            )
            .render(cx, msg_idx),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_text_part(
        &self,
        cx: &mut Context<Self>,
        msg_idx: usize,
        text: SharedString,
        state: Option<PartState>,
        markdown_entity: Option<Entity<TextViewState>>,
        assistant_text_width: Pixels,
        is_user: bool,
        msg_id: String,
    ) -> AnyElement {
        let is_streaming_empty = state == Some(PartState::Streaming) && text.is_empty();
        let is_editing = is_user && self.editing_message_id.as_ref() == Some(&msg_id);

        if is_editing {
            let inline_input = self
                .inline_edit_input
                .clone()
                .unwrap_or_else(|| self.chat_input.clone());
            return div()
                .flex()
                .flex_col()
                .gap_1()
                .w_full()
                .child(
                    div()
                        .rounded_md()
                        .bg(cx.theme().background)
                        .text_color(cx.theme().foreground)
                        .text_sm()
                        .child(Input::new(&inline_input.read(cx).input_state).w_full()),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("inline-edit-save")
                                .label("Save")
                                .with_size(Size::XSmall)
                                .primary()
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.confirm_inline_edit(&ConfirmInlineEdit, _window, cx);
                                })),
                        )
                        .child(
                            Button::new("inline-edit-cancel")
                                .label("Cancel")
                                .with_size(Size::XSmall)
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.cancel_inline_edit(&CancelInlineEdit, _window, cx);
                                })),
                        ),
                )
                .into_any_element();
        }

        div()
            .flex()
            .flex_col()
            .gap_1()
            .when(!is_user, |this| this.w_full().min_w_0())
            .child(
                div()
                    .py_2()
                    .rounded_md()
                    .when(!is_user, |this| this.w_full().min_w_0())
                    .when(is_user, |this| {
                        this.px_3()
                            .bg(cx.theme().primary)
                            .text_color(cx.theme().primary_foreground)
                    })
                    .when(!is_user, |this| this.text_color(cx.theme().foreground))
                    .text_sm()
                    .when(is_streaming_empty, |this| this.child(text_loader()))
                    .when(!is_streaming_empty, |this| {
                        if let Some(ref md) = markdown_entity {
                            this.child(
                                div().flex().w(assistant_text_width).min_w_0().child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(TextView::new(md).selectable(true).w_full()),
                                ),
                            )
                        } else {
                            this.child(text.clone())
                        }
                    }),
            )
            .when(!is_user && !is_streaming_empty, |this| {
                let copy_text = text.clone();
                this.child(
                    Button::new(("copy-btn", msg_idx as u64))
                        .with_size(Size::XSmall)
                        .ghost()
                        .icon(
                            Icon::empty()
                                .path("icons/clipboard.svg")
                                .size(px(12.))
                                .text_color(cx.theme().muted_foreground),
                        )
                        .on_click(cx.listener(move |_this, _, _window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_text.to_string()));
                        })),
                )
            })
            .when(is_user && !is_streaming_empty, |this| {
                let edit_msg_id = msg_id.clone();
                let text_to_copy = text.clone();
                this.child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new(("copy-btn", msg_idx as u64))
                                .with_size(Size::XSmall)
                                .ghost()
                                .icon(
                                    Icon::empty()
                                        .path("icons/clipboard.svg")
                                        .size(px(12.))
                                        .text_color(cx.theme().muted_foreground),
                                )
                                .on_click(cx.listener(move |_this, _, _window, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        text_to_copy.to_string(),
                                    ));
                                })),
                        )
                        .child(
                            Button::new(("edit-btn", msg_idx as u64))
                                .with_size(Size::XSmall)
                                .ghost()
                                .icon(
                                    Icon::empty()
                                        .path("icons/edit.svg")
                                        .size(px(12.))
                                        .text_color(cx.theme().muted_foreground),
                                )
                                .on_click(cx.listener(move |this, _, _window, cx| {
                                    this.start_edit_message(edit_msg_id.clone(), _window, cx);
                                })),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let stats = self.session_stats.as_ref();
        let total_messages = stats.map(|s| s.total_messages).unwrap_or(0);
        let input_tokens = stats.map(|s| s.tokens_input).unwrap_or(0);
        let output_tokens = stats.map(|s| s.tokens_output).unwrap_or(0);
        let total_tokens = stats.map(|s| s.tokens_total).unwrap_or(0);
        let cache_read = stats.map(|s| s.tokens_cache_read).unwrap_or(0);
        let cache_write = stats.map(|s| s.tokens_cache_write).unwrap_or(0);
        let cost = stats.map(|s| s.cost).unwrap_or(0.0);
        let context_percent = stats.and_then(|s| s.context_percent);

        fn format_tokens(n: usize) -> String {
            if n >= 1_000_000 {
                format!("{:.1}M", n as f64 / 1_000_000.0)
            } else if n >= 1_000 {
                format!("{:.1}k", n as f64 / 1_000.0)
            } else {
                n.to_string()
            }
        }

        let muted = cx.theme().muted_foreground;

        StatusBar::new()
            .left(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::empty()
                            .path("icons/pi.svg")
                            .size(px(10.))
                            .text_color(muted),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("{} messages", total_messages)),
                    )
                    .when(total_tokens > 0, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(format!("{} tokens", format_tokens(total_tokens))),
                        )
                    }),
            )
            .right(
                h_flex()
                    .items_center()
                    .gap_3()
                    .when(input_tokens > 0 || output_tokens > 0, |this| {
                        this.child(div().text_xs().text_color(muted).child(format!(
                            "{} in / {} out",
                            format_tokens(input_tokens),
                            format_tokens(output_tokens)
                        )))
                    })
                    .when(cache_read > 0 || cache_write > 0, |this| {
                        this.child(div().text_xs().text_color(muted).child(format!(
                            "cache {}r / {}w",
                            format_tokens(cache_read),
                            format_tokens(cache_write)
                        )))
                    })
                    .when(cost > 0.0, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(format!("${:.4}", cost)),
                        )
                    })
                    .when_some(context_percent, |this, pct| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(format!("context {:.1}%", pct)),
                        )
                    }),
            )
    }
}

impl Render for ChatWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Process any pending extension UI request (deferred from the session
        // subscription callback where Window is not available).
        if let Some((id, method, payload)) = self.pending_extension_request.take() {
            self.handle_extension_ui_request(id, method, payload, window, cx);
        }

        let status = match &self.state {
            ChatState::Idle => None,
            ChatState::Loading => Some(SharedString::from("Loading...")),
            ChatState::Streaming => Some(SharedString::from("Thinking...")),
            ChatState::Error(msg) => Some(msg.clone()),
        };
        let is_error = matches!(self.state, ChatState::Error(_));
        let is_loading = matches!(self.state, ChatState::Loading);
        let is_streaming = matches!(self.state, ChatState::Streaming);

        // Sync chat state into the chat input so the composer toolbar can
        // render the correct send/stop button and disable attachments while
        // busy.
        self.chat_input.update(cx, |ci, cx| {
            ci.set_chat_state(self.state.clone(), cx);
        });

        // Clone the pre-synced display handles (cheap — Entity is Copy).
        // sync_display_entities is called from sync_from_session whenever
        // the message list changes, not from inside render().
        let reasoning_entities = self.reasoning_displays.clone();
        let markdown_entities = self.markdown_displays.clone();
        let assistant_text_width = (window.viewport_size().width - px(80.)).max(px(320.));

        div()
            .relative()
            .track_focus(&self.focus_handle)
            .on_action(|_: &CloseWindow, window, _| {
                window.remove_window();
            })
            .on_action(cx.listener(Self::send_message))
            .on_action(cx.listener(Self::stop_streaming))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                if event.keystroke.key == "escape" && this.chat_input.read(cx).is_popup_visible() {
                    this.chat_input.update(cx, |ci, cx| ci.close_popup(cx));
                }
            }))
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .bg(cx.theme().background)
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .relative()
                    .child(self.render_messages(
                        cx,
                        &reasoning_entities,
                        &markdown_entities,
                        assistant_text_width,
                        status,
                        is_error,
                        is_loading,
                        is_streaming,
                    ))
                    .child(self.render_messages_scrollbar(cx))
                    .child(self.render_scroll_to_bottom_button(cx)),
            )
            .when(self.session.is_none(), |el| {
                el.child(self.render_workspace_selector(cx))
            })
            .child(self.chat_input.clone())
            .when(self.session.is_some(), |el| {
                el.child(self.render_status_bar(cx))
            })
    }
}
