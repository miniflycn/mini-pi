use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt;
use gpui::prelude::*;
use gpui::{Context, EventEmitter, SharedString, Task};
use uuid::Uuid;

use crate::config::model_config::parse_model_id;
use crate::core::app::AppStore;
use crate::data::models::{ChatState, Message, MessagePart, PartState, Role};
use crate::data::store::Store;
use crate::rpc::pi_rpc::{
    BridgeEvent, ImageContent, LoadedMessage, LoadedPart, PiRpc, is_assistant_error,
};
use crate::ui::chat_input::CommandItem;
use crate::utils::format::truncate_str;
use crate::utils::llm::generate_title;

#[derive(Clone, Debug)]
pub struct WorkspaceInfo {
    pub id: String,
    pub path: PathBuf,
    pub name: String,
}

#[derive(Clone, Debug, Default)]
pub struct SessionStats {
    pub session_file: Option<String>,
    pub session_id: String,
    pub user_messages: usize,
    pub assistant_messages: usize,
    pub tool_calls: usize,
    pub tool_results: usize,
    pub total_messages: usize,
    pub tokens_input: usize,
    pub tokens_output: usize,
    pub tokens_cache_read: usize,
    pub tokens_cache_write: usize,
    pub tokens_total: usize,
    pub cost: f64,
    pub context_tokens: Option<usize>,
    pub context_window: usize,
    pub context_percent: Option<f64>,
}

#[derive(Clone, Debug)]
pub enum SessionEvent {
    Changed,
    ExportHtmlSucceeded {
        path: PathBuf,
    },
    ExtensionUiRequest {
        id: String,
        method: String,
        payload: serde_json::Value,
    },
}

pub struct SessionHandle {
    pub thread_id: Option<String>,
    pub _session_id: String,
    pub session_file: String,
    pub title: SharedString,
    pub messages: Vec<Message>,
    pub state: ChatState,
    pub commands: Vec<CommandItem>,
    pub selected_model: Option<String>,
    pub thinking_level: Option<String>,
    pub workspace: Option<WorkspaceInfo>,
    pub rpc: Option<PiRpc>,
    pub _event_task: Option<Task<()>>,
    pub pending_fork: Option<(String, String)>,
    pub refresh_entry_ids_after_streaming: bool,
    pub pi_restart_count: u32,
    pub session_stats: Option<SessionStats>,
    pub store: Arc<Store>,
}

impl EventEmitter<SessionEvent> for SessionHandle {}

impl SessionHandle {
    pub fn new(
        cx: &mut Context<Self>,
        thread_id: Option<String>,
        session_file: String,
        workspace: Option<WorkspaceInfo>,
        model: Option<String>,
        thinking_level: Option<String>,
        store: Arc<Store>,
        restore_history: bool,
    ) -> Self {
        let session_id = session_file.clone();
        let mut handle = Self {
            thread_id: thread_id.clone(),
            _session_id: session_id,
            session_file,
            title: SharedString::from("New Thread"),
            messages: vec![],
            state: ChatState::Idle,
            commands: vec![],
            selected_model: model.clone(),
            thinking_level: thinking_level.clone(),
            workspace,
            rpc: None,
            _event_task: None,
            pending_fork: None,
            refresh_entry_ids_after_streaming: false,
            pi_restart_count: 0,
            session_stats: None,
            store,
        };

        if let Some(ref tid) = thread_id
            && let Ok(Some(thread)) = handle.store.get_thread(tid)
        {
            handle.title = if thread.title.is_empty() {
                "New Thread".into()
            } else {
                thread.title.into()
            };
        }

        handle.spawn_pi(restore_history, cx);
        handle
    }

    pub fn is_streaming(&self) -> bool {
        matches!(self.state, ChatState::Streaming)
    }

    pub fn has_error(&self) -> bool {
        matches!(self.state, ChatState::Error(_))
    }

    pub fn set_thread_id(&mut self, thread_id: String) {
        self.thread_id = Some(thread_id);
    }

    pub fn set_model(&mut self, model: Option<String>, cx: &mut Context<Self>) {
        self.selected_model = model.clone();
        if let Some(ref id) = model {
            if let Some(ref tid) = self.thread_id {
                let _ = self.store.update_thread(
                    tid,
                    None,
                    None,
                    None,
                    Some(Some(id)),
                    None,
                    None,
                    None,
                    false,
                );
            }
            if let Some(ref mut rpc) = self.rpc
                && let Some((provider, model_id)) = parse_model_id(id)
            {
                eprintln!(
                    "[mini-pi] setting model: provider={} model={}",
                    provider, model_id
                );
                if let Err(e) = rpc.send_set_model(provider, model_id, None) {
                    eprintln!("[mini-pi] send_set_model failed: {}", e);
                }
            }
        }
        cx.emit(SessionEvent::Changed);
    }

    pub fn set_thinking_level(&mut self, level: Option<String>, cx: &mut Context<Self>) {
        self.thinking_level = level.clone();
        if let Some(ref id) = level {
            if let Some(ref tid) = self.thread_id {
                let _ = self.store.update_thread(
                    tid,
                    None,
                    None,
                    None,
                    None,
                    Some(Some(id)),
                    None,
                    None,
                    false,
                );
            }
            if let Some(ref mut rpc) = self.rpc
                && let Err(e) = rpc.send_set_thinking_level(id, None)
            {
                eprintln!("[mini-pi] send_set_thinking_level failed: {}", e);
            }
        }
        cx.emit(SessionEvent::Changed);
    }

    pub fn set_workspace(&mut self, workspace: WorkspaceInfo) {
        self.workspace = Some(workspace);
    }

    pub fn send_message(&mut self, content: SharedString, cx: &mut Context<Self>) {
        eprintln!("[mini-pi] send_message: {} chars", content.len());
        if content.is_empty() {
            return;
        }

        if self.rpc.is_none() && !self.spawn_pi(false, cx) {
            return;
        }

        self.messages.push(Message {
            id: Uuid::new_v4().to_string(),
            entry_id: None,
            role: Role::User,
            parts: vec![MessagePart::Text {
                text: content.clone(),
                state: Some(PartState::Done),
            }],
            media: Vec::new(),
        });

        if self.thread_id.is_none() {
            match self.store.create_thread("", "") {
                Ok(thread) => {
                    let thread_id = thread.id.clone();
                    self.thread_id = Some(thread_id);
                    self.title = content.chars().take(80).collect::<String>().into();
                    let sf = self.session_file.clone();
                    let model_opt = self.selected_model.as_deref();
                    let thinking_opt = self.thinking_level.as_deref();
                    // Persist the selected workspace on the thread so that
                    // reopening it later restores the same cwd used for the
                    // bridge's create_session.
                    let metadata = self
                        .workspace
                        .as_ref()
                        .map(|ws| serde_json::json!({ "workspace_id": ws.id }));
                    let metadata_ref = metadata.as_ref();
                    let _ = self.store.update_thread(
                        &thread.id,
                        Some(&self.title),
                        None,
                        Some(Some(&sf)),
                        Some(model_opt),
                        Some(thinking_opt),
                        None,
                        Some(metadata_ref),
                        false,
                    );
                }
                Err(_) => {
                    self.state = ChatState::Error("Failed to create thread".into());
                    cx.emit(SessionEvent::Changed);
                    return;
                }
            }
        }

        let tid = self.thread_id.as_ref().unwrap();
        let user_count = self
            .messages
            .iter()
            .filter(|m| matches!(m.role, Role::User))
            .count();
        let title = if user_count == 1 {
            let temp_title: String = content.chars().take(80).collect();

            let content_clone = content.clone();
            let weak = cx.entity().downgrade();
            cx.spawn(async move |_, cx| {
                let result = smol::unblock(move || generate_title(&content_clone)).await;
                match result {
                    Ok(title) => {
                        let _ = weak.update(cx, |session, cx| {
                            session.title = title.clone().into();
                            if let Some(ref tid) = session.thread_id {
                                let _ = session.store.update_thread(
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
                            cx.emit(SessionEvent::Changed);
                        });
                        let _ = cx.update_global(|_: &mut AppStore, _| {});
                    }
                    Err(e) => {
                        eprintln!("[mini-pi] failed to generate title: {}", e);
                    }
                }
            })
            .detach();

            temp_title
        } else {
            self.store
                .get_thread(tid)
                .ok()
                .flatten()
                .map(|t| t.title)
                .unwrap_or_default()
        };
        let preview: String = content.chars().take(120).collect();
        let _ = self.store.update_thread(
            tid,
            Some(&title),
            Some(&preview),
            None,
            None,
            None,
            None,
            None,
            true,
        );

        self.state = ChatState::Streaming;

        if let Some(ref mut rpc) = self.rpc {
            let _ = rpc.send_prompt(&content);
        }

        cx.emit(SessionEvent::Changed);
    }

    pub fn send_message_with_media(
        &mut self,
        content: SharedString,
        media: Vec<ImageContent>,
        cx: &mut Context<Self>,
    ) {
        eprintln!(
            "[mini-pi] send_message_with_media: {} chars, {} media items",
            content.len(),
            media.len()
        );
        if media.is_empty() {
            self.send_message(content, cx);
            return;
        }

        if self.rpc.is_none() && !self.spawn_pi(false, cx) {
            return;
        }

        self.messages.push(Message {
            id: Uuid::new_v4().to_string(),
            entry_id: None,
            role: Role::User,
            parts: vec![MessagePart::Text {
                text: content.clone(),
                state: Some(PartState::Done),
            }],
            media: media.clone(),
        });

        if self.thread_id.is_none() {
            match self.store.create_thread("", "") {
                Ok(thread) => {
                    let thread_id = thread.id.clone();
                    self.thread_id = Some(thread_id);
                    self.title = content.chars().take(80).collect::<String>().into();
                    let sf = self.session_file.clone();
                    let model_opt = self.selected_model.as_deref();
                    let thinking_opt = self.thinking_level.as_deref();
                    let metadata = self
                        .workspace
                        .as_ref()
                        .map(|ws| serde_json::json!({ "workspace_id": ws.id }));
                    let metadata_ref = metadata.as_ref();
                    let _ = self.store.update_thread(
                        &thread.id,
                        Some(&self.title),
                        None,
                        Some(Some(&sf)),
                        Some(model_opt),
                        Some(thinking_opt),
                        None,
                        Some(metadata_ref),
                        false,
                    );
                }
                Err(_) => {
                    self.state = ChatState::Error("Failed to create thread".into());
                    cx.emit(SessionEvent::Changed);
                    return;
                }
            }
        }

        let tid = self.thread_id.as_ref().unwrap();
        let user_count = self
            .messages
            .iter()
            .filter(|m| matches!(m.role, Role::User))
            .count();
        let title = if user_count == 1 {
            let temp_title: String = content.chars().take(80).collect();

            let content_clone = content.clone();
            let weak = cx.entity().downgrade();
            cx.spawn(async move |_, cx| {
                let result = smol::unblock(move || generate_title(&content_clone)).await;
                match result {
                    Ok(title) => {
                        let _ = weak.update(cx, |session, cx| {
                            session.title = title.clone().into();
                            if let Some(ref tid) = session.thread_id {
                                let _ = session.store.update_thread(
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
                            cx.emit(SessionEvent::Changed);
                        });
                        let _ = cx.update_global(|_: &mut AppStore, _| {});
                    }
                    Err(e) => {
                        eprintln!("[mini-pi] failed to generate title: {}", e);
                    }
                }
            })
            .detach();

            temp_title
        } else {
            self.store
                .get_thread(tid)
                .ok()
                .flatten()
                .map(|t| t.title)
                .unwrap_or_default()
        };
        let preview: String = content.chars().take(120).collect();
        let _ = self.store.update_thread(
            tid,
            Some(&title),
            Some(&preview),
            None,
            None,
            None,
            None,
            None,
            true,
        );

        self.state = ChatState::Streaming;

        if let Some(ref mut rpc) = self.rpc {
            let _ = rpc.send_prompt_ext(&content, Some(&media), None, None);
        }

        cx.emit(SessionEvent::Changed);
    }

    pub fn send_edited_prompt(
        &mut self,
        message_id: String,
        content: SharedString,
        cx: &mut Context<Self>,
    ) {
        if self.rpc.is_none() && !self.spawn_pi(false, cx) {
            return;
        }

        let Some(edit_idx) = self.messages.iter().position(|m| m.id == message_id) else {
            eprintln!("[mini-pi] edited message {} not found", message_id);
            return;
        };

        if self.messages[edit_idx].entry_id.is_none() {
            self.pending_fork = Some((message_id, content.to_string()));
            if let Some(ref mut rpc) = self.rpc {
                let _ = rpc.send_get_messages(None);
            }
            cx.emit(SessionEvent::Changed);
            return;
        }

        let entry_id = self.messages[edit_idx].entry_id.clone().unwrap();

        self.messages[edit_idx].parts = vec![MessagePart::Text {
            text: content.clone(),
            state: Some(PartState::Done),
        }];
        self.messages.truncate(edit_idx + 1);

        if self.thread_id.is_none() {
            match self.store.create_thread("", "") {
                Ok(thread) => {
                    let thread_id = thread.id.clone();
                    self.thread_id = Some(thread_id);
                    let sf = self.session_file.clone();
                    let model_opt = self.selected_model.as_deref();
                    let thinking_opt = self.thinking_level.as_deref();
                    let _ = self.store.update_thread(
                        &thread.id,
                        None,
                        None,
                        Some(Some(&sf)),
                        Some(model_opt),
                        Some(thinking_opt),
                        None,
                        None,
                        false,
                    );
                }
                Err(_) => {
                    self.state = ChatState::Error("Failed to create thread".into());
                    cx.emit(SessionEvent::Changed);
                    return;
                }
            }
        }
        if let Some(ref tid) = self.thread_id {
            let preview: String = content.chars().take(120).collect();
            let _ = self.store.update_thread(
                tid,
                None,
                Some(&preview),
                None,
                None,
                None,
                None,
                None,
                true,
            );
        }
        self.state = ChatState::Streaming;
        if let Some(ref mut rpc) = self.rpc {
            let _ = rpc.send_navigate_tree(&entry_id, None);
            let _ = rpc.send_prompt(&content);
        }
        self.refresh_entry_ids_after_streaming = true;
        cx.emit(SessionEvent::Changed);
    }

    pub fn abort(&mut self, cx: &mut Context<Self>) {
        if !self.is_streaming() {
            return;
        }

        // Tell the SDK to stop generating. We intentionally do NOT try to
        // suppress late events here; that filtering belongs in the bridge
        // event consumer once the abort is acknowledged.
        if let Some(ref mut rpc) = self.rpc {
            let _ = rpc.send_abort(None);
        }

        // Finalize any in-flight streaming parts so the UI stops animating
        // immediately, even before the bridge acknowledges the abort.
        for msg in self.messages.iter_mut() {
            for part in msg.parts.iter_mut() {
                match part {
                    MessagePart::Text { state, .. }
                    | MessagePart::Reasoning { state, .. }
                    | MessagePart::ToolCall { state, .. }
                    | MessagePart::ToolResult { state, .. } => {
                        if let Some(s) = state {
                            if *s == PartState::Streaming {
                                *s = PartState::Done;
                            }
                        }
                    }
                }
            }
        }

        self.state = ChatState::Idle;
        self.request_session_stats();

        if let Some(ref tid) = self.thread_id {
            cx.update_global(|app: &mut AppStore, _| {
                app.set_thread_streaming(tid.clone(), false);
            });
        }

        cx.emit(SessionEvent::Changed);
    }

    pub fn request_history(&mut self) {
        if let Some(ref mut rpc) = self.rpc {
            let _ = rpc.send_get_messages(None);
        }
    }

    pub fn request_commands(&mut self) {
        if let Some(ref mut rpc) = self.rpc {
            let _ = rpc.send_get_commands(None);
        }
    }

    pub fn request_session_stats(&mut self) {
        if let Some(ref mut rpc) = self.rpc {
            let _ = rpc.send_get_session_stats(None);
        }
    }

    pub fn export_html(&mut self, output_path: &str) {
        if let Some(ref mut rpc) = self.rpc
            && let Err(e) = rpc.send_export_html(Some(output_path), None)
        {
            eprintln!("[mini-pi] send_export_html failed: {}", e);
        }
    }

    pub fn respond_extension_ui(
        &mut self,
        id: &str,
        response: &serde_json::Value,
        _cx: &mut Context<Self>,
    ) {
        if let Some(ref mut rpc) = self.rpc {
            if let Err(e) = rpc.send_extension_ui_response(id, response) {
                eprintln!("[mini-pi] extension_ui_response failed: {}", e);
            }
        }
    }

    fn spawn_pi(&mut self, restoring: bool, cx: &mut Context<Self>) -> bool {
        let Some(bridge) = cx.global::<AppStore>().pi_bridge.clone() else {
            self.state = ChatState::Error(
                "Failed to start pi SDK bridge. Run `cd pi-bridge && bun install`.".into(),
            );
            cx.emit(SessionEvent::Changed);
            return false;
        };

        let session_path = self.store.sessions_dir().join(&self.session_file);
        let workspace_dir: Option<PathBuf> = self.workspace.as_ref().map(|ws| ws.path.clone());

        let session_id = self.session_file.clone();
        let rx = match bridge.create_session(
            session_id.clone(),
            Some(session_path),
            workspace_dir,
            self.selected_model.clone(),
            self.thinking_level.clone(),
        ) {
            Ok(rx) => rx,
            Err(e) => {
                eprintln!("[mini-pi] failed to create SDK session: {}", e);
                self.state = ChatState::Error("Failed to create pi SDK session.".into());
                cx.emit(SessionEvent::Changed);
                return false;
            }
        };

        let mut rpc = PiRpc::new(session_id.clone(), bridge);
        eprintln!("[mini-pi] pi SDK session created: {}", self.session_file);

        if let Err(e) = rpc.send_get_commands(None) {
            eprintln!("[mini-pi] failed to send get_commands: {}", e);
        }

        if restoring {
            eprintln!("[mini-pi] restoring session, requesting message history");
            if let Err(e) = rpc.send_get_messages(None) {
                eprintln!("[mini-pi] failed to send get_messages: {}", e);
            }
            self.state = ChatState::Loading;
        }

        if let Some(ref level) = self.thinking_level
            && let Err(e) = rpc.send_set_thinking_level(level, None)
        {
            eprintln!("[mini-pi] send_set_thinking_level failed: {}", e);
        }

        let weak = cx.entity().downgrade();
        let task = cx.spawn(async move |_, cx: &mut gpui::AsyncApp| {
            let mut rx = rx;
            while let Some(event) = rx.next().await {
                let result = weak.update(cx, |session, cx| session.handle_bridge_event(event, cx));
                match result {
                    Ok((streaming_changed, new_activity)) => {
                        if streaming_changed || new_activity {
                            let (thread_id, is_streaming) = match weak.update(cx, |session, _cx| {
                                (session.thread_id.clone(), session.is_streaming())
                            }) {
                                Ok(v) => v,
                                Err(_) => break,
                            };
                            if let Some(tid) = thread_id {
                                let has_open_window =
                                    cx.update_global(|app: &mut AppStore, _cx| {
                                        app.set_thread_streaming(tid.clone(), is_streaming);
                                        app.is_thread_window_open(&tid)
                                    });
                                if new_activity && !has_open_window {
                                    let _ = weak.update(cx, |session, _cx| {
                                        session.mark_has_new_activity();
                                    });
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            eprintln!("[mini-pi] event loop ended");
        });

        self.rpc = Some(rpc);
        self._event_task = Some(task);
        self.pi_restart_count = 0;
        cx.emit(SessionEvent::Changed);
        true
    }

    fn set_has_new_activity_db(&self, value: bool) {
        if let Some(ref tid) = self.thread_id {
            let md = self
                .store
                .get_thread(tid)
                .ok()
                .flatten()
                .and_then(|t| t.metadata)
                .unwrap_or_else(|| serde_json::json!({}));
            let mut md = md;
            md["has_new_activity"] = serde_json::Value::Bool(value);
            let _ = self.store.update_thread(
                tid,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(Some(&md)),
                false,
            );
        }
    }

    pub fn clear_new_activity(&mut self, cx: &mut Context<Self>) {
        self.set_has_new_activity_db(false);
        cx.emit(SessionEvent::Changed);
    }

    pub fn mark_has_new_activity(&self) {
        self.set_has_new_activity_db(true);
    }

    fn handle_bridge_event(&mut self, event: BridgeEvent, cx: &mut Context<Self>) -> (bool, bool) {
        eprintln!("[mini-pi] bridge event: {:?}", event);
        let was_streaming = self.is_streaming();
        match event {
            BridgeEvent::AgentStart => {
                self.state = ChatState::Streaming;
            }
            BridgeEvent::MessageStart { message } => {
                let role = message
                    .as_ref()
                    .and_then(|m| m.get("role"))
                    .and_then(|r| r.as_str());
                // The SDK emits message_start for both user and assistant messages.
                // We already add the user message locally when send_message is called,
                // so only create a new assistant message for assistant turns.
                if role == Some("assistant") {
                    let entry_id = message
                        .as_ref()
                        .and_then(|m| m.get("id"))
                        .and_then(|id| id.as_str())
                        .map(|s| s.to_string());
                    let error_message = message
                        .as_ref()
                        .and_then(|m| m.get("errorMessage"))
                        .and_then(|e| e.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                    let stop_reason = message
                        .as_ref()
                        .and_then(|m| m.get("stopReason"))
                        .and_then(|s| s.as_str());
                    let is_error = is_assistant_error(error_message.as_deref(), stop_reason);

                    let id = Uuid::new_v4().to_string();
                    let mut parts = vec![];
                    if let Some(ref error) = error_message {
                        parts.push(MessagePart::Text {
                            text: format!("Error: {}", error).into(),
                            state: Some(PartState::Done),
                        });
                        self.state = ChatState::Error(error.clone().into());
                    } else if is_error {
                        parts.push(MessagePart::Text {
                            text: "Error: assistant response failed.".into(),
                            state: Some(PartState::Done),
                        });
                        self.state = ChatState::Error("Assistant response failed.".into());
                    }
                    self.messages.push(Message {
                        id,
                        entry_id,
                        role: Role::Assistant,
                        parts,
                        media: Vec::new(),
                    });
                }
            }
            BridgeEvent::AgentEnd { messages } => {
                let had_error = messages
                    .as_ref()
                    .map(|m| self.apply_final_agent_messages(m.clone()))
                    .unwrap_or(false);
                self.request_session_stats();
                for msg in self.messages.iter_mut() {
                    for part in msg.parts.iter_mut() {
                        match part {
                            MessagePart::Text { state, .. }
                            | MessagePart::Reasoning { state, .. }
                            | MessagePart::ToolCall { state, .. }
                            | MessagePart::ToolResult { state, .. } => {
                                if let Some(s) = state {
                                    *s = PartState::Done;
                                }
                            }
                        }
                    }
                }
                if !had_error {
                    self.state = ChatState::Idle;
                }
                if self.refresh_entry_ids_after_streaming {
                    if let Some(ref mut rpc) = self.rpc {
                        let _ = rpc.send_get_messages(None);
                    } else {
                        self.refresh_entry_ids_after_streaming = false;
                    }
                }
            }
            BridgeEvent::TextDelta { content } => {
                // Always target the most recent assistant message. Stale assistant
                // messages from earlier turns may still carry a Streaming state, and
                // scanning the whole list would append deltas to the wrong message.
                if let Some(msg) = self.messages.last_mut()
                    && matches!(msg.role, Role::Assistant)
                {
                    if let Some(part) = msg.parts.iter_mut().rev().find(|p| {
                        matches!(
                            p,
                            MessagePart::Text {
                                state: Some(PartState::Streaming),
                                ..
                            }
                        )
                    }) && let MessagePart::Text { text, .. } = part
                    {
                        let new_text = format!("{}{}", text, content);
                        *text = new_text.into();
                    } else {
                        msg.parts.push(MessagePart::Text {
                            text: content.into(),
                            state: Some(PartState::Streaming),
                        });
                    }
                }
            }
            BridgeEvent::ThinkingDelta { content } => {
                if let Some(msg) = self.messages.last_mut()
                    && matches!(msg.role, Role::Assistant)
                {
                    if let Some(part) = msg.parts.iter_mut().rev().find(|p| {
                        matches!(
                            p,
                            MessagePart::Reasoning {
                                state: Some(PartState::Streaming),
                                ..
                            }
                        )
                    }) && let MessagePart::Reasoning { text, .. } = part
                    {
                        let new_text = format!("{}{}", text, content);
                        *text = new_text.into();
                    } else {
                        msg.parts.push(MessagePart::Reasoning {
                            text: content.into(),
                            state: Some(PartState::Streaming),
                            provider_metadata: None,
                        });
                    }
                }
            }
            BridgeEvent::ToolEnd {
                tool_name,
                output,
                is_error,
                call_id,
                details,
            } => {
                if let Some(msg) = self.messages.last_mut()
                    && matches!(msg.role, Role::Assistant)
                {
                    if let Some(part) = msg.parts.iter_mut().find(|p| {
                        matches!(
                            p,
                            MessagePart::ToolCall {
                                tool_call_id: id,
                                ..
                            } if id == &SharedString::from(call_id.clone())
                        )
                    }) && let MessagePart::ToolCall { state, .. } = part
                    {
                        *state = Some(PartState::Done);
                    }

                    msg.parts.push(MessagePart::ToolResult {
                        tool_call_id: call_id.into(),
                        name: tool_name.into(),
                        output: if is_error {
                            format!("ERROR: {}", truncate_str(&output, 5000))
                        } else {
                            truncate_str(&output, 5000)
                        }
                        .into(),
                        state: Some(PartState::Done),
                        details: if is_error { None } else { details },
                    });
                }
            }
            BridgeEvent::Error { message } => {
                self.state = ChatState::Error(message.into());
            }
            BridgeEvent::TextStart => {
                if let Some(msg) = self.messages.last_mut()
                    && matches!(msg.role, Role::Assistant)
                {
                    msg.parts.push(MessagePart::Text {
                        text: SharedString::from(""),
                        state: Some(PartState::Streaming),
                    });
                }
            }
            BridgeEvent::TextEnd { .. } => {
                if let Some(msg) = self.messages.last_mut()
                    && matches!(msg.role, Role::Assistant)
                    && let Some(part) = msg.parts.iter_mut().rev().find(|p| {
                        matches!(
                            p,
                            MessagePart::Text {
                                state: Some(PartState::Streaming),
                                ..
                            }
                        )
                    })
                    && let MessagePart::Text { state, .. } = part
                {
                    *state = Some(PartState::Done);
                }
            }
            BridgeEvent::ThinkingStart => {
                if let Some(msg) = self.messages.last_mut()
                    && matches!(msg.role, Role::Assistant)
                {
                    msg.parts.push(MessagePart::Reasoning {
                        text: SharedString::from(""),
                        state: Some(PartState::Streaming),
                        provider_metadata: None,
                    });
                }
            }
            BridgeEvent::ThinkingEnd { .. } => {
                if let Some(msg) = self.messages.last_mut()
                    && matches!(msg.role, Role::Assistant)
                    && let Some(part) = msg.parts.iter_mut().rev().find(|p| {
                        matches!(
                            p,
                            MessagePart::Reasoning {
                                state: Some(PartState::Streaming),
                                ..
                            }
                        )
                    })
                    && let MessagePart::Reasoning { state, .. } = part
                {
                    *state = Some(PartState::Done);
                }
            }
            // ToolCallStart/ToolCallDelta use placeholder ids ("unknown"), so we
            // ignore them and only create/update ToolCalls once the real id/name
            // arrive in ToolStart or ToolCallEnd.
            BridgeEvent::ToolCallStart { .. } => {}
            BridgeEvent::ToolCallDelta { .. } => {}
            BridgeEvent::ToolStart {
                name,
                args,
                call_id,
            } => {
                if let Some(msg) = self.messages.last_mut()
                    && matches!(msg.role, Role::Assistant)
                {
                    let args_str = args
                        .as_ref()
                        .map(|v| serde_json::to_string(v).unwrap_or_default())
                        .unwrap_or_default();
                    if let Some(part) = msg.parts.iter_mut().find(|p| {
                        matches!(
                            p,
                            MessagePart::ToolCall {
                                tool_call_id: id,
                                ..
                            } if id == &SharedString::from(call_id.clone())
                        )
                    }) && let MessagePart::ToolCall {
                        name: part_name,
                        args: part_args,
                        state,
                        ..
                    } = part
                    {
                        if !name.is_empty() && name != "unknown" {
                            *part_name = name.into();
                        }
                        if !args_str.is_empty() {
                            *part_args = args_str.into();
                        }
                        if !matches!(state, Some(PartState::Done)) {
                            *state = Some(PartState::Streaming);
                        }
                    } else {
                        msg.parts.push(MessagePart::ToolCall {
                            tool_call_id: call_id.into(),
                            name: name.into(),
                            args: args_str.into(),
                            state: Some(PartState::Streaming),
                        });
                    }
                }
            }
            BridgeEvent::ToolCallEnd {
                call_id,
                name,
                args,
            } => {
                if let Some(msg) = self.messages.last_mut()
                    && matches!(msg.role, Role::Assistant)
                {
                    let args_str = serde_json::to_string(&args).unwrap_or_default();
                    if let Some(part) = msg.parts.iter_mut().find(|p| {
                        matches!(
                            p,
                            MessagePart::ToolCall {
                                tool_call_id: id,
                                ..
                            } if id == &SharedString::from(call_id.clone())
                        )
                    }) && let MessagePart::ToolCall {
                        name: part_name,
                        args: part_args,
                        state,
                        ..
                    } = part
                    {
                        if !name.is_empty() && name != "unknown" {
                            *part_name = name.into();
                        }
                        *part_args = args_str.into();
                        *state = Some(PartState::Done);
                    } else {
                        msg.parts.push(MessagePart::ToolCall {
                            tool_call_id: call_id.into(),
                            name: name.into(),
                            args: args_str.into(),
                            state: Some(PartState::Done),
                        });
                    }
                }
            }
            BridgeEvent::ToolUpdate { .. } => {}
            BridgeEvent::TurnStart | BridgeEvent::TurnEnd => {}
            BridgeEvent::MessageEnd => {
                if let Some(msg) = self.messages.last_mut()
                    && matches!(msg.role, Role::Assistant)
                {
                    for part in msg.parts.iter_mut() {
                        match part {
                            MessagePart::Text { state, .. }
                            | MessagePart::Reasoning { state, .. }
                            | MessagePart::ToolCall { state, .. }
                            | MessagePart::ToolResult { state, .. } => {
                                if let Some(s) = state {
                                    *s = PartState::Done;
                                }
                            }
                        }
                    }
                }
            }
            BridgeEvent::ExtensionUiRequest {
                id,
                method,
                payload,
            } => {
                cx.emit(SessionEvent::ExtensionUiRequest {
                    id,
                    method,
                    payload,
                });
            }
            BridgeEvent::ExtensionError {
                extension_path,
                event,
                error,
            } => {
                eprintln!(
                    "[mini-pi] extension error in {} (event: {}): {}",
                    extension_path, event, error
                );
            }
            BridgeEvent::Disconnected => {
                eprintln!("[mini-pi] pi process disconnected");
                self.rpc = None;
                if self.pi_restart_count < 1 {
                    self.pi_restart_count += 1;
                    eprintln!(
                        "[mini-pi] attempting to restart pi (attempt {})",
                        self.pi_restart_count
                    );
                    if !self.spawn_pi(false, cx) {
                        self.state = ChatState::Error(
                            "Pi agent disconnected and could not be restarted.".into(),
                        );
                    }
                } else {
                    self.state = ChatState::Error(
                        "Pi agent disconnected and could not be restarted.".into(),
                    );
                }
            }
            BridgeEvent::Response {
                command,
                success,
                data,
                error,
                ..
            } => {
                if command == "create_session"
                    && success
                    && let Some(ref data_val) = data
                    && let Some(session_file) = data_val.get("sessionFile").and_then(|s| s.as_str())
                {
                    let file_name = std::path::Path::new(session_file)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(session_file)
                        .to_string();
                    if self.session_file != file_name {
                        eprintln!("[mini-pi] SDK session file: {}", file_name);
                        self.session_file = file_name.clone();
                        if let Some(ref tid) = self.thread_id {
                            let _ = self.store.update_thread(
                                tid,
                                None,
                                None,
                                Some(Some(&file_name)),
                                None,
                                None,
                                None,
                                None,
                                false,
                            );
                        }
                    }
                }
                if command == "fork" && !success {
                    let err = error.as_deref().unwrap_or("fork failed");
                    eprintln!("[mini-pi] fork failed: {}", err);
                    self.state = ChatState::Error(format!("Fork failed: {}", err).into());
                }
                if command == "get_messages" {
                    if !success {
                        let err = error.as_deref().unwrap_or("failed to load messages");
                        eprintln!("[mini-pi] get_messages failed: {}", err);
                        self.state = ChatState::Error(format!("Load failed: {}", err).into());
                        self.pending_fork = None;
                        self.refresh_entry_ids_after_streaming = false;
                    } else if let Some(ref data_val) = data {
                        if let Some(messages_val) = data_val.get("messages") {
                            if let Some(loaded) =
                                crate::rpc::pi_rpc::parse_loaded_messages(messages_val)
                            {
                                if self.pending_fork.is_some() {
                                    self.update_entry_ids(&loaded);
                                    if let Some((msg_id, content)) = self.pending_fork.take() {
                                        if let Some(msg) =
                                            self.messages.iter().find(|m| m.id == msg_id)
                                        {
                                            if msg.entry_id.is_some() {
                                                self.send_edited_prompt(msg_id, content.into(), cx);
                                            } else {
                                                eprintln!(
                                                    "[mini-pi] could not resolve entry id for edited message"
                                                );
                                                self.state = ChatState::Error(
                                                    "Could not resolve message entry id".into(),
                                                );
                                            }
                                        } else {
                                            eprintln!(
                                                "[mini-pi] pending fork target message disappeared"
                                            );
                                            self.state =
                                                ChatState::Error("Edited message not found".into());
                                        }
                                    }
                                } else if self.refresh_entry_ids_after_streaming {
                                    self.refresh_entry_ids_after_streaming = false;
                                    self.update_entry_ids(&loaded);
                                } else {
                                    self.load_messages(loaded, cx);
                                }
                            } else {
                                self.state = ChatState::Idle;
                            }
                        } else {
                            self.state = ChatState::Idle;
                        }
                    } else {
                        self.state = ChatState::Idle;
                    }
                }
                if command == "export_html" {
                    if success {
                        if let Some(ref data_val) = data
                            && let Some(path) = data_val.get("path").and_then(|p| p.as_str())
                        {
                            eprintln!("[mini-pi] session exported to: {}", path);
                            cx.emit(SessionEvent::ExportHtmlSucceeded {
                                path: PathBuf::from(path),
                            });
                        }
                    } else {
                        let err = error.as_deref().unwrap_or("export failed");
                        eprintln!("[mini-pi] export_html failed: {}", err);
                        self.state = ChatState::Error(format!("Export failed: {}", err).into());
                    }
                }
                if command == "get_commands"
                    && success
                    && let Some(ref data_val) = data
                    && let Some(commands) = data_val.get("commands")
                    && let Some(arr) = commands.as_array()
                {
                    let items: Vec<CommandItem> = arr
                        .iter()
                        .filter_map(|cmd| {
                            let name = cmd.get("name")?.as_str()?.to_string();
                            let description = cmd
                                .get("description")
                                .and_then(|d| d.as_str())
                                .map(|s| s.to_string());
                            let source = cmd
                                .get("source")
                                .and_then(|s| s.as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            Some(CommandItem {
                                name,
                                description,
                                source,
                            })
                        })
                        .collect();
                    self.commands = items;
                }
                if command == "get_session_stats" && success {
                    if let Some(ref data_val) = data {
                        self.session_stats = Some(parse_session_stats(data_val));
                    }
                }
            }
        }

        let is_streaming = self.is_streaming();
        let streaming_changed = was_streaming != is_streaming;
        let new_activity = was_streaming && !is_streaming;
        cx.emit(SessionEvent::Changed);
        (streaming_changed, new_activity)
    }

    fn load_messages(&mut self, messages: Vec<LoadedMessage>, cx: &mut Context<Self>) {
        eprintln!("[mini-pi] loaded {} messages from history", messages.len());
        self.messages.clear();
        let mut has_error = false;
        let mut last_error_text: SharedString = SharedString::from("Assistant response failed.");
        for msg in messages {
            match msg.role.as_str() {
                "user" => {
                    let mut parts = vec![];
                    let mut media = vec![];
                    for part in msg.parts {
                        match part {
                            LoadedPart::Text { text } => {
                                parts.push(MessagePart::Text {
                                    text: if text.is_empty() {
                                        SharedString::from("(empty)")
                                    } else {
                                        text.into()
                                    },
                                    state: Some(PartState::Done),
                                });
                            }
                            LoadedPart::Image { data, mime_type } => {
                                media.push(ImageContent { data, mime_type });
                            }
                            _ => {}
                        }
                    }
                    if !parts.is_empty() || !media.is_empty() {
                        self.messages.push(Message {
                            id: Uuid::new_v4().to_string(),
                            entry_id: msg.id,
                            role: Role::User,
                            parts,
                            media,
                        });
                    }
                }
                "assistant" => {
                    let mut parts = vec![];
                    for part in msg.parts {
                        match part {
                            LoadedPart::Text { text } => {
                                parts.push(MessagePart::Text {
                                    text: text.into(),
                                    state: Some(PartState::Done),
                                });
                            }
                            LoadedPart::Thinking { text } => {
                                parts.push(MessagePart::Reasoning {
                                    text: text.into(),
                                    state: Some(PartState::Done),
                                    provider_metadata: None,
                                });
                            }
                            LoadedPart::ToolCall { name, args } => {
                                parts.push(MessagePart::ToolCall {
                                    tool_call_id: SharedString::from(""),
                                    name: name.into(),
                                    args: args.into(),
                                    state: Some(PartState::Done),
                                });
                            }
                            LoadedPart::ToolResult { name, output } => {
                                parts.push(MessagePart::ToolResult {
                                    tool_call_id: SharedString::from(""),
                                    name: name.into(),
                                    output: output.into(),
                                    state: Some(PartState::Done),
                                    details: None,
                                });
                            }
                            LoadedPart::Image { .. } => {}
                        }
                    }
                    // Preserve any errorMessage that isn't already represented by a
                    // synthetic error text part from parse_loaded_messages.
                    if let Some(ref error) = msg.error_message {
                        if !error.is_empty() {
                            let error_text: SharedString = format!("Error: {}", error).into();
                            let already_present = parts.iter().any(|p| {
                                matches!(p, MessagePart::Text { text, .. } if text == &error_text)
                            });
                            if !already_present {
                                parts.push(MessagePart::Text {
                                    text: error_text,
                                    state: Some(PartState::Done),
                                });
                            }
                        }
                    } else if msg.is_error {
                        let fallback: SharedString = "Error: assistant response failed.".into();
                        if !parts.iter().any(
                            |p| matches!(p, MessagePart::Text { text, .. } if text == &fallback),
                        ) {
                            parts.push(MessagePart::Text {
                                text: fallback,
                                state: Some(PartState::Done),
                            });
                        }
                    }
                    if msg.is_error {
                        has_error = true;
                        if let Some(ref error) = msg.error_message {
                            if !error.is_empty() {
                                last_error_text = format!("Error: {}", error).into();
                            }
                        }
                    }
                    if !parts.is_empty() {
                        self.messages.push(Message {
                            id: Uuid::new_v4().to_string(),
                            entry_id: msg.id,
                            role: Role::Assistant,
                            parts,
                            media: Vec::new(),
                        });
                    }
                }
                "tool" => {
                    for part in msg.parts {
                        if let LoadedPart::ToolResult { name, output } = part
                            && let Some(last_msg) = self.messages.last_mut()
                            && matches!(last_msg.role, Role::Assistant)
                        {
                            last_msg.parts.push(MessagePart::ToolResult {
                                tool_call_id: SharedString::from(""),
                                name: name.into(),
                                output: output.into(),
                                state: Some(PartState::Done),
                                details: None,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        if has_error {
            self.state = ChatState::Error(last_error_text);
        } else if matches!(self.state, ChatState::Loading) {
            self.state = ChatState::Idle;
        }
        cx.emit(SessionEvent::Changed);
    }

    fn update_entry_ids(&mut self, loaded: &[LoadedMessage]) {
        use std::collections::HashMap;
        let mut by_role: HashMap<&str, Vec<&LoadedMessage>> = HashMap::new();
        for msg in loaded {
            by_role.entry(msg.role.as_str()).or_default().push(msg);
        }
        let mut indices: HashMap<&str, usize> = HashMap::new();
        for ui_msg in self.messages.iter_mut() {
            let role_str = match ui_msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            let idx = indices.entry(role_str).or_insert(0);
            if let Some(loaded_msg) = by_role.get(role_str).and_then(|v| v.get(*idx)) {
                if ui_msg.entry_id.is_none() {
                    ui_msg.entry_id = loaded_msg.id.clone();
                }
                *idx += 1;
            }
        }
    }

    fn apply_final_agent_messages(&mut self, loaded: Vec<LoadedMessage>) -> bool {
        let mut assistant_messages = loaded
            .into_iter()
            .filter(|msg| msg.role == "assistant" && !msg.parts.is_empty())
            .collect::<Vec<_>>();
        let Some(final_assistant) = assistant_messages.pop() else {
            return false;
        };

        let Some(target) = self
            .messages
            .iter_mut()
            .rev()
            .find(|msg| matches!(msg.role, Role::Assistant))
        else {
            return false;
        };

        let final_parts = loaded_parts_to_message_parts(final_assistant.parts);
        if final_parts.is_empty() {
            return false;
        }

        if target.entry_id.is_none() {
            target.entry_id = final_assistant.id;
        }

        // If the SDK reported an error for this final assistant message, keep the
        // session in an error state so the user sees the failure banner.
        if final_assistant.is_error {
            self.state = ChatState::Error(
                final_assistant
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "Assistant response failed.".into())
                    .into(),
            );
        }

        // If streaming already populated parts, don't blindly replace them with the
        // final snapshot: the SDK final message may omit tool-call/tool-result
        // details or use a different part ordering, which would desynchronize the
        // stream's part_states index tracking. Instead, merge final text/reasoning
        // into existing parts and only replace when the target is still empty.
        let target_is_empty = target.parts.is_empty()
            || target.parts.iter().all(|p| match p {
                MessagePart::Text { text, .. } => text.is_empty(),
                MessagePart::Reasoning { text, .. } => text.is_empty(),
                MessagePart::ToolCall { args, .. } => args.is_empty(),
                MessagePart::ToolResult { output, .. } => output.is_empty(),
            });

        if target_is_empty {
            target.parts = final_parts;
        } else {
            // For error turns, keep content already surfaced during
            // streaming/MessageStart rather than letting a later non-empty snapshot
            // overwrite the error text. For normal turns, merge final parts.
            if !final_assistant.is_error {
                for final_part in final_parts {
                    match final_part {
                        MessagePart::Text { text, .. } => {
                            // Don't overwrite already-populated text (e.g. an error
                            // surfaced during MessageStart) with an empty final snapshot.
                            if text.is_empty() {
                                continue;
                            }
                            if let Some(MessagePart::Text {
                                text: existing_text,
                                state,
                                ..
                            }) = target
                                .parts
                                .iter_mut()
                                .find(|p| matches!(p, MessagePart::Text { .. }))
                            {
                                *existing_text = text;
                                *state = Some(PartState::Done);
                            } else {
                                target.parts.push(MessagePart::Text {
                                    text,
                                    state: Some(PartState::Done),
                                });
                            }
                        }
                        MessagePart::Reasoning {
                            text,
                            provider_metadata,
                            ..
                        } => {
                            if text.is_empty() {
                                continue;
                            }
                            if let Some(MessagePart::Reasoning {
                                text: existing_text,
                                state,
                                ..
                            }) = target
                                .parts
                                .iter_mut()
                                .find(|p| matches!(p, MessagePart::Reasoning { .. }))
                            {
                                *existing_text = text;
                                *state = Some(PartState::Done);
                            } else {
                                target.parts.push(MessagePart::Reasoning {
                                    text,
                                    state: Some(PartState::Done),
                                    provider_metadata,
                                });
                            }
                        }
                        MessagePart::ToolCall { .. } | MessagePart::ToolResult { .. } => {
                            // Already streamed or created inline; keep them.
                        }
                    }
                }
            }
        }

        final_assistant.is_error
    }
}

fn parse_session_stats(val: &serde_json::Value) -> SessionStats {
    let tokens = val.get("tokens").and_then(|t| t.as_object());
    let context_usage = val.get("contextUsage");
    SessionStats {
        session_file: val
            .get("sessionFile")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        session_id: val
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        user_messages: val
            .get("userMessages")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        assistant_messages: val
            .get("assistantMessages")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        tool_calls: val.get("toolCalls").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
        tool_results: val.get("toolResults").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
        total_messages: val
            .get("totalMessages")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        tokens_input: tokens
            .and_then(|t| t.get("input"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        tokens_output: tokens
            .and_then(|t| t.get("output"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        tokens_cache_read: tokens
            .and_then(|t| t.get("cacheRead"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        tokens_cache_write: tokens
            .and_then(|t| t.get("cacheWrite"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        tokens_total: tokens
            .and_then(|t| t.get("total"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        cost: val.get("cost").and_then(|v| v.as_f64()).unwrap_or(0.0),
        context_tokens: context_usage.and_then(|c| c.get("tokens")).and_then(|v| {
            if v.is_null() {
                None
            } else {
                v.as_u64().map(|n| n as usize)
            }
        }),
        context_window: context_usage
            .and_then(|c| c.get("contextWindow"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        context_percent: context_usage
            .and_then(|c| c.get("percent"))
            .and_then(|v| if v.is_null() { None } else { v.as_f64() }),
    }
}

fn loaded_parts_to_message_parts(parts: Vec<LoadedPart>) -> Vec<MessagePart> {
    parts
        .into_iter()
        .filter_map(|part| match part {
            LoadedPart::Text { text } => Some(MessagePart::Text {
                text: text.into(),
                state: Some(PartState::Done),
            }),
            LoadedPart::Image { .. } => None,
            LoadedPart::Thinking { text } => Some(MessagePart::Reasoning {
                text: text.into(),
                state: Some(PartState::Done),
                provider_metadata: None,
            }),
            LoadedPart::ToolCall { name, args } => Some(MessagePart::ToolCall {
                tool_call_id: SharedString::from(""),
                name: name.into(),
                args: args.into(),
                state: Some(PartState::Done),
            }),
            LoadedPart::ToolResult { name, output } => Some(MessagePart::ToolResult {
                tool_call_id: SharedString::from(""),
                name: name.into(),
                output: output.into(),
                state: Some(PartState::Done),
                details: None,
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_session_stats_full() {
        let val = serde_json::json!({
            "sessionFile": "/home/user/.mini-pi/sessions/session_123.jsonl",
            "sessionId": "session_123",
            "userMessages": 3,
            "assistantMessages": 4,
            "toolCalls": 5,
            "toolResults": 6,
            "totalMessages": 7,
            "tokens": {
                "input": 1000,
                "output": 5000,
                "cacheRead": 200,
                "cacheWrite": 100,
                "total": 1500
            },
            "cost": 0.0042,
            "contextUsage": {
                "tokens": 1200,
                "contextWindow": 200000,
                "percent": 0.006
            }
        });
        let stats = parse_session_stats(&val);
        assert_eq!(
            stats.session_file,
            Some("/home/user/.mini-pi/sessions/session_123.jsonl".to_string())
        );
        assert_eq!(stats.session_id, "session_123");
        assert_eq!(stats.user_messages, 3);
        assert_eq!(stats.assistant_messages, 4);
        assert_eq!(stats.tool_calls, 5);
        assert_eq!(stats.tool_results, 6);
        assert_eq!(stats.total_messages, 7);
        assert_eq!(stats.tokens_input, 1000);
        assert_eq!(stats.tokens_output, 5000);
        assert_eq!(stats.tokens_cache_read, 200);
        assert_eq!(stats.tokens_cache_write, 100);
        assert_eq!(stats.tokens_total, 1500);
        assert!((stats.cost - 0.0042).abs() < f64::EPSILON);
        assert_eq!(stats.context_tokens, Some(1200));
        assert_eq!(stats.context_window, 200000);
        assert!((stats.context_percent.unwrap() - 0.006).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_session_stats_null_context() {
        let val = serde_json::json!({
            "sessionId": "session_456",
            "tokens": {},
            "contextUsage": {
                "tokens": null,
                "contextWindow": 100000,
                "percent": null
            }
        });
        let stats = parse_session_stats(&val);
        assert_eq!(stats.session_id, "session_456");
        assert_eq!(stats.tokens_total, 0);
        assert_eq!(stats.context_tokens, None);
        assert_eq!(stats.context_window, 100000);
        assert_eq!(stats.context_percent, None);
    }

    #[test]
    fn parse_session_stats_minimal() {
        let val = serde_json::json!({
            "sessionId": "session_789"
        });
        let stats = parse_session_stats(&val);
        assert_eq!(stats.session_id, "session_789");
        assert_eq!(stats.total_messages, 0);
        assert_eq!(stats.tokens_total, 0);
        assert_eq!(stats.cost, 0.0);
        assert_eq!(stats.context_window, 0);
    }

    fn test_store() -> Arc<Store> {
        let dir = std::env::temp_dir().join(format!("mini-pi-test-{}", uuid::Uuid::new_v4()));
        Arc::new(Store::new_test(&dir).expect("test store"))
    }

    fn test_session_handle() -> SessionHandle {
        SessionHandle {
            thread_id: None,
            _session_id: "test-session".into(),
            session_file: "test-session.jsonl".into(),
            title: "Test".into(),
            messages: vec![],
            state: ChatState::Idle,
            commands: vec![],
            selected_model: None,
            thinking_level: None,
            workspace: None,
            rpc: None,
            _event_task: None,
            pending_fork: None,
            refresh_entry_ids_after_streaming: false,
            pi_restart_count: 0,
            session_stats: None,
            store: test_store(),
        }
    }

    #[test]
    fn apply_final_agent_messages_preserves_error_state_for_stop_reason_only() {
        let mut session = test_session_handle();
        session.messages.push(Message {
            id: "user-1".into(),
            entry_id: None,
            role: Role::User,
            parts: vec![MessagePart::Text {
                text: "Hi".into(),
                state: Some(PartState::Done),
            }],
            media: Vec::new(),
        });
        session.messages.push(Message {
            id: "assistant-1".into(),
            entry_id: None,
            role: Role::Assistant,
            parts: vec![MessagePart::Text {
                text: "Error: assistant response failed.".into(),
                state: Some(PartState::Done),
            }],
            media: Vec::new(),
        });

        let loaded = vec![LoadedMessage {
            id: Some("entry-1".into()),
            role: "assistant".into(),
            parts: vec![LoadedPart::Text { text: "".into() }],
            error_message: None,
            is_error: true,
        }];

        let had_error = session.apply_final_agent_messages(loaded);
        assert!(had_error);
        assert!(session.has_error());
        assert!(
            matches!(session.state, ChatState::Error(ref msg) if msg.contains("Assistant response failed.")),
            "expected Error state, got {:?}",
            session.state
        );
        // Existing error text should not be overwritten by the empty final snapshot.
        let assistant = session
            .messages
            .iter()
            .find(|m| matches!(m.role, Role::Assistant))
            .unwrap();
        assert_eq!(assistant.parts.len(), 1);
        assert_eq!(
            assistant.parts[0],
            MessagePart::Text {
                text: "Error: assistant response failed.".into(),
                state: Some(PartState::Done),
            }
        );
    }

    #[gpui::test]
    async fn load_messages_sets_error_state_for_errored_turn(cx: &mut gpui::TestAppContext) {
        let store = test_store();
        let session = cx.new(|_cx| {
            let mut session = test_session_handle();
            session.store = store;
            session
        });
        session.update(cx, |session, cx| {
            let loaded = vec![
                LoadedMessage {
                    id: Some("user-1".into()),
                    role: "user".into(),
                    parts: vec![LoadedPart::Text { text: "Hi".into() }],
                    error_message: None,
                    is_error: false,
                },
                LoadedMessage {
                    id: Some("assistant-1".into()),
                    role: "assistant".into(),
                    parts: vec![LoadedPart::Text { text: "".into() }],
                    error_message: Some("Failed to resolve API key".into()),
                    is_error: true,
                },
            ];
            session.load_messages(loaded, cx);
            assert!(session.has_error());
            assert!(
                matches!(session.state, ChatState::Error(ref msg) if msg.contains("Failed to resolve API key")),
                "expected Error state, got {:?}",
                session.state
            );
            let assistant = session
                .messages
                .iter()
                .find(|m| matches!(m.role, Role::Assistant))
                .unwrap();
            assert!(
                assistant.parts.iter().any(|p| matches!(p, MessagePart::Text { text, .. } if text.contains("Failed to resolve API key"))),
                "expected error text in assistant message, got {:?}",
                assistant.parts
            );
        });
    }
}
