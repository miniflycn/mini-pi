use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui::{
    AppContext, BorrowAppContext, Context, Entity, EventEmitter, Subscription, Task, WeakEntity,
};
use serde_json::{Value, json};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::config::app_config::RemoteControlConfig;
use crate::config::model_config;
use crate::core::app::AppStore;
use crate::core::session_handle::{SessionEvent, SessionHandle, WorkspaceInfo};
use crate::data::models::{ChatState, Message, MessagePart, PartState, Role};
use crate::data::store::{StoreError, ThreadMeta};
use crate::remote::cloudflared;
use crate::remote::server;
use crate::remote::tunnel;
use crate::remote::types::{AiStreamEvent, CommandEnvelope, RemoteCommand, RemoteResponse};

const RESTART_BASE_DELAY: std::time::Duration = std::time::Duration::from_secs(1);
const RESTART_MAX_DELAY: std::time::Duration = std::time::Duration::from_secs(30);
const RESTART_ATTEMPT_CAP: u32 = 10;
const STARTUP_WATCHDOG_SECONDS: u64 = 20;

#[derive(Clone, Debug, PartialEq)]
pub enum RemoteStatus {
    Disabled,
    Starting,
    Running,
    Reconnecting,
    Error(String),
}

impl RemoteStatus {
    /// A short, stable label suitable for the UI and API status field.
    pub fn label(&self) -> &'static str {
        match self {
            RemoteStatus::Disabled => "disabled",
            RemoteStatus::Starting => "starting",
            RemoteStatus::Running => "running",
            RemoteStatus::Reconnecting => "reconnecting",
            RemoteStatus::Error(_) => "error",
        }
    }

    /// Structured detail for API responses. Avoids leaking the internal Debug format.
    pub fn detail(&self) -> serde_json::Value {
        match self {
            RemoteStatus::Disabled => json!("disabled"),
            RemoteStatus::Starting => json!("starting"),
            RemoteStatus::Running => json!("running"),
            RemoteStatus::Reconnecting => json!("reconnecting"),
            RemoteStatus::Error(msg) => json!({ "error": msg }),
        }
    }
}

#[derive(Clone, Debug)]
pub enum RemoteControllerEvent {
    StatusChanged,
}

/// A single cloudflared log line surfaced in the UI.
#[derive(Clone, Debug)]
pub struct TunnelLog {
    pub level: String,
    pub message: String,
}

pub struct RemoteController {
    pub config: RemoteControlConfig,
    pub status: RemoteStatus,
    pub tunnel_url: Option<String>,
    pub error_message: Option<String>,
    pub tunnel_log: Option<TunnelLog>,
    command_sender: Option<UnboundedSender<CommandEnvelope>>,
    command_task: Option<Task<()>>,
    server_handle: Option<server::RemoteServerHandle>,
    tunnel_handle: Option<tunnel::TunnelHandle>,
    watchdog_task: Option<Task<()>>,
    restart_attempts: u32,
    target_thread_id: Option<String>,
    target_session: Option<WeakEntity<SessionHandle>>,
    session_subscription: Option<Subscription>,
    active_streams: Vec<AiSubmitStream>,
}

impl EventEmitter<RemoteControllerEvent> for RemoteController {}

#[derive(Debug)]
struct AiSubmitStream {
    thread_id: String,
    sender: UnboundedSender<AiStreamEvent>,
    initial_message_ids: HashSet<String>,
    assistant_message_id: Option<String>,
    part_states: Vec<AiPartState>,
    finished: bool,
}

#[derive(Clone, Debug)]
enum AiPartState {
    Text {
        id: String,
        text: String,
        done: bool,
    },
    Reasoning {
        id: String,
        text: String,
        done: bool,
    },
    Tool {
        id: String,
        name: String,
        args: String,
        input_available: bool,
        output_available: bool,
    },
}

impl AiSubmitStream {
    fn new(
        thread_id: String,
        sender: UnboundedSender<AiStreamEvent>,
        initial_message_ids: HashSet<String>,
    ) -> Self {
        Self {
            thread_id,
            sender,
            initial_message_ids,
            assistant_message_id: None,
            part_states: Vec::new(),
            finished: false,
        }
    }

    fn send_chunk(&mut self, chunk: Value) -> bool {
        self.sender.send(AiStreamEvent::Chunk(chunk)).is_ok()
    }

    fn send_done(&mut self) -> bool {
        self.finished = true;
        self.sender.send(AiStreamEvent::Done).is_ok()
    }

    fn update(&mut self, messages: &[Message], state: &ChatState) -> bool {
        if self.finished {
            return false;
        }

        let Some(message) = self.resolve_assistant_message(messages) else {
            if let ChatState::Error(msg) = state {
                return self.finish_with_error(msg.to_string());
            }
            return true;
        };

        let switched = self
            .assistant_message_id
            .as_ref()
            .map(|id| id != &message.id)
            .unwrap_or(true);

        if switched {
            // The SDK started a new assistant message mid-turn (e.g. after a tool
            // result). Reset our part tracking and emit a fresh start event.
            self.part_states.clear();
            self.assistant_message_id = Some(message.id.clone());
            if !self.send_chunk(json!({
                "type": "start",
                "messageId": message.id,
            })) {
                return false;
            }
        }

        if !self.sync_parts(&message.parts) {
            return false;
        }

        match state {
            ChatState::Idle if self.assistant_message_id.is_some() => self.finish_success(),
            ChatState::Error(msg) => self.finish_with_error(msg.to_string()),
            _ => true,
        }
    }

    fn resolve_assistant_message<'a>(&mut self, messages: &'a [Message]) -> Option<&'a Message> {
        // The current assistant turn is always the most recent one not present when
        // the stream started. This handles the SDK creating a new assistant message
        // after a tool result while the same user request is still streaming.
        messages.iter().rev().find(|m| {
            matches!(m.role, Role::Assistant) && !self.initial_message_ids.contains(&m.id)
        })
    }

    fn sync_parts(&mut self, parts: &[MessagePart]) -> bool {
        for index in 0..parts.len() {
            if self.part_states.len() <= index {
                if !self.start_part(index, &parts[index]) {
                    return false;
                }
            }
            if !self.update_part(index, &parts[index]) {
                return false;
            }
        }
        true
    }

    fn start_part(&mut self, index: usize, part: &MessagePart) -> bool {
        match part {
            MessagePart::Text { .. } => {
                let id = format!("text-{}", index);
                if !self.send_chunk(json!({ "type": "text-start", "id": id })) {
                    return false;
                }
                self.part_states.push(AiPartState::Text {
                    id,
                    text: String::new(),
                    done: false,
                });
            }
            MessagePart::Reasoning { .. } => {
                let id = format!("reasoning-{}", index);
                if !self.send_chunk(json!({ "type": "reasoning-start", "id": id })) {
                    return false;
                }
                self.part_states.push(AiPartState::Reasoning {
                    id,
                    text: String::new(),
                    done: false,
                });
            }
            MessagePart::ToolCall {
                tool_call_id, name, ..
            } => {
                let id = stable_tool_call_id(tool_call_id, index);
                if !self.send_chunk(json!({
                    "type": "tool-input-start",
                    "toolCallId": id,
                    "toolName": name.to_string(),
                })) {
                    return false;
                }
                self.part_states.push(AiPartState::Tool {
                    id,
                    name: name.to_string(),
                    args: String::new(),
                    input_available: false,
                    output_available: false,
                });
            }
            MessagePart::ToolResult { .. } => {
                let previous_tool = self.part_states.iter().rev().find_map(|state| {
                    if let AiPartState::Tool { id, name, .. } = state {
                        Some((id.clone(), name.clone()))
                    } else {
                        None
                    }
                });
                let (id, name) =
                    previous_tool.unwrap_or_else(|| (format!("tool-{}", index), String::new()));
                self.part_states.push(AiPartState::Tool {
                    id,
                    name,
                    args: String::new(),
                    input_available: true,
                    output_available: false,
                });
            }
        }
        true
    }

    fn update_part(&mut self, index: usize, part: &MessagePart) -> bool {
        let Some(state) = self.part_states.get_mut(index).cloned() else {
            return true;
        };

        match (state, part) {
            (AiPartState::Text { id, text: _, done }, MessagePart::Text { text, state }) => {
                let text = text.to_string();
                let is_done = done || matches!(state, Some(PartState::Done));
                if is_done && !done {
                    if !text.is_empty()
                        && !self.send_chunk(json!({
                            "type": "text-delta",
                            "id": id,
                            "delta": text,
                        }))
                    {
                        return false;
                    }
                    if !self.send_chunk(json!({ "type": "text-end", "id": id })) {
                        return false;
                    }
                }
                self.part_states[index] = AiPartState::Text {
                    id,
                    text,
                    done: is_done,
                };
            }
            (
                AiPartState::Reasoning { id, text: _, done },
                MessagePart::Reasoning { text, state, .. },
            ) => {
                let text = text.to_string();
                let is_done = done || matches!(state, Some(PartState::Done));
                if is_done && !done {
                    if !text.is_empty()
                        && !self.send_chunk(json!({
                            "type": "reasoning-delta",
                            "id": id,
                            "delta": text,
                        }))
                    {
                        return false;
                    }
                    if !self.send_chunk(json!({ "type": "reasoning-end", "id": id })) {
                        return false;
                    }
                }
                self.part_states[index] = AiPartState::Reasoning {
                    id,
                    text,
                    done: is_done,
                };
            }
            (
                AiPartState::Tool {
                    id,
                    name,
                    args: _,
                    input_available,
                    output_available,
                },
                MessagePart::ToolCall {
                    name: part_name,
                    args,
                    state,
                    ..
                },
            ) => {
                let name = if name.is_empty() {
                    part_name.to_string()
                } else {
                    name
                };
                let args = args.to_string();
                let input_done = input_available || matches!(state, Some(PartState::Done));
                if input_done && !input_available {
                    if !args.is_empty()
                        && !self.send_chunk(json!({
                            "type": "tool-input-delta",
                            "toolCallId": id,
                            "inputTextDelta": args,
                        }))
                    {
                        return false;
                    }
                    if !self.send_chunk(json!({
                        "type": "tool-input-available",
                        "toolCallId": id,
                        "toolName": name,
                        "input": parse_tool_input(&args),
                    })) {
                        return false;
                    }
                }
                self.part_states[index] = AiPartState::Tool {
                    id,
                    name,
                    args,
                    input_available: input_done,
                    output_available,
                };
            }
            (
                AiPartState::Tool {
                    id,
                    name,
                    args,
                    input_available,
                    output_available,
                },
                MessagePart::ToolResult { output, .. },
            ) => {
                if !output_available
                    && !self.send_chunk(json!({
                        "type": "tool-output-available",
                        "toolCallId": id,
                        "output": output.to_string(),
                    }))
                {
                    return false;
                }
                self.part_states[index] = AiPartState::Tool {
                    id,
                    name,
                    args,
                    input_available,
                    output_available: true,
                };
            }
            _ => {}
        }

        true
    }

    fn finish_success(&mut self) -> bool {
        if !self.close_open_parts() {
            return false;
        }
        if !self.send_chunk(json!({ "type": "finish-step" })) {
            return false;
        }
        if !self.send_chunk(json!({ "type": "finish", "finishReason": "stop" })) {
            return false;
        }
        let _ = self.send_done();
        false
    }

    fn finish_with_error(&mut self, message: String) -> bool {
        if !self.close_open_parts() {
            return false;
        }
        if !self.send_chunk(json!({ "type": "error", "errorText": message })) {
            return false;
        }
        if !self.send_chunk(json!({ "type": "finish", "finishReason": "error" })) {
            return false;
        }
        let _ = self.send_done();
        false
    }

    fn close_open_parts(&mut self) -> bool {
        for index in 0..self.part_states.len() {
            match self.part_states[index].clone() {
                AiPartState::Text { id, text, done } if !done => {
                    if !text.is_empty()
                        && !self.send_chunk(json!({
                            "type": "text-delta",
                            "id": id,
                            "delta": text,
                        }))
                    {
                        return false;
                    }
                    if !self.send_chunk(json!({ "type": "text-end", "id": id })) {
                        return false;
                    }
                    self.part_states[index] = AiPartState::Text {
                        id,
                        text,
                        done: true,
                    };
                }
                AiPartState::Reasoning { id, text, done } if !done => {
                    if !text.is_empty()
                        && !self.send_chunk(json!({
                            "type": "reasoning-delta",
                            "id": id,
                            "delta": text,
                        }))
                    {
                        return false;
                    }
                    if !self.send_chunk(json!({ "type": "reasoning-end", "id": id })) {
                        return false;
                    }
                    self.part_states[index] = AiPartState::Reasoning {
                        id,
                        text,
                        done: true,
                    };
                }
                AiPartState::Tool {
                    id,
                    name,
                    args,
                    input_available,
                    output_available,
                } if !input_available => {
                    if !args.is_empty()
                        && !self.send_chunk(json!({
                            "type": "tool-input-delta",
                            "toolCallId": id,
                            "inputTextDelta": args,
                        }))
                    {
                        return false;
                    }
                    if !self.send_chunk(json!({
                        "type": "tool-input-available",
                        "toolCallId": id,
                        "toolName": name,
                        "input": parse_tool_input(&args),
                    })) {
                        return false;
                    }
                    self.part_states[index] = AiPartState::Tool {
                        id,
                        name,
                        args,
                        input_available: true,
                        output_available,
                    };
                }
                _ => {}
            }
        }
        true
    }
}

fn stable_tool_call_id(tool_call_id: &gpui::SharedString, index: usize) -> String {
    let id = tool_call_id.to_string();
    if id.is_empty() {
        format!("tool-{}", index)
    } else {
        id
    }
}

fn parse_tool_input(input: &str) -> Value {
    serde_json::from_str(input).unwrap_or_else(|_| json!(input))
}

impl RemoteController {
    pub fn new(_cx: &mut Context<Self>, config: RemoteControlConfig) -> Self {
        // The caller is responsible for ensuring `config.enabled` reflects the
        // desired startup state. We never auto-start from the constructor.
        Self {
            config,
            status: RemoteStatus::Disabled,
            tunnel_url: None,
            error_message: None,
            tunnel_log: None,
            command_sender: None,
            command_task: None,
            server_handle: None,
            tunnel_handle: None,
            watchdog_task: None,
            restart_attempts: 0,
            target_thread_id: None,
            target_session: None,
            session_subscription: None,
            active_streams: Vec::new(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    pub fn is_starting(&self) -> bool {
        matches!(self.status, RemoteStatus::Starting)
    }

    pub fn is_reconnecting(&self) -> bool {
        matches!(self.status, RemoteStatus::Reconnecting)
    }

    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.config.enabled == enabled || self.is_starting() {
            return;
        }
        self.config.enabled = enabled;
        self.save_config(cx);

        if enabled {
            self.start(cx);
        } else {
            self.stop(cx);
        }
    }

    pub fn save_config(&self, cx: &mut Context<Self>) {
        cx.update_global(|app: &mut AppStore, _| {
            app.config.remote_control = self.config.clone();
            if let Err(e) = app.config.save() {
                eprintln!("[remote] failed to save config: {}", e);
            }
        });
    }

    pub fn start(&mut self, cx: &mut Context<Self>) {
        if matches!(self.status, RemoteStatus::Starting | RemoteStatus::Running) {
            return;
        }
        self.restart_attempts = 0;
        self.begin_start(cx);
    }

    fn begin_start(&mut self, cx: &mut Context<Self>) {
        self.status = RemoteStatus::Starting;
        self.error_message = None;
        self.tunnel_log = None;
        self.tunnel_url = None;
        cx.emit(RemoteControllerEvent::StatusChanged);
        cx.notify();

        let (command_tx, command_rx) = mpsc::unbounded_channel::<CommandEnvelope>();
        self.command_sender = Some(command_tx.clone());
        self.start_command_task(command_rx, cx);

        let (server_handle, bound_port) = match server::start(
            self.config.bind_port,
            self.config.bearer_token.clone(),
            command_tx,
        ) {
            Ok(v) => v,
            Err(e) => {
                self.set_error(format!("HTTP server failed: {}", e), cx);
                return;
            }
        };
        self.server_handle = Some(server_handle);

        let this = cx.entity().downgrade();
        let command_path = self.config.cloudflared.command.clone();
        let token = self.config.cloudflared.tunnel_token.clone();
        let hostname = self.config.cloudflared.hostname.clone();
        let bearer_token = self.config.cloudflared.bearer_token.clone();
        let watchdog_attempts = self.restart_attempts;

        let watchdog_this = this.clone();
        self.watchdog_task = Some(cx.spawn(async move |_, cx| {
            smol::Timer::after(std::time::Duration::from_secs(STARTUP_WATCHDOG_SECONDS)).await;
            let _ = watchdog_this.update(cx, |this, cx| {
                if matches!(this.status, RemoteStatus::Starting)
                    && this.restart_attempts == watchdog_attempts
                {
                    this.restart(
                        "remote control startup timed out; check that cloudflared is installed and reachable"
                            .to_string(),
                        cx,
                    );
                }
            });
        }));

        cx.spawn(async move |_, cx| {
            let start_result = smol::unblock(move || {
                let command_path = cloudflared::resolve_cloudflared_command(&command_path)?;
                tunnel::start(
                    &command_path,
                    token.as_deref(),
                    hostname.as_deref(),
                    bearer_token.as_deref(),
                    bound_port,
                )
            })
            .await;

            match start_result {
                Ok((handle, url_rx)) => {
                    let keep = this
                        .update(cx, |this, cx| {
                            if !matches!(this.status, RemoteStatus::Starting) {
                                return false;
                            }
                            this.tunnel_handle = Some(handle);
                            cx.emit(RemoteControllerEvent::StatusChanged);
                            cx.notify();
                            true
                        })
                        .unwrap_or(false);
                    if !keep {
                        return;
                    }

                    // Forward tunnel outcomes from a blocking thread so we can keep
                    // monitoring for process exit after the URL is known.
                    let (fwd_tx, mut fwd_rx) = mpsc::unbounded_channel::<tunnel::TunnelOutcome>();
                    std::thread::spawn(move || {
                        let mut url_seen = false;
                        loop {
                            let timeout = if url_seen {
                                std::time::Duration::from_secs(5)
                            } else {
                                tunnel::URL_TIMEOUT
                            };
                            match url_rx.recv_timeout(timeout) {
                                Ok(outcome) => {
                                    if matches!(outcome, tunnel::TunnelOutcome::Url(_)) {
                                        url_seen = true;
                                    }
                                    if fwd_tx.send(outcome).is_err() {
                                        break;
                                    }
                                }
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                                    if !url_seen {
                                        let _ = fwd_tx.send(tunnel::TunnelOutcome::Error(
                                            "timed out waiting for cloudflared URL".to_string(),
                                        ));
                                        break;
                                    }
                                }
                                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                            }
                        }
                    });

                    let mut url_seen = false;
                    while let Some(outcome) = fwd_rx.recv().await {
                        match outcome {
                            tunnel::TunnelOutcome::Url(url) => {
                                let mut applied = false;
                                let _ = this.update(cx, |this, cx| {
                                    let should_apply = (!url_seen
                                        && matches!(this.status, RemoteStatus::Starting))
                                        || matches!(this.status, RemoteStatus::Reconnecting);
                                    if !should_apply {
                                        return;
                                    }
                                    applied = true;
                                    this.tunnel_url = Some(url);
                                    this.status = RemoteStatus::Running;
                                    this.error_message = None;
                                    this.tunnel_log = None;
                                    this.restart_attempts = 0;
                                    cx.emit(RemoteControllerEvent::StatusChanged);
                                    cx.notify();
                                });
                                if applied {
                                    url_seen = true;
                                }
                            }
                            tunnel::TunnelOutcome::Error(e) => {
                                let _ = this.update(cx, |this, cx| {
                                    if matches!(
                                        this.status,
                                        RemoteStatus::Starting
                                            | RemoteStatus::Running
                                            | RemoteStatus::Reconnecting
                                    ) {
                                        this.restart(e, cx);
                                    }
                                });
                                break;
                            }
                            tunnel::TunnelOutcome::Log { level, message } => {
                                let _ = this.update(cx, |this, cx| {
                                    if !matches!(
                                        this.status,
                                        RemoteStatus::Starting
                                            | RemoteStatus::Running
                                            | RemoteStatus::Reconnecting
                                    ) {
                                        return;
                                    }
                                    if level == "ERR"
                                        && matches!(
                                            this.status,
                                            RemoteStatus::Starting | RemoteStatus::Running
                                        )
                                    {
                                        this.status = RemoteStatus::Reconnecting;
                                        cx.emit(RemoteControllerEvent::StatusChanged);
                                        cx.notify();
                                    }
                                    this.set_tunnel_log(level, message, cx);
                                });
                            }
                            tunnel::TunnelOutcome::Connected => {
                                let _ = this.update(cx, |this, cx| {
                                    if matches!(
                                        this.status,
                                        RemoteStatus::Starting | RemoteStatus::Reconnecting
                                    ) {
                                        this.status = RemoteStatus::Running;
                                        this.error_message = None;
                                        this.tunnel_log = None;
                                        this.restart_attempts = 0;
                                        cx.emit(RemoteControllerEvent::StatusChanged);
                                        cx.notify();
                                    }
                                });
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        if matches!(this.status, RemoteStatus::Starting) {
                            this.set_error(e, cx);
                        }
                    });
                }
            }
        })
        .detach();
    }

    fn restart(&mut self, message: String, cx: &mut Context<Self>) {
        eprintln!("[remote] tunnel lost, will restart: {}", message);
        self.shutdown_services();
        self.status = RemoteStatus::Reconnecting;
        self.error_message = Some(message);
        self.tunnel_log = None;
        self.tunnel_url = None;
        self.target_thread_id = None;
        self.target_session = None;
        self.session_subscription = None;
        cx.emit(RemoteControllerEvent::StatusChanged);
        cx.notify();

        let delay = std::cmp::min(
            RESTART_BASE_DELAY * 2u32.pow(self.restart_attempts.min(RESTART_ATTEMPT_CAP)),
            RESTART_MAX_DELAY,
        );
        self.restart_attempts += 1;

        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            smol::Timer::after(delay).await;
            let _ = this.update(cx, |this, cx| {
                if this.config.enabled && matches!(this.status, RemoteStatus::Reconnecting) {
                    this.begin_start(cx);
                }
            });
        })
        .detach();
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.shutdown_services();
        self.restart_attempts = 0;
        self.tunnel_url = None;
        self.error_message = None;
        self.tunnel_log = None;
        self.status = RemoteStatus::Disabled;
        self.target_thread_id = None;
        self.target_session = None;
        self.session_subscription = None;
        cx.emit(RemoteControllerEvent::StatusChanged);
        cx.notify();
    }

    fn shutdown_services(&mut self) {
        self.tunnel_handle = None;
        self.server_handle = None;
        self.watchdog_task = None;
        self.command_sender = None;
        self.command_task = None;
        self.finish_active_streams_with_error("remote control stopped");
    }

    fn set_error(&mut self, message: String, cx: &mut Context<Self>) {
        eprintln!("[remote] {}", message);
        self.shutdown_services();
        self.status = RemoteStatus::Error(message.clone());
        self.error_message = Some(message);
        self.tunnel_log = None;
        self.tunnel_url = None;
        self.config.enabled = false;
        self.target_thread_id = None;
        self.target_session = None;
        self.session_subscription = None;
        self.save_config(cx);
        cx.emit(RemoteControllerEvent::StatusChanged);
        cx.notify();
    }

    fn set_tunnel_log(&mut self, level: String, message: String, cx: &mut Context<Self>) {
        self.tunnel_log = Some(TunnelLog { level, message });
        cx.emit(RemoteControllerEvent::StatusChanged);
        cx.notify();
    }

    fn start_command_task(
        &mut self,
        mut rx: UnboundedReceiver<CommandEnvelope>,
        cx: &mut Context<Self>,
    ) {
        let task = cx.spawn(async move |this, cx| {
            while let Some(envelope) = rx.recv().await {
                let respond_to = envelope.respond_to;
                let command = envelope.command;
                let result = this.update(cx, |this, cx| this.handle_command(command, cx));
                let value = match result {
                    Ok(value) => value,
                    Err(_) => json!({ "error": "command failed" }),
                };
                let _ = respond_to.send(value);
            }
        });
        self.command_task = Some(task);
    }

    fn handle_command(&mut self, command: RemoteCommand, cx: &mut Context<Self>) -> RemoteResponse {
        match command {
            RemoteCommand::Status => self.status_response(),
            RemoteCommand::GetModels => self.get_models_response(cx),
            RemoteCommand::ListWorkspaces => self.list_workspaces_response(cx),
            RemoteCommand::ListThreads { page, per_page } => {
                self.list_threads_response(page, per_page, cx)
            }
            RemoteCommand::CreateThread {
                workspace_id,
                model_id,
            } => self.create_thread(workspace_id, model_id, cx),
            RemoteCommand::OpenThread { thread_id } => self.open_thread(thread_id, cx),
            RemoteCommand::SendMessageStream {
                thread_id,
                message,
                sender,
            } => self.send_message_stream(thread_id, message, sender, cx),
            RemoteCommand::GetMessages {
                thread_id,
                since_id,
            } => self.get_messages(thread_id, since_id, cx),
            RemoteCommand::Abort { thread_id } => self.abort(thread_id, cx),
            RemoteCommand::SetModel {
                thread_id,
                model_id,
            } => self.set_model(thread_id, model_id, cx),
            RemoteCommand::SetThinkingLevel {
                thread_id,
                thinking_level,
            } => self.set_thinking_level(thread_id, thinking_level, cx),
            RemoteCommand::SetWorkspace {
                thread_id,
                workspace_id,
            } => self.set_workspace(thread_id, workspace_id, cx),
            RemoteCommand::DownloadFile { path, mime_type } => {
                self.download_file(path, mime_type, cx)
            }
        }
    }

    fn status_response(&self) -> RemoteResponse {
        json!({
            "enabled": self.config.enabled,
            "status": self.status.label(),
            "status_detail": self.status.detail(),
            "tunnel_url": self.tunnel_url,
            "target_thread_id": self.target_thread_id,
        })
    }

    fn get_models_response(&self, cx: &mut Context<Self>) -> RemoteResponse {
        let models = &cx.global::<AppStore>().models;
        let models_json: Vec<serde_json::Value> = models
            .iter()
            .filter_map(|m| {
                let (provider, id) = model_config::parse_model_id(&m.id)?;
                Some(json!({
                    "provider": provider,
                    "id": id,
                    "name": m.name,
                    "thinking_level_map": m.thinking_level_map,
                }))
            })
            .collect();
        json!({ "models": models_json })
    }

    fn list_workspaces_response(&self, cx: &mut Context<Self>) -> RemoteResponse {
        let store = cx.global::<AppStore>().store.clone();
        match store.list_workspaces() {
            Ok(workspaces) => {
                let workspaces_json: Vec<serde_json::Value> = workspaces
                    .into_iter()
                    .map(|ws| {
                        json!({
                            "id": ws.id,
                            "name": ws.name,
                            "path": ws.path,
                            "created_at": ws.created_at,
                            "updated_at": ws.updated_at,
                        })
                    })
                    .collect();
                json!({ "workspaces": workspaces_json })
            }
            Err(e) => json!({ "error": e.to_string() }),
        }
    }

    fn list_threads_response(
        &self,
        page: usize,
        per_page: usize,
        cx: &mut Context<Self>,
    ) -> RemoteResponse {
        let store = cx.global::<AppStore>().store.clone();
        match store.list_threads_paginated(page, per_page) {
            Ok(result) => {
                let total_pages = if result.total == 0 {
                    0
                } else {
                    (result.total + result.per_page - 1) / result.per_page
                };
                json!({
                    "threads": result.threads.iter().map(thread_to_json).collect::<Vec<_>>(),
                    "pagination": {
                        "page": result.page,
                        "per_page": result.per_page,
                        "total": result.total,
                        "total_pages": total_pages,
                    }
                })
            }
            Err(e) => json!({ "error": e.to_string() }),
        }
    }

    fn create_thread(
        &mut self,
        workspace_id: Option<String>,
        model_id: Option<String>,
        cx: &mut Context<Self>,
    ) -> RemoteResponse {
        let models = &cx.global::<AppStore>().models;
        if let Some(ref id) = model_id {
            if model_config::get_model_name(models, id).is_none() {
                return json!({ "error": format!("unknown model_id: {}", id) });
            }
        }

        let store = cx.global::<AppStore>().store.clone();
        let thread = match store.create_thread("", "") {
            Ok(t) => t,
            Err(e) => return json!({ "error": e.to_string() }),
        };

        let workspace = match self.resolve_workspace(workspace_id.as_deref(), cx) {
            Ok(ws) => ws,
            Err(e) => return json!({ "error": e }),
        };

        let mut metadata = json!({});
        if let Some(ref ws) = workspace {
            metadata["workspace_id"] = json!(ws.id);
        }

        let session_file = self.generate_session_file();
        if let Err(e) = store.update_thread(
            &thread.id,
            None,
            None,
            Some(Some(&session_file)),
            Some(model_id.as_deref()),
            None,
            None,
            Some(Some(&metadata)),
            false,
        ) {
            return json!({ "error": e.to_string() });
        }

        let thread_id = thread.id.clone();
        let session = cx.new(|cx| {
            SessionHandle::new(
                cx,
                Some(thread_id.clone()),
                session_file,
                workspace,
                model_id,
                None,
                store,
                false,
            )
        });
        let session_file = session.read(cx).session_file.clone();
        cx.update_global(|app: &mut AppStore, _| {
            app.session_manager.register(session_file, session.clone());
        });

        self.set_target_internal(thread_id.clone(), Some(session.downgrade()), cx);

        json!({ "thread_id": thread_id })
    }

    fn open_thread(&mut self, thread_id: String, cx: &mut Context<Self>) -> RemoteResponse {
        let store = cx.global::<AppStore>().store.clone();
        let thread = match store.get_thread(&thread_id) {
            Ok(Some(t)) => t,
            Ok(None) => return json!({ "error": "thread not found" }),
            Err(e) => return json!({ "error": e.to_string() }),
        };

        let session_file = match thread.session_file.clone() {
            Some(sf) => sf,
            None => {
                let sf = self.generate_session_file();
                if let Err(e) = store.update_thread(
                    &thread_id,
                    None,
                    None,
                    Some(Some(&sf)),
                    None,
                    None,
                    None,
                    None,
                    false,
                ) {
                    return json!({ "error": e.to_string() });
                }
                sf
            }
        };

        let session =
            cx.update_global(|app: &mut AppStore, _| app.session_manager.get(&session_file));

        let session = match session {
            Some(s) => s,
            None => {
                let workspace = match thread
                    .metadata
                    .as_ref()
                    .and_then(|md| md.get("workspace_id").and_then(|v| v.as_str()))
                    .map(|id| self.resolve_workspace(Some(id), cx))
                    .unwrap_or_else(|| self.resolve_workspace(None, cx))
                {
                    Ok(ws) => ws,
                    Err(e) => return json!({ "error": e }),
                };
                let session = cx.new(|cx| {
                    SessionHandle::new(
                        cx,
                        Some(thread_id.clone()),
                        session_file,
                        workspace,
                        thread.model.clone(),
                        thread.thinking_level.clone(),
                        store,
                        true,
                    )
                });
                let session_file = session.read(cx).session_file.clone();
                cx.update_global(|app: &mut AppStore, _| {
                    app.session_manager.register(session_file, session.clone());
                });
                session
            }
        };

        self.set_target_internal(thread_id.clone(), Some(session.downgrade()), cx);
        json!({ "thread_id": thread_id })
    }

    fn send_message_stream(
        &mut self,
        thread_id: String,
        message: String,
        sender: UnboundedSender<AiStreamEvent>,
        cx: &mut Context<Self>,
    ) -> RemoteResponse {
        let session = match self.ensure_target_session(thread_id.clone(), cx) {
            Ok(Some(s)) => s,
            Ok(None) => return json!({ "error": "could not open thread" }),
            Err(e) => return json!({ "error": e }),
        };

        let initial_message_ids = session.update(cx, |session, _cx| {
            session
                .messages
                .iter()
                .map(|message| message.id.clone())
                .collect::<HashSet<_>>()
        });

        let initial_ids_for_acceptance = initial_message_ids.clone();
        let accepted = session.update(cx, |session, cx| {
            session.send_message(message.into(), cx);
            session.messages.iter().any(|m| {
                matches!(m.role, Role::User) && !initial_ids_for_acceptance.contains(&m.id)
            })
        });

        if accepted {
            self.active_streams
                .push(AiSubmitStream::new(thread_id, sender, initial_message_ids));
            self.on_session_changed(cx);
            json!({ "status": "streaming" })
        } else {
            json!({ "error": "message was not accepted" })
        }
    }

    fn get_messages(
        &self,
        thread_id: String,
        since_id: Option<String>,
        cx: &mut Context<Self>,
    ) -> RemoteResponse {
        let messages = self
            .resolve_session(&thread_id, cx)
            .map(|e| e.read(cx).messages.clone());

        match messages {
            Some(msgs) => {
                let start_idx = since_id
                    .as_ref()
                    .and_then(|sid| msgs.iter().position(|m| m.id == *sid))
                    .map(|i| i + 1)
                    .unwrap_or(0);
                json!(
                    msgs.iter()
                        .skip(start_idx)
                        .map(message_to_json)
                        .collect::<Vec<_>>()
                )
            }
            None => json!({ "error": "thread not found" }),
        }
    }

    fn abort(&self, thread_id: String, cx: &mut Context<Self>) -> RemoteResponse {
        if let Some(session) = self.find_session(&thread_id, cx) {
            if let Some(session) = session.upgrade() {
                session.update(cx, |session, _cx| session.abort(_cx));
            }
        }
        json!({ "status": "aborted" })
    }

    fn set_model(
        &self,
        thread_id: String,
        model_id: String,
        cx: &mut Context<Self>,
    ) -> RemoteResponse {
        let models = &cx.global::<AppStore>().models;
        if model_config::get_model_name(models, &model_id).is_none() {
            return json!({ "error": format!("unknown model_id: {}", model_id) });
        }
        if let Some(session) = self.find_session(&thread_id, cx) {
            if let Some(session) = session.upgrade() {
                session.update(cx, |session, cx| session.set_model(Some(model_id), cx));
                return json!({ "status": "ok" });
            }
        }
        json!({ "error": "thread not found" })
    }

    fn set_thinking_level(
        &self,
        thread_id: String,
        thinking_level: String,
        cx: &mut Context<Self>,
    ) -> RemoteResponse {
        if let Some(session) = self.find_session(&thread_id, cx) {
            if let Some(session) = session.upgrade() {
                session.update(cx, |session, cx| {
                    session.set_thinking_level(Some(thinking_level), cx);
                });
                return json!({ "status": "ok" });
            }
        }
        json!({ "error": "thread not found" })
    }

    fn set_workspace(
        &self,
        thread_id: String,
        workspace_id: String,
        cx: &mut Context<Self>,
    ) -> RemoteResponse {
        let workspace = match self.resolve_workspace(Some(&workspace_id), cx) {
            Ok(Some(ws)) => ws,
            Ok(None) => return json!({ "error": "workspace not found" }),
            Err(e) => return json!({ "error": e }),
        };
        if let Some(session) = self.find_session(&thread_id, cx) {
            if let Some(session) = session.upgrade() {
                let ws = workspace.clone();
                session.update(cx, |session, _cx| session.set_workspace(ws));
                if let Err(e) = self.persist_workspace_id(&thread_id, &workspace_id, cx) {
                    return json!({ "error": e.to_string() });
                }
                return json!({ "status": "ok" });
            }
        }
        json!({ "error": "thread not found" })
    }

    fn download_file(
        &self,
        path: String,
        mime_type: Option<String>,
        cx: &mut Context<Self>,
    ) -> RemoteResponse {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return json!({ "error": "path must be absolute" });
        }

        let store = cx.global::<AppStore>().store.clone();
        let workspaces = match store.list_workspaces() {
            Ok(ws) => ws,
            Err(e) => return json!({ "error": e.to_string() }),
        };

        let canonical_path = match std::fs::canonicalize(&path) {
            Ok(p) => p,
            Err(e) => return json!({ "error": format!("invalid path: {}", e) }),
        };

        let allowed = workspaces.iter().any(|ws| {
            let ws_path = PathBuf::from(&ws.path);
            let canonical_ws = std::fs::canonicalize(&ws_path).unwrap_or(ws_path);
            canonical_path.starts_with(&canonical_ws)
        });

        if !allowed {
            return json!({ "error": "path outside workspace" });
        }

        match std::fs::read(&canonical_path) {
            Ok(bytes) => {
                let name = canonical_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("download")
                    .to_string();
                let mime_type = mime_type.unwrap_or_else(|| guess_mime_type(&canonical_path));
                use base64::Engine;
                let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                json!({
                    "name": name,
                    "mime_type": mime_type,
                    "size": bytes.len(),
                    "data": data,
                })
            }
            Err(e) => json!({ "error": format!("failed to read file: {}", e) }),
        }
    }

    fn persist_workspace_id(
        &self,
        thread_id: &str,
        workspace_id: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), StoreError> {
        let store = cx.global::<AppStore>().store.clone();
        let md = store
            .get_thread(thread_id)?
            .map(|t| t.metadata.unwrap_or_else(|| json!({})))
            .unwrap_or_else(|| json!({}));
        let mut md = md;
        md["workspace_id"] = json!(workspace_id);
        store.update_thread(
            thread_id,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(Some(&md)),
            false,
        )
    }

    fn ensure_target_session(
        &mut self,
        thread_id: String,
        cx: &mut Context<Self>,
    ) -> Result<Option<Entity<SessionHandle>>, String> {
        if self.target_thread_id.as_ref() == Some(&thread_id) {
            if let Some(ref weak) = self.target_session {
                if let Some(session) = weak.upgrade() {
                    return Ok(Some(session));
                }
            }
        }
        let response = self.open_thread(thread_id, cx);
        if response.get("error").is_some() {
            return Err(response["error"]
                .as_str()
                .unwrap_or("open thread failed")
                .to_string());
        }
        Ok(self.target_session.as_ref().and_then(|w| w.upgrade()))
    }

    fn resolve_session(
        &self,
        thread_id: &str,
        cx: &mut Context<Self>,
    ) -> Option<Entity<SessionHandle>> {
        if self.target_thread_id.as_ref().map(|s| s.as_str()) == Some(thread_id) {
            if let Some(ref weak) = self.target_session {
                if let Some(session) = weak.upgrade() {
                    return Some(session);
                }
            }
        }
        let store = cx.global::<AppStore>().store.clone();
        store
            .get_thread(thread_id)
            .ok()
            .flatten()
            .and_then(|t| t.session_file)
            .and_then(|sf| cx.update_global(|app: &mut AppStore, _| app.session_manager.get(&sf)))
    }

    fn find_session(
        &self,
        thread_id: &str,
        cx: &mut Context<Self>,
    ) -> Option<WeakEntity<SessionHandle>> {
        self.resolve_session(thread_id, cx).map(|e| e.downgrade())
    }

    fn set_target_internal(
        &mut self,
        thread_id: String,
        session: Option<WeakEntity<SessionHandle>>,
        cx: &mut Context<Self>,
    ) {
        self.target_thread_id = Some(thread_id);
        self.target_session = session.clone();
        self.session_subscription = session.and_then(|session| {
            let entity = session.upgrade()?;
            Some(cx.subscribe(
                &entity,
                move |this: &mut RemoteController, _session, _event: &SessionEvent, cx| {
                    this.on_session_changed(cx);
                },
            ))
        });
        cx.emit(RemoteControllerEvent::StatusChanged);
        cx.notify();
    }

    fn on_session_changed(&mut self, cx: &mut Context<Self>) {
        let Some(ref thread_id) = self.target_thread_id else {
            return;
        };
        let Some(ref weak) = self.target_session else {
            return;
        };
        let Some(session) = weak.upgrade() else {
            // Session was dropped; release the target so we don't leak references.
            self.target_thread_id = None;
            self.target_session = None;
            self.session_subscription = None;
            return;
        };
        let session = session.read(cx);
        let messages = session.messages.clone();
        let state = session.state.clone();

        self.active_streams.retain_mut(|stream| {
            stream.thread_id != *thread_id || stream.update(&messages, &state)
        });
    }

    fn finish_active_streams_with_error(&mut self, message: &str) {
        for stream in self.active_streams.iter_mut() {
            let _ = stream.finish_with_error(message.to_string());
        }
        self.active_streams.clear();
    }

    fn resolve_workspace(
        &self,
        workspace_id: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Result<Option<WorkspaceInfo>, String> {
        let store = cx.global::<AppStore>().store.clone();
        let workspaces = store.list_workspaces().map_err(|e| e.to_string())?;
        match workspace_id {
            Some(id) => workspaces
                .into_iter()
                .find(|w| w.id == id)
                .map(|ws| {
                    Some(WorkspaceInfo {
                        id: ws.id.clone(),
                        path: PathBuf::from(&ws.path),
                        name: ws.name,
                    })
                })
                .ok_or_else(|| format!("workspace {} not found", id)),
            None => {
                // Prefer the "Default" workspace used by the local UI so remote
                // sessions start in ~/.mini-pi/workspace instead of whichever
                // workspace happened to be created first.
                if let Some(ws) = workspaces.iter().find(|w| w.name == "Default") {
                    return Ok(Some(WorkspaceInfo {
                        id: ws.id.clone(),
                        path: PathBuf::from(&ws.path),
                        name: ws.name.clone(),
                    }));
                }

                let default_dir = store.default_workspace_dir();
                std::fs::create_dir_all(&default_dir).map_err(|e| e.to_string())?;
                let default_path_str = default_dir.to_string_lossy().to_string();
                let ws = store
                    .create_workspace("Default", &default_path_str)
                    .map_err(|e| e.to_string())?;
                Ok(Some(WorkspaceInfo {
                    id: ws.id,
                    path: PathBuf::from(&ws.path),
                    name: ws.name,
                }))
            }
        }
    }

    fn generate_session_file(&self) -> String {
        format!("session_{}.jsonl", uuid::Uuid::new_v4())
    }
}

fn thread_to_json(t: &ThreadMeta) -> serde_json::Value {
    json!({
        "id": t.id,
        "title": t.title,
        "preview": t.preview,
        "session_file": t.session_file,
        "model": t.model,
        "thinking_level": t.thinking_level,
        "pinned": t.pinned,
        "metadata": t.metadata,
        "created_at": t.created_at,
        "updated_at": t.updated_at,
    })
}

fn message_to_json(m: &Message) -> serde_json::Value {
    json!({
        "id": m.id,
        "entry_id": m.entry_id,
        "role": match m.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        },
        "parts": m.parts.iter().map(part_to_json).collect::<Vec<_>>(),
    })
}

fn guess_mime_type(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "txt" => "text/plain",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "application/javascript",
        "json" => "application/json",
        "ts" => "application/typescript",
        "py" => "text/x-python",
        "rs" => "text/x-rust",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn part_to_json(p: &MessagePart) -> serde_json::Value {
    match p {
        MessagePart::Text { text, state } => json!({
            "type": "text",
            "text": text.to_string(),
            "state": state.as_ref().map(|s| format!("{:?}", s)),
        }),
        MessagePart::Reasoning { text, state, .. } => json!({
            "type": "thinking",
            "text": text.to_string(),
            "state": state.as_ref().map(|s| format!("{:?}", s)),
        }),
        MessagePart::ToolCall {
            name, args, state, ..
        } => json!({
            "type": "tool_call",
            "name": name.to_string(),
            "args": args.to_string(),
            "state": state.as_ref().map(|s| format!("{:?}", s)),
        }),
        MessagePart::ToolResult {
            name,
            output,
            state,
            details,
            ..
        } => json!({
            "type": "tool_result",
            "name": name.to_string(),
            "output": output.to_string(),
            "state": state.as_ref().map(|s| format!("{:?}", s)),
            "details": details,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::SharedString;

    fn assistant_with_text(id: &str, text: &str, state: Option<PartState>) -> Message {
        Message {
            id: id.to_string(),
            entry_id: None,
            role: Role::Assistant,
            parts: vec![MessagePart::Text {
                text: SharedString::from(text.to_string()),
                state,
            }],
            media: Vec::new(),
        }
    }

    fn assistant_empty(id: &str) -> Message {
        Message {
            id: id.to_string(),
            entry_id: None,
            role: Role::Assistant,
            parts: vec![],
            media: Vec::new(),
        }
    }

    fn recv_chunk_type(rx: &mut UnboundedReceiver<AiStreamEvent>) -> Option<String> {
        match rx.try_recv().ok()? {
            AiStreamEvent::Chunk(value) => value
                .get("type")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string()),
            AiStreamEvent::Done => Some("[DONE]".to_string()),
        }
    }

    #[test]
    fn ai_submit_stream_closes_after_done() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut stream = AiSubmitStream::new("1".to_string(), tx, HashSet::new());
        let message = assistant_with_text("assistant-1", "hello", Some(PartState::Done));

        assert!(!stream.update(&[message], &ChatState::Idle));

        let mut types = Vec::new();
        while let Some(kind) = recv_chunk_type(&mut rx) {
            types.push(kind);
        }

        assert_eq!(
            types,
            vec![
                "start",
                "text-start",
                "text-delta",
                "text-end",
                "finish-step",
                "finish",
                "[DONE]"
            ]
        );
    }

    #[test]
    fn ai_submit_stream_ignores_rewritten_non_suffix_text() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut stream = AiSubmitStream::new("1".to_string(), tx, HashSet::new());
        let first = assistant_with_text("assistant-1", "abcd", Some(PartState::Streaming));
        let rewritten = assistant_with_text("assistant-1", "éfg", Some(PartState::Streaming));

        assert!(stream.update(&[first], &ChatState::Streaming));
        assert!(stream.update(&[rewritten], &ChatState::Streaming));

        let mut deltas = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let AiStreamEvent::Chunk(value) = event
                && value.get("type").and_then(|value| value.as_str()) == Some("text-delta")
            {
                deltas.push(value["delta"].as_str().unwrap_or_default().to_string());
            }
        }

        assert!(
            deltas.is_empty(),
            "text deltas should be coalesced until the part is done"
        );
    }

    #[test]
    fn ai_submit_stream_emits_final_parts_after_empty_placeholder() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut stream = AiSubmitStream::new("1".to_string(), tx, HashSet::new());
        let empty = assistant_empty("assistant-1");
        let final_message = assistant_with_text("assistant-1", "done", Some(PartState::Done));

        assert!(stream.update(&[empty], &ChatState::Streaming));
        assert!(!stream.update(&[final_message], &ChatState::Idle));

        let mut chunks = Vec::new();
        while let Ok(event) = rx.try_recv() {
            chunks.push(event);
        }

        assert!(chunks.iter().any(|event| matches!(
            event,
            AiStreamEvent::Chunk(value)
                if value.get("type").and_then(|value| value.as_str()) == Some("text-delta")
                    && value.get("delta").and_then(|value| value.as_str()) == Some("done")
        )));
        assert!(matches!(chunks.last(), Some(AiStreamEvent::Done)));
    }

    #[test]
    fn ai_submit_stream_finishes_empty_assistant_message() {
        // Reproduces the reported SSE output where the assistant message is
        // created but never receives any text parts before the turn ends.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut stream = AiSubmitStream::new("1".to_string(), tx, HashSet::new());
        let empty = assistant_empty("assistant-1");

        assert!(stream.update(&[empty.clone()], &ChatState::Streaming));
        assert!(!stream.update(&[empty], &ChatState::Idle));

        let mut types = Vec::new();
        while let Some(kind) = recv_chunk_type(&mut rx) {
            types.push(kind);
        }

        assert_eq!(types, vec!["start", "finish-step", "finish", "[DONE]"]);
        assert!(
            types
                .iter()
                .all(|t| t != "text-start" && t != "text-delta" && t != "text-end"),
            "empty assistant message should not emit text chunks"
        );
    }

    #[test]
    fn ai_submit_stream_emits_text_deltas_after_empty_start() {
        // Verifies that real chunks are forwarded when the assistant message
        // starts empty and is populated during streaming.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut stream = AiSubmitStream::new("1".to_string(), tx, HashSet::new());
        let empty = assistant_empty("assistant-1");
        let streaming = assistant_with_text("assistant-1", "hello", Some(PartState::Streaming));
        let done = assistant_with_text("assistant-1", "hello", Some(PartState::Done));

        assert!(stream.update(&[empty], &ChatState::Streaming));
        assert!(stream.update(&[streaming], &ChatState::Streaming));
        assert!(!stream.update(&[done], &ChatState::Idle));

        let mut types = Vec::new();
        while let Some(kind) = recv_chunk_type(&mut rx) {
            types.push(kind);
        }

        assert_eq!(
            types,
            vec![
                "start",
                "text-start",
                "text-delta",
                "text-end",
                "finish-step",
                "finish",
                "[DONE]"
            ]
        );
    }

    #[test]
    fn ai_submit_stream_prefers_latest_assistant_message() {
        // Regression: a stale assistant message from a previous turn may still
        // carry a Streaming text part. The stream must latch onto the most recent
        // assistant message, not the stale one.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut stream = AiSubmitStream::new("1".to_string(), tx, HashSet::new());

        let stale = Message {
            id: "stale".to_string(),
            entry_id: None,
            role: Role::Assistant,
            parts: vec![MessagePart::Text {
                text: SharedString::from("old streaming text"),
                state: Some(PartState::Streaming),
            }],
            media: Vec::new(),
        };
        let new_empty = assistant_empty("new");
        let new_with_text = assistant_with_text("new", "hello", Some(PartState::Streaming));
        let new_done = assistant_with_text("new", "hello", Some(PartState::Done));

        assert!(stream.update(&[stale, new_empty], &ChatState::Streaming));
        assert!(stream.update(&[new_with_text.clone()], &ChatState::Streaming));
        assert!(!stream.update(&[new_done], &ChatState::Idle));

        let mut deltas = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let AiStreamEvent::Chunk(value) = event
                && value.get("type").and_then(|v| v.as_str()) == Some("text-delta")
            {
                deltas.push(value["delta"].as_str().unwrap_or_default().to_string());
            }
        }

        assert_eq!(deltas, vec!["hello"]);
    }

    #[test]
    fn ai_submit_stream_switches_to_new_assistant_after_tool_call() {
        // Regression: the SDK creates a new assistant message after a tool result
        // while the same user request is still streaming. The stream must switch
        // to the new assistant message and emit its text chunks.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut stream = AiSubmitStream::new("1".to_string(), tx, HashSet::new());

        let first = Message {
            id: "first".to_string(),
            entry_id: None,
            role: Role::Assistant,
            parts: vec![
                MessagePart::ToolCall {
                    name: SharedString::from("kimi_datasource"),
                    args: SharedString::from("{\"data_source_name\":\"stock\"}"),
                    state: Some(PartState::Done),
                    tool_call_id: SharedString::from("tool-1"),
                },
                MessagePart::ToolResult {
                    name: SharedString::from("kimi_datasource"),
                    output: SharedString::from("result"),
                    state: Some(PartState::Done),
                    tool_call_id: SharedString::from("tool-1"),
                    details: None,
                },
            ],
            media: Vec::new(),
        };
        let second_empty = assistant_empty("second");
        let second_with_text =
            assistant_with_text("second", "腾讯股价...", Some(PartState::Streaming));
        let second_done = assistant_with_text("second", "腾讯股价...", Some(PartState::Done));

        assert!(stream.update(&[first.clone()], &ChatState::Streaming));
        assert!(stream.update(&[first, second_empty], &ChatState::Streaming));
        assert!(stream.update(&[second_with_text.clone()], &ChatState::Streaming));
        assert!(!stream.update(&[second_done], &ChatState::Idle));

        let mut types = Vec::new();
        while let Some(kind) = recv_chunk_type(&mut rx) {
            types.push(kind);
        }

        assert_eq!(
            types,
            vec![
                "start",
                "tool-input-start",
                "tool-input-delta",
                "tool-input-available",
                "tool-output-available",
                "start",
                "text-start",
                "text-delta",
                "text-end",
                "finish-step",
                "finish",
                "[DONE]"
            ]
        );
    }
}
