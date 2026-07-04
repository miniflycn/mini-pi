use std::collections::HashMap;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use uuid::Uuid;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use crate::utils::format::truncate_str;

// ---------------------------------------------------------------------------
// Bridge Events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum BridgeEvent {
    AgentStart,
    AgentEnd {
        messages: Option<Vec<LoadedMessage>>,
    },
    Disconnected,
    MessageStart {
        message: Option<serde_json::Value>,
    },
    MessageEnd,
    TextStart,
    TextDelta {
        content: String,
    },
    TextEnd {
        content: String,
    },
    ThinkingStart,
    ThinkingDelta {
        content: String,
    },
    ThinkingEnd {
        content: String,
    },
    ToolCallStart {
        name: String,
        call_id: String,
    },
    ToolCallDelta {
        call_id: String,
        delta: String,
    },
    ToolCallEnd {
        call_id: String,
        name: String,
        args: serde_json::Value,
    },
    ToolStart {
        name: String,
        args: Option<serde_json::Value>,
        call_id: String,
    },
    ToolUpdate {
        call_id: String,
        tool_name: String,
        partial_output: String,
    },
    ToolEnd {
        call_id: String,
        tool_name: String,
        output: String,
        is_error: bool,
        details: Option<serde_json::Value>,
    },
    TurnStart,
    TurnEnd,
    Error {
        message: String,
    },
    ExtensionUiRequest {
        id: String,
        method: String,
        payload: serde_json::Value,
    },
    ExtensionError {
        extension_path: String,
        event: String,
        error: String,
    },
    Response {
        command: String,
        success: bool,
        data: Option<serde_json::Value>,
        error: Option<String>,
        request_id: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub enum LoadedPart {
    Text { text: String },
    Image { data: String, mime_type: String },
    Thinking { text: String },
    ToolCall { name: String, args: String },
    ToolResult { name: String, output: String },
}

#[derive(Debug, Clone)]
pub struct LoadedMessage {
    pub id: Option<String>,
    pub role: String,
    pub parts: Vec<LoadedPart>,
    pub error_message: Option<String>,
    pub is_error: bool,
}

pub fn is_error_stop_reason(stop_reason: Option<&str>) -> bool {
    matches!(stop_reason, Some("error") | Some("api_error"))
}

pub fn has_non_empty_error_message(error_message: Option<&str>) -> bool {
    error_message.map_or(false, |e| !e.is_empty())
}

pub fn is_assistant_error(error_message: Option<&str>, stop_reason: Option<&str>) -> bool {
    has_non_empty_error_message(error_message) || is_error_stop_reason(stop_reason)
}

// ---------------------------------------------------------------------------
// Image Content
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ImageContent {
    pub data: String,
    pub mime_type: String,
}

#[derive(Debug, Clone)]
pub struct BridgeModel {
    pub provider: String,
    pub id: String,
    pub name: String,
    pub thinking_level_map: Option<HashMap<String, Option<String>>>,
}

#[derive(Debug, Clone)]
pub struct BridgeSkill {
    pub name: String,
    pub description: Option<String>,
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct BridgeExtension {
    pub name: String,
    pub description: Option<String>,
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct BridgePrompt {
    pub name: String,
    pub description: Option<String>,
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct BridgeProvider {
    pub id: String,
    pub name: String,
    pub configured: bool,
}

#[derive(Debug, Clone)]
pub struct BridgeSettings {
    pub compaction_enabled: bool,
    pub default_thinking_level: Option<String>,
    pub default_model: Option<String>,
    pub default_provider: Option<String>,
}

// ---------------------------------------------------------------------------
// Macros
// ---------------------------------------------------------------------------

macro_rules! log {
    ($($arg:tt)*) => {
        eprintln!("[pi-rpc] {}", format!($($arg)*))
    };
}

// ---------------------------------------------------------------------------
// PiBridge: shared WebSocket connection to the SDK bridge
// ---------------------------------------------------------------------------

pub struct PiBridge {
    write_tx: tokio::sync::mpsc::UnboundedSender<String>,
    sessions: Arc<Mutex<HashMap<String, futures::channel::mpsc::UnboundedSender<BridgeEvent>>>>,
    child: Arc<Mutex<std::process::Child>>,
    runtime: tokio::runtime::Runtime,
}

impl Drop for PiBridge {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}

/// Ensures a temporary session entry is removed from the bridge's session map
/// even if the caller panics or the future is dropped early.
struct SessionGuard {
    sessions: Arc<Mutex<HashMap<String, futures::channel::mpsc::UnboundedSender<BridgeEvent>>>>,
    session_id: String,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let mut sessions = self.sessions.lock().unwrap();
        sessions.remove(&self.session_id);
    }
}

impl PiBridge {
    pub fn spawn() -> Result<Arc<Self>, PiRpcError> {
        let bridge_dir = crate::utils::paths::app_root().join("pi-bridge");
        let (program, args) = find_runtime(&bridge_dir)?;

        let agent_dir = dirs::home_dir()
            .map(|h| h.join(".mini-pi").join("agent"))
            .unwrap_or_else(|| bridge_dir.join("agent"));

        let mut cmd = Command::new(&program);
        cmd.args(&args)
            .current_dir(&bridge_dir)
            .arg("--agent-dir")
            .arg(&agent_dir);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(windows)]
        {
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }

        log!("spawning bridge: {:?} {:?}", cmd, args);
        let mut child = cmd.spawn().map_err(|e| {
            log!("failed to spawn pi bridge: {}", e);
            PiRpcError::Spawn(format!("failed to spawn pi bridge with {}: {}", program, e))
        })?;
        log!("pi bridge spawned, pid={}", child.id());

        let stdout = child.stdout.take().ok_or(PiRpcError::Stdout)?;
        let port = read_bridge_port(stdout)?;
        log!("pi bridge listening on port {}", port);

        // Forward bridge stderr to our stderr so SDK diagnostics are visible.
        if let Some(stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                let reader = std::io::BufReader::new(stderr);
                for line in reader.lines() {
                    if let Ok(text) = line
                        && !text.is_empty()
                    {
                        eprintln!("[pi-bridge] {}", text);
                    }
                }
            });
        }

        let child = Arc::new(Mutex::new(child));

        // Monitor the child process and log when it exits.
        let child_for_monitor = Arc::clone(&child);
        std::thread::spawn(move || {
            let status = child_for_monitor.lock().unwrap().wait();
            log!("pi bridge process exited: {:?}", status);
        });

        let url = format!("ws://127.0.0.1:{}/", port);

        let runtime = tokio::runtime::Runtime::new()
            .map_err(|e| PiRpcError::Runtime(format!("tokio runtime: {}", e)))?;

        let (ws_stream, _) = runtime
            .block_on(tokio_tungstenite::connect_async(&url))
            .map_err(|e| PiRpcError::WebSocket(format!("connect: {}", e)))?;

        let (write_half, read_half) = ws_stream.split();
        let (write_tx, mut write_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

        let sessions: Arc<
            Mutex<HashMap<String, futures::channel::mpsc::UnboundedSender<BridgeEvent>>>,
        > = Arc::new(Mutex::new(HashMap::new()));
        let sessions_for_reader = Arc::clone(&sessions);

        // Writer task: forward outgoing JSON to the WebSocket.
        runtime.spawn(async move {
            let mut write_half = write_half;
            while let Some(text) = write_rx.recv().await {
                if write_half.send(Message::text(text)).await.is_err() {
                    break;
                }
            }
            log!("bridge writer task exiting");
            let _ = write_half.close().await;
        });

        // Reader task: parse incoming messages and route to the right session.
        runtime.spawn(async move {
            let mut read_half = read_half;
            while let Some(result) = read_half.next().await {
                match result {
                    Ok(Message::Text(text)) => {
                        if let Some((session_id, event)) = parse_bridge_message(&text) {
                            let sessions_guard = sessions_for_reader.lock().unwrap();
                            if let Some(sender) = sessions_guard.get(&session_id) {
                                let _ = sender.unbounded_send(event);
                            } else {
                                log!("no session receiver for {}", session_id);
                            }
                        }
                    }
                    Ok(Message::Close(_)) => {
                        log!("bridge closed websocket");
                        break;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        log!("bridge websocket error: {}", e);
                        break;
                    }
                }
            }
            log!("bridge reader task exiting; notifying sessions");
            let mut sessions_guard = sessions_for_reader.lock().unwrap();
            for (_id, sender) in sessions_guard.drain() {
                let _: Result<(), _> = sender.unbounded_send(BridgeEvent::Disconnected);
            }
        });

        Ok(Arc::new(Self {
            write_tx,
            sessions,
            child,
            runtime,
        }))
    }

    pub fn create_session(
        &self,
        session_id: String,
        session_path: Option<PathBuf>,
        cwd: Option<PathBuf>,
        model: Option<String>,
        thinking_level: Option<String>,
    ) -> Result<futures::channel::mpsc::UnboundedReceiver<BridgeEvent>, PiRpcError> {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.insert(session_id.clone(), tx);
        }

        let mut cmd = serde_json::json!({
            "type": "create_session",
            "sessionId": session_id,
        });
        if let Some(path) = session_path {
            cmd["sessionPath"] = serde_json::json!(path.to_string_lossy().to_string());
        }
        if let Some(dir) = cwd {
            cmd["cwd"] = serde_json::json!(dir.to_string_lossy().to_string());
        }
        if let Some(m) = model {
            cmd["model"] = serde_json::json!(m);
        }
        if let Some(level) = thinking_level {
            cmd["thinkingLevel"] = serde_json::json!(level);
        }

        self.send_json(&cmd)?;
        Ok(rx)
    }

    pub fn get_models(&self) -> Result<Vec<BridgeModel>, PiRpcError> {
        let session_id = format!("__models__{}", Uuid::new_v4());
        let request_id = Uuid::new_v4().to_string();
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.insert(session_id.clone(), tx);
        }
        let _guard = SessionGuard {
            sessions: Arc::clone(&self.sessions),
            session_id: session_id.clone(),
        };

        let req = serde_json::json!({
            "type": "get_models",
            "sessionId": session_id,
            "id": request_id,
        });
        if let Err(e) = self.send_json(&req) {
            return Err(e);
        }

        let result = self.runtime.block_on(async {
            while let Some(event) = rx.next().await {
                if let BridgeEvent::Response {
                    command,
                    success,
                    data,
                    error,
                    ..
                } = event
                {
                    if command == "get_models" {
                        if success {
                            return Ok(data);
                        }
                        return Err(PiRpcError::Models(
                            error.unwrap_or_else(|| "get_models failed".into()),
                        ));
                    }
                }
            }
            Err(PiRpcError::WebSocket(
                "bridge closed before get_models response".into(),
            ))
        });

        let data = result?;
        let models_val = data
            .and_then(|d| d.get("models").cloned())
            .ok_or_else(|| PiRpcError::Models("get_models response missing models".into()))?;
        let arr = models_val
            .as_array()
            .ok_or_else(|| PiRpcError::Models("get_models models is not an array".into()))?;

        let mut models = Vec::new();
        for m in arr {
            let provider = m
                .get("provider")
                .and_then(|v| v.as_str())
                .ok_or_else(|| PiRpcError::Models("model missing provider".into()))?;
            let id = m
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| PiRpcError::Models("model missing id".into()))?;
            let name = m
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| PiRpcError::Models("model missing name".into()))?;
            let thinking_level_map =
                m.get("thinkingLevelMap")
                    .and_then(|v| v.as_object())
                    .map(|obj| {
                        obj.iter()
                            .map(|(k, v)| {
                                let value = if v.is_null() {
                                    None
                                } else {
                                    v.as_str().map(|s| s.to_string())
                                };
                                (k.clone(), value)
                            })
                            .collect::<HashMap<String, Option<String>>>()
                    });
            models.push(BridgeModel {
                provider: provider.to_string(),
                id: id.to_string(),
                name: name.to_string(),
                thinking_level_map,
            });
        }
        Ok(models)
    }

    pub fn get_skills(&self) -> Result<Vec<BridgeSkill>, PiRpcError> {
        self.query_session_resource("get_skills", "skills", |data| {
            let arr = data
                .as_array()
                .ok_or_else(|| PiRpcError::Skills("skills is not an array".into()))?;
            let mut items = Vec::new();
            for val in arr {
                let obj = val.as_object();
                let name = obj
                    .and_then(|o| o.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unnamed")
                    .to_string();
                let description = obj
                    .and_then(|o| o.get("description"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let raw = obj
                    .and_then(|o| o.get("raw"))
                    .cloned()
                    .unwrap_or_else(|| val.clone());
                items.push(BridgeSkill {
                    name,
                    description,
                    raw,
                });
            }
            Ok(items)
        })
    }

    pub fn get_extensions(&self) -> Result<Vec<BridgeExtension>, PiRpcError> {
        self.query_session_resource("get_extensions", "extensions", |data| {
            let arr = data
                .as_array()
                .ok_or_else(|| PiRpcError::Extensions("extensions is not an array".into()))?;
            let mut items = Vec::new();
            for val in arr {
                let obj = val.as_object();
                let name = obj
                    .and_then(|o| o.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unnamed")
                    .to_string();
                let description = obj
                    .and_then(|o| o.get("description"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let raw = obj
                    .and_then(|o| o.get("raw"))
                    .cloned()
                    .unwrap_or_else(|| val.clone());
                items.push(BridgeExtension {
                    name,
                    description,
                    raw,
                });
            }
            Ok(items)
        })
    }

    pub fn get_prompts(&self) -> Result<Vec<BridgePrompt>, PiRpcError> {
        self.query_session_resource("get_prompts", "prompts", |data| {
            let arr = data
                .as_array()
                .ok_or_else(|| PiRpcError::Prompts("prompts is not an array".into()))?;
            let mut items = Vec::new();
            for val in arr {
                let obj = val.as_object();
                let name = obj
                    .and_then(|o| o.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unnamed")
                    .to_string();
                let description = obj
                    .and_then(|o| o.get("description"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let raw = obj
                    .and_then(|o| o.get("raw"))
                    .cloned()
                    .unwrap_or_else(|| val.clone());
                items.push(BridgePrompt {
                    name,
                    description,
                    raw,
                });
            }
            Ok(items)
        })
    }

    /// Query the bridge for the effective slash-command list.
    ///
    /// Returns the full response `data` object (which contains a `commands`
    /// array) so callers can parse it consistently with the per-session
    /// `get_commands` response.
    pub fn get_commands(&self) -> Result<serde_json::Value, PiRpcError> {
        let session_id = format!("__commands__{}", Uuid::new_v4());
        let request_id = Uuid::new_v4().to_string();
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.insert(session_id.clone(), tx);
        }
        let _guard = SessionGuard {
            sessions: Arc::clone(&self.sessions),
            session_id: session_id.clone(),
        };

        let req = serde_json::json!({
            "type": "get_commands",
            "sessionId": session_id,
            "id": request_id,
        });
        if let Err(e) = self.send_json(&req) {
            return Err(e);
        }

        let result = self.runtime.block_on(async {
            while let Some(event) = rx.next().await {
                if let BridgeEvent::Response {
                    command,
                    success,
                    data,
                    error,
                    ..
                } = event
                {
                    if command == "get_commands" {
                        if success {
                            return Ok(
                                data.unwrap_or(serde_json::Value::Object(serde_json::Map::new()))
                            );
                        }
                        return Err(PiRpcError::Bridge(
                            error.unwrap_or_else(|| "get_commands failed".into()),
                        ));
                    }
                }
            }
            Err(PiRpcError::WebSocket(
                "bridge closed before get_commands response".into(),
            ))
        });

        result
    }

    pub fn get_providers(&self) -> Result<Vec<BridgeProvider>, PiRpcError> {
        let session_id = format!("__providers__{}", Uuid::new_v4());
        let request_id = Uuid::new_v4().to_string();
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.insert(session_id.clone(), tx);
        }
        let _guard = SessionGuard {
            sessions: Arc::clone(&self.sessions),
            session_id: session_id.clone(),
        };

        let req = serde_json::json!({
            "type": "get_providers",
            "sessionId": session_id,
            "id": request_id,
        });
        if let Err(e) = self.send_json(&req) {
            return Err(e);
        }

        let result = self.runtime.block_on(async {
            while let Some(event) = rx.next().await {
                if let BridgeEvent::Response {
                    command,
                    success,
                    data,
                    error,
                    ..
                } = event
                {
                    if command == "get_providers" {
                        if success {
                            return Ok(data);
                        }
                        return Err(PiRpcError::Bridge(
                            error.unwrap_or_else(|| "get_providers failed".into()),
                        ));
                    }
                }
            }
            Err(PiRpcError::WebSocket(
                "bridge closed before get_providers response".into(),
            ))
        });

        let data = result?;
        let providers_val = data
            .and_then(|d| d.get("providers").cloned())
            .ok_or_else(|| PiRpcError::Bridge("get_providers response missing providers".into()))?;
        let arr = providers_val
            .as_array()
            .ok_or_else(|| PiRpcError::Bridge("get_providers providers is not an array".into()))?;

        let mut providers = Vec::new();
        for p in arr {
            let id = p
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| PiRpcError::Bridge("provider missing id".into()))?;
            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or(id);
            let configured = p
                .get("configured")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            providers.push(BridgeProvider {
                id: id.to_string(),
                name: name.to_string(),
                configured,
            });
        }
        Ok(providers)
    }

    pub fn set_auth(&self, provider: &str, key: &str) -> Result<(), PiRpcError> {
        let session_id = format!("__auth__{}", Uuid::new_v4());
        let request_id = Uuid::new_v4().to_string();
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.insert(session_id.clone(), tx);
        }
        let _guard = SessionGuard {
            sessions: Arc::clone(&self.sessions),
            session_id: session_id.clone(),
        };

        let req = serde_json::json!({
            "type": "set_auth",
            "sessionId": session_id,
            "id": request_id,
            "provider": provider,
            "key": key,
        });
        if let Err(e) = self.send_json(&req) {
            return Err(e);
        }

        self.runtime.block_on(async {
            while let Some(event) = rx.next().await {
                if let BridgeEvent::Response {
                    command,
                    success,
                    error,
                    ..
                } = event
                {
                    if command == "set_auth" {
                        if success {
                            return Ok(());
                        }
                        return Err(PiRpcError::Bridge(
                            error.unwrap_or_else(|| "set_auth failed".into()),
                        ));
                    }
                }
            }
            Err(PiRpcError::WebSocket(
                "bridge closed before set_auth response".into(),
            ))
        })
    }

    pub fn get_settings(&self) -> Result<BridgeSettings, PiRpcError> {
        let session_id = format!("__settings__{}", Uuid::new_v4());
        let request_id = Uuid::new_v4().to_string();
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.insert(session_id.clone(), tx);
        }
        let _guard = SessionGuard {
            sessions: Arc::clone(&self.sessions),
            session_id: session_id.clone(),
        };

        let req = serde_json::json!({
            "type": "get_settings",
            "sessionId": session_id,
            "id": request_id,
        });
        if let Err(e) = self.send_json(&req) {
            return Err(e);
        }

        let result = self.runtime.block_on(async {
            while let Some(event) = rx.next().await {
                if let BridgeEvent::Response {
                    command,
                    success,
                    data,
                    error,
                    ..
                } = event
                {
                    if command == "get_settings" {
                        if success {
                            return Ok(data);
                        }
                        return Err(PiRpcError::Bridge(
                            error.unwrap_or_else(|| "get_settings failed".into()),
                        ));
                    }
                }
            }
            Err(PiRpcError::WebSocket(
                "bridge closed before get_settings response".into(),
            ))
        });

        let data = result?;
        let data =
            data.ok_or_else(|| PiRpcError::Bridge("get_settings response missing data".into()))?;

        let compaction_enabled = data
            .get("compactionEnabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let default_thinking_level = data
            .get("defaultThinkingLevel")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let default_model = data
            .get("defaultModel")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let default_provider = data
            .get("defaultProvider")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(BridgeSettings {
            compaction_enabled,
            default_thinking_level,
            default_model,
            default_provider,
        })
    }

    pub fn set_compaction_enabled(&self, enabled: bool) -> Result<(), PiRpcError> {
        self.send_settings_command(
            "set_compaction_enabled",
            serde_json::json!({ "enabled": enabled }),
        )
    }

    pub fn set_default_thinking_level(&self, level: &str) -> Result<(), PiRpcError> {
        self.send_settings_command(
            "set_default_thinking_level",
            serde_json::json!({ "level": level }),
        )
    }

    pub fn set_default_model(&self, model_id: &str) -> Result<(), PiRpcError> {
        self.send_settings_command(
            "set_default_model",
            serde_json::json!({ "modelId": model_id }),
        )
    }

    pub fn set_default_provider(&self, provider: &str) -> Result<(), PiRpcError> {
        self.send_settings_command(
            "set_default_provider",
            serde_json::json!({ "provider": provider }),
        )
    }

    fn send_settings_command(
        &self,
        command: &str,
        payload: serde_json::Value,
    ) -> Result<(), PiRpcError> {
        let session_id = format!("__settings__{}", Uuid::new_v4());
        let request_id = Uuid::new_v4().to_string();
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.insert(session_id.clone(), tx);
        }
        let _guard = SessionGuard {
            sessions: Arc::clone(&self.sessions),
            session_id: session_id.clone(),
        };

        let mut req = serde_json::json!({
            "type": command,
            "sessionId": session_id,
            "id": request_id,
        });
        if let Some(obj) = payload.as_object() {
            for (key, value) in obj {
                req[key] = value.clone();
            }
        }

        if let Err(e) = self.send_json(&req) {
            return Err(e);
        }

        self.runtime.block_on(async {
            while let Some(event) = rx.next().await {
                if let BridgeEvent::Response {
                    command: resp_command,
                    success,
                    error,
                    ..
                } = event
                {
                    if resp_command == command {
                        if success {
                            return Ok(());
                        }
                        return Err(PiRpcError::Bridge(
                            error.unwrap_or_else(|| format!("{} failed", command)),
                        ));
                    }
                }
            }
            Err(PiRpcError::WebSocket(format!(
                "bridge closed before {} response",
                command
            )))
        })
    }

    fn query_session_resource<T>(
        &self,
        command: &str,
        data_key: &str,
        parse: impl FnOnce(serde_json::Value) -> Result<T, PiRpcError>,
    ) -> Result<T, PiRpcError> {
        let session_id = format!("__resources__{}", Uuid::new_v4());
        let request_id = Uuid::new_v4().to_string();

        // Create a session so the bridge has a runtime with services.resourceLoader.
        let mut rx = self.create_session(session_id.clone(), None, None, None, None)?;
        let _guard = SessionGuard {
            sessions: Arc::clone(&self.sessions),
            session_id: session_id.clone(),
        };

        let req = serde_json::json!({
            "type": command,
            "id": request_id,
        });
        if let Err(e) = self.send(session_id.clone(), &req) {
            return Err(e);
        }

        let result = self.runtime.block_on(async move {
            while let Some(event) = rx.next().await {
                if let BridgeEvent::Response {
                    command: resp_command,
                    success,
                    data,
                    error,
                    ..
                } = event
                {
                    if resp_command == command {
                        if success {
                            return Ok(data);
                        }
                        return Err(PiRpcError::Bridge(format!(
                            "{} failed: {}",
                            command,
                            error.unwrap_or_else(|| "unknown error".into())
                        )));
                    }
                }
            }
            Err(PiRpcError::WebSocket(format!(
                "bridge closed before {} response",
                command
            )))
        });

        let data = result?;
        let resource_val = data.and_then(|d| d.get(data_key).cloned()).ok_or_else(|| {
            PiRpcError::Bridge(format!("{} response missing {}", command, data_key))
        })?;
        parse(resource_val)
    }

    pub fn send(&self, session_id: String, json: &serde_json::Value) -> Result<(), PiRpcError> {
        let mut msg = json.clone();
        msg["sessionId"] = serde_json::json!(session_id);
        self.send_json(&msg)
    }

    fn send_json(&self, json: &serde_json::Value) -> Result<(), PiRpcError> {
        let text = serde_json::to_string(json).map_err(PiRpcError::Serde)?;
        log!(
            "send: type={} sessionId={}",
            json.get("type").and_then(|t| t.as_str()).unwrap_or("?"),
            json.get("sessionId")
                .and_then(|s| s.as_str())
                .unwrap_or("?")
        );
        self.write_tx
            .send(text)
            .map_err(|_| PiRpcError::WebSocket("writer channel closed".to_string()))
    }
}

// ---------------------------------------------------------------------------
// PiRpc: per-session handle that sends commands through PiBridge
// ---------------------------------------------------------------------------

pub struct PiRpc {
    session_id: String,
    bridge: Arc<PiBridge>,
}

impl PiRpc {
    pub fn new(session_id: String, bridge: Arc<PiBridge>) -> Self {
        Self { session_id, bridge }
    }

    fn send(&mut self, json: &serde_json::Value) -> Result<(), PiRpcError> {
        self.bridge.send(self.session_id.clone(), json)
    }

    // ------------------------------------------------------------------
    // Prompting
    // ------------------------------------------------------------------

    pub fn send_prompt(&mut self, message: &str) -> Result<(), PiRpcError> {
        self.send_prompt_ext(message, None, None, None)
    }

    pub fn send_prompt_ext(
        &mut self,
        message: &str,
        media: Option<&[ImageContent]>,
        streaming_behavior: Option<&str>,
        request_id: Option<&str>,
    ) -> Result<(), PiRpcError> {
        let mut cmd = serde_json::json!({
            "type": "prompt",
            "message": message,
        });
        add_request_id(&mut cmd, request_id);
        add_media(&mut cmd, media);
        if let Some(behavior) = streaming_behavior {
            cmd["streamingBehavior"] = serde_json::json!(behavior);
        }
        self.send(&cmd)
    }

    pub fn send_steer(
        &mut self,
        message: &str,
        media: Option<&[ImageContent]>,
        request_id: Option<&str>,
    ) -> Result<(), PiRpcError> {
        let mut cmd = serde_json::json!({
            "type": "steer",
            "message": message,
        });
        add_request_id(&mut cmd, request_id);
        add_media(&mut cmd, media);
        self.send(&cmd)
    }

    pub fn send_follow_up(
        &mut self,
        message: &str,
        media: Option<&[ImageContent]>,
        request_id: Option<&str>,
    ) -> Result<(), PiRpcError> {
        let mut cmd = serde_json::json!({
            "type": "follow_up",
            "message": message,
        });
        add_request_id(&mut cmd, request_id);
        add_media(&mut cmd, media);
        self.send(&cmd)
    }

    pub fn send_abort(&mut self, request_id: Option<&str>) -> Result<(), PiRpcError> {
        let mut cmd = serde_json::json!({ "type": "abort" });
        add_request_id(&mut cmd, request_id);
        self.send(&cmd)
    }

    pub fn send_new_session(
        &mut self,
        parent_session: Option<&str>,
        request_id: Option<&str>,
    ) -> Result<(), PiRpcError> {
        let mut cmd = serde_json::json!({ "type": "new_session" });
        if let Some(parent) = parent_session {
            cmd["parentSession"] = serde_json::json!(parent);
        }
        add_request_id(&mut cmd, request_id);
        self.send(&cmd)
    }

    // ------------------------------------------------------------------
    // State
    // ------------------------------------------------------------------

    pub fn send_get_state(&mut self, request_id: Option<&str>) -> Result<(), PiRpcError> {
        let mut cmd = serde_json::json!({ "type": "get_state" });
        add_request_id(&mut cmd, request_id);
        self.send(&cmd)
    }

    pub fn send_get_messages(&mut self, request_id: Option<&str>) -> Result<(), PiRpcError> {
        let mut cmd = serde_json::json!({ "type": "get_messages" });
        add_request_id(&mut cmd, request_id);
        self.send(&cmd)
    }

    pub fn send_get_commands(&mut self, request_id: Option<&str>) -> Result<(), PiRpcError> {
        let mut cmd = serde_json::json!({ "type": "get_commands" });
        add_request_id(&mut cmd, request_id);
        self.send(&cmd)
    }

    pub fn send_get_session_stats(&mut self, request_id: Option<&str>) -> Result<(), PiRpcError> {
        let mut cmd = serde_json::json!({ "type": "get_session_stats" });
        add_request_id(&mut cmd, request_id);
        self.send(&cmd)
    }

    pub fn send_navigate_tree(
        &mut self,
        entry_id: &str,
        request_id: Option<&str>,
    ) -> Result<(), PiRpcError> {
        let mut cmd = serde_json::json!({
            "type": "navigate_tree",
            "entryId": entry_id,
        });
        add_request_id(&mut cmd, request_id);
        self.send(&cmd)
    }

    pub fn send_fork(
        &mut self,
        entry_id: &str,
        request_id: Option<&str>,
    ) -> Result<(), PiRpcError> {
        let mut cmd = serde_json::json!({
            "type": "fork",
            "entryId": entry_id,
        });
        add_request_id(&mut cmd, request_id);
        self.send(&cmd)
    }

    pub fn send_clone(&mut self, request_id: Option<&str>) -> Result<(), PiRpcError> {
        let mut cmd = serde_json::json!({ "type": "clone" });
        add_request_id(&mut cmd, request_id);
        self.send(&cmd)
    }

    pub fn send_get_fork_messages(&mut self, request_id: Option<&str>) -> Result<(), PiRpcError> {
        let mut cmd = serde_json::json!({ "type": "get_fork_messages" });
        add_request_id(&mut cmd, request_id);
        self.send(&cmd)
    }

    // ------------------------------------------------------------------
    // Model
    // ------------------------------------------------------------------

    pub fn send_set_model(
        &mut self,
        provider: &str,
        model_id: &str,
        request_id: Option<&str>,
    ) -> Result<(), PiRpcError> {
        let mut cmd = serde_json::json!({
            "type": "set_model",
            "provider": provider,
            "modelId": model_id,
        });
        add_request_id(&mut cmd, request_id);
        self.send(&cmd)
    }

    pub fn send_get_model(
        &mut self,
        provider: Option<&str>,
        model_id: Option<&str>,
        request_id: Option<&str>,
    ) -> Result<(), PiRpcError> {
        let mut cmd = serde_json::json!({ "type": "get_model" });
        if let Some(provider) = provider {
            cmd["provider"] = serde_json::json!(provider);
        }
        if let Some(model_id) = model_id {
            cmd["modelId"] = serde_json::json!(model_id);
        }
        add_request_id(&mut cmd, request_id);
        self.send(&cmd)
    }

    // ------------------------------------------------------------------
    // Thinking
    // ------------------------------------------------------------------

    pub fn send_set_thinking_level(
        &mut self,
        level: &str,
        request_id: Option<&str>,
    ) -> Result<(), PiRpcError> {
        let mut cmd = serde_json::json!({
            "type": "set_thinking_level",
            "level": level,
        });
        add_request_id(&mut cmd, request_id);
        self.send(&cmd)
    }

    // ------------------------------------------------------------------
    // Bash
    // ------------------------------------------------------------------

    pub fn send_bash(&mut self, command: &str, request_id: Option<&str>) -> Result<(), PiRpcError> {
        let mut cmd = serde_json::json!({
            "type": "bash",
            "command": command,
        });
        add_request_id(&mut cmd, request_id);
        self.send(&cmd)
    }

    // ------------------------------------------------------------------
    // Compaction
    // ------------------------------------------------------------------

    pub fn send_compact(
        &mut self,
        custom_instructions: Option<&str>,
        request_id: Option<&str>,
    ) -> Result<(), PiRpcError> {
        let mut cmd = serde_json::json!({ "type": "compact" });
        if let Some(instructions) = custom_instructions {
            cmd["customInstructions"] = serde_json::json!(instructions);
        }
        add_request_id(&mut cmd, request_id);
        self.send(&cmd)
    }

    // ------------------------------------------------------------------
    // Export
    // ------------------------------------------------------------------

    pub fn send_export_html(
        &mut self,
        output_path: Option<&str>,
        request_id: Option<&str>,
    ) -> Result<(), PiRpcError> {
        let mut cmd = serde_json::json!({ "type": "export_html" });
        if let Some(path) = output_path {
            cmd["outputPath"] = serde_json::json!(path);
        }
        add_request_id(&mut cmd, request_id);
        self.send(&cmd)
    }

    // ------------------------------------------------------------------
    // Extension UI
    // ------------------------------------------------------------------

    pub fn send_extension_ui_response(
        &mut self,
        id: &str,
        response: &serde_json::Value,
    ) -> Result<(), PiRpcError> {
        let mut cmd = response.clone();
        if let Some(obj) = cmd.as_object_mut() {
            obj.insert(
                "type".to_string(),
                serde_json::json!("extension_ui_response"),
            );
            obj.insert("id".to_string(), serde_json::json!(id));
        }
        self.send(&cmd)
    }
}

// ---------------------------------------------------------------------------
// Bridge process helpers
// ---------------------------------------------------------------------------

fn find_runtime(bridge_dir: &PathBuf) -> Result<(String, Vec<String>), PiRpcError> {
    // Release builds compile the bridge into a single executable with
    // `bun build --compile`. No separate runtime or node_modules is needed.
    let compiled_names: [&str; 2] = if cfg!(windows) {
        ["pi-bridge.exe", "pi-bridge"]
    } else {
        ["pi-bridge", "pi-bridge.exe"]
    };
    for name in compiled_names.iter() {
        let exe = bridge_dir.join(name);
        if exe.exists() {
            return Ok((exe.to_string_lossy().to_string(), vec![]));
        }
    }

    // Development fallback: a bundled bun binary that runs src/index.ts.
    let src_ts = bridge_dir.join("src").join("index.ts");
    if src_ts.exists() {
        let bun_candidates: Vec<PathBuf> = if cfg!(windows) {
            vec![
                bridge_dir.join("bun.exe"),
                bridge_dir
                    .parent()
                    .map(|p| p.join("bun.exe"))
                    .unwrap_or_default(),
            ]
        } else {
            vec![
                bridge_dir.join("bun"),
                bridge_dir
                    .parent()
                    .map(|p| p.join("bun"))
                    .unwrap_or_default(),
            ]
        };
        for bun in bun_candidates {
            if bun.exists() {
                return Ok((
                    bun.to_string_lossy().to_string(),
                    vec!["run".to_string(), "src/index.ts".to_string()],
                ));
            }
        }

        // Fallback to a system-installed bun.
        if Command::new("bun").arg("--version").output().is_ok() {
            return Ok((
                "bun".to_string(),
                vec!["run".to_string(), "src/index.ts".to_string()],
            ));
        }
    }

    Err(PiRpcError::Spawn(
        "no bun runtime found (tried compiled pi-bridge, bundled bun, system bun).".to_string(),
    ))
}

fn read_bridge_port(stdout: std::process::ChildStdout) -> Result<u16, PiRpcError> {
    let reader = std::io::BufReader::new(stdout);
    for line in reader.lines() {
        let text = line.map_err(PiRpcError::Io)?;
        if text.trim().is_empty() {
            continue;
        }
        let preview_end = text.len().min(200);
        let preview_end = text.floor_char_boundary(preview_end);
        log!("bridge stdout: {}", &text[..preview_end]);
        if let Some(prefix) = text.strip_prefix("BRIDGE_PORT ") {
            return prefix
                .trim()
                .parse::<u16>()
                .map_err(|e| PiRpcError::Spawn(format!("invalid port: {}", e)));
        }
    }
    Err(PiRpcError::Spawn(
        "bridge exited before reporting port".to_string(),
    ))
}

// ---------------------------------------------------------------------------
// Message helpers
// ---------------------------------------------------------------------------

fn add_request_id(cmd: &mut serde_json::Value, request_id: Option<&str>) {
    if let Some(id) = request_id {
        cmd["id"] = serde_json::json!(id);
    }
}

fn add_media(cmd: &mut serde_json::Value, media: Option<&[ImageContent]>) {
    if let Some(items) = media
        && !items.is_empty()
    {
        let vals: Vec<serde_json::Value> = items
            .iter()
            .map(|item| {
                serde_json::json!({
                    "type": "image",
                    "data": item.data,
                    "mimeType": item.mime_type,
                })
            })
            .collect();
        cmd["images"] = serde_json::json!(vals);
    }
}

fn parse_bridge_message(text: &str) -> Option<(String, BridgeEvent)> {
    let mut val: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            log!(
                "failed to parse JSON: {} (line: {})",
                e,
                truncate_str(text, 5000)
            );
            return None;
        }
    };

    let session_id = val
        .get("sessionId")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string())?;

    // Remove sessionId so the rest of the payload matches the original RPC event shape.
    if let Some(obj) = val.as_object_mut() {
        obj.remove("sessionId");
    }

    let event = parse_pi_line_value(&val)?;
    Some((session_id, event))
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

fn parse_pi_line_value(val: &serde_json::Value) -> Option<BridgeEvent> {
    let event_type = val.get("type")?.as_str()?;
    log!(
        "raw event type={}, data={}",
        event_type,
        truncate_str(&serde_json::to_string(val).unwrap_or_default(), 5000)
    );

    match event_type {
        // -- lifecycle ---------------------------------------------------
        "agent_start" => Some(BridgeEvent::AgentStart),
        "agent_end" => Some(BridgeEvent::AgentEnd {
            messages: val.get("messages").and_then(parse_loaded_messages),
        }),

        // -- message -----------------------------------------------------
        "message_start" => {
            let message = val.get("message").cloned();
            Some(BridgeEvent::MessageStart { message })
        }
        "message_end" => Some(BridgeEvent::MessageEnd),

        // -- streaming deltas --------------------------------------------
        "message_update" => {
            let delta = val.get("assistantMessageEvent")?;
            let delta_type = delta.get("type")?.as_str()?;
            match delta_type {
                "text_start" => Some(BridgeEvent::TextStart),
                "text_delta" => Some(BridgeEvent::TextDelta {
                    content: delta.get("delta")?.as_str()?.to_string(),
                }),
                "text_end" => Some(BridgeEvent::TextEnd {
                    content: delta
                        .get("content")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string(),
                }),
                "thinking_start" => Some(BridgeEvent::ThinkingStart),
                "thinking_delta" => Some(BridgeEvent::ThinkingDelta {
                    content: delta.get("delta")?.as_str()?.to_string(),
                }),
                "thinking_end" => Some(BridgeEvent::ThinkingEnd {
                    content: delta
                        .get("content")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string(),
                }),
                "toolcall_start" => Some(BridgeEvent::ToolCallStart {
                    name: delta
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("unknown")
                        .to_string(),
                    call_id: delta
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("unknown")
                        .to_string(),
                }),
                "toolcall_delta" => Some(BridgeEvent::ToolCallDelta {
                    call_id: delta
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("unknown")
                        .to_string(),
                    delta: delta
                        .get("delta")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string(),
                }),
                "toolcall_end" => {
                    let tool_call = delta.get("toolCall")?;
                    Some(BridgeEvent::ToolCallEnd {
                        call_id: tool_call
                            .get("id")
                            .and_then(|i| i.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                        name: tool_call
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                        args: tool_call
                            .get("arguments")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    })
                }
                "start" | "done" | "error" => None,
                _ => {
                    log!("unknown message_update delta type: {}", delta_type);
                    None
                }
            }
        }

        // -- tool execution ----------------------------------------------
        "tool_execution_start" => Some(BridgeEvent::ToolStart {
            name: val.get("toolName")?.as_str()?.to_string(),
            args: val.get("args").cloned(),
            call_id: val
                .get("toolCallId")
                .and_then(|i| i.as_str())
                .unwrap_or("unknown")
                .to_string(),
        }),

        "tool_execution_update" => {
            let mut output = String::new();
            if let Some(pr) = val.get("partialResult")
                && let Some(content) = pr.get("content")
            {
                extract_text_parts(content, &mut output);
            }
            Some(BridgeEvent::ToolUpdate {
                call_id: val
                    .get("toolCallId")
                    .and_then(|i| i.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                tool_name: val
                    .get("toolName")
                    .and_then(|n| n.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                partial_output: output,
            })
        }

        "tool_execution_end" => {
            let tool_name = val
                .get("toolName")
                .and_then(|n| n.as_str())
                .unwrap_or("unknown")
                .to_string();
            let call_id = val
                .get("toolCallId")
                .and_then(|i| i.as_str())
                .unwrap_or("unknown")
                .to_string();
            let mut output = String::new();
            let details = val.get("result").and_then(|r| r.get("details")).cloned();
            if let Some(result) = val.get("result")
                && let Some(content) = result.get("content")
            {
                extract_text_parts(content, &mut output);
            }
            let is_error = val
                .get("isError")
                .and_then(|e| e.as_bool())
                .unwrap_or(false);
            Some(BridgeEvent::ToolEnd {
                call_id,
                tool_name,
                output: truncate_str(&output, 5000),
                is_error,
                details,
            })
        }

        // -- turn --------------------------------------------------------
        "turn_start" => Some(BridgeEvent::TurnStart),
        "turn_end" => Some(BridgeEvent::TurnEnd),

        // -- responses ---------------------------------------------------
        "response" => {
            let command = val
                .get("command")
                .and_then(|c| c.as_str())
                .unwrap_or("unknown");
            let success = val
                .get("success")
                .and_then(|s| s.as_bool())
                .unwrap_or(false);
            let error = val
                .get("error")
                .and_then(|e| e.as_str())
                .map(|e| e.to_string());
            let data = val.get("data").cloned();
            let request_id = val
                .get("id")
                .and_then(|i| i.as_str())
                .map(|i| i.to_string());

            if !success {
                log!(
                    "pi response error for '{}': {}",
                    command,
                    error.as_deref().unwrap_or("unknown error")
                );
            }

            if command == "get_messages" && success {
                if let Some(ref data_val) = data
                    && data_val.get("messages").is_some()
                {
                    return Some(BridgeEvent::Response {
                        command: command.to_string(),
                        success,
                        data: data.clone(),
                        error: error.clone(),
                        request_id: request_id.clone(),
                    });
                }
                log!("get_messages response missing data.messages");
            }

            Some(BridgeEvent::Response {
                command: command.to_string(),
                success,
                data,
                error,
                request_id,
            })
        }

        // -- extension ---------------------------------------------------
        "extension_ui_request" => Some(BridgeEvent::ExtensionUiRequest {
            id: val
                .get("id")
                .and_then(|i| i.as_str())
                .unwrap_or("unknown")
                .to_string(),
            method: val
                .get("method")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown")
                .to_string(),
            payload: val.clone(),
        }),

        "extension_error" => Some(BridgeEvent::ExtensionError {
            extension_path: val
                .get("extensionPath")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown")
                .to_string(),
            event: val
                .get("event")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown")
                .to_string(),
            error: val
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown")
                .to_string(),
        }),

        _ => {
            log!("unknown pi event type: {}", event_type);
            None
        }
    }
}

// ---------------------------------------------------------------------------
// History parsing
// ---------------------------------------------------------------------------

pub fn parse_loaded_messages(messages_val: &serde_json::Value) -> Option<Vec<LoadedMessage>> {
    let arr = messages_val.as_array()?;
    let mut loaded = Vec::new();

    for msg in arr {
        let id = msg
            .get("id")
            .and_then(|i| i.as_str())
            .map(|s| s.to_string());
        let role = msg.get("role")?.as_str()?.to_string();

        match role.as_str() {
            "user" => {
                let content = msg.get("content");
                let mut text = String::new();
                let mut parts: Vec<LoadedPart> = vec![];
                if let Some(arr) = content.and_then(|c| c.as_array()) {
                    for block in arr {
                        let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        if block_type == "image" {
                            if let (Some(data), Some(mime_type)) = (
                                block.get("data").and_then(|d| d.as_str()),
                                block.get("mimeType").and_then(|m| m.as_str()),
                            ) {
                                parts.push(LoadedPart::Image {
                                    data: data.to_string(),
                                    mime_type: mime_type.to_string(),
                                });
                            }
                        } else if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(t);
                        }
                    }
                } else if let Some(s) = content.and_then(|c| c.as_str()) {
                    text = s.to_string();
                }
                if !text.is_empty() {
                    parts.insert(0, LoadedPart::Text { text });
                }
                loaded.push(LoadedMessage {
                    id,
                    role: "user".to_string(),
                    parts,
                    error_message: None,
                    is_error: false,
                });
            }
            "assistant" => {
                let content = msg.get("content");
                let mut parts: Vec<LoadedPart> = vec![];
                if let Some(content_arr) = content.and_then(|c| c.as_array()) {
                    for block in content_arr {
                        let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        match block_type {
                            "text" => {
                                if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                                    parts.push(LoadedPart::Text {
                                        text: t.to_string(),
                                    });
                                }
                            }
                            "thinking" => {
                                if let Some(t) = block.get("thinking").and_then(|t| t.as_str()) {
                                    parts.push(LoadedPart::Thinking {
                                        text: t.to_string(),
                                    });
                                }
                            }
                            "toolCall" => {
                                let name = block
                                    .get("name")
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("unknown");
                                let args_str = block
                                    .get("arguments")
                                    .and_then(|a| serde_json::to_string(a).ok())
                                    .unwrap_or_default();
                                parts.push(LoadedPart::ToolCall {
                                    name: name.to_string(),
                                    args: truncate_str(&args_str, 5000),
                                });
                            }
                            _ => {}
                        }
                    }
                } else if let Some(content_str) = content.and_then(|c| c.as_str()) {
                    parts.push(LoadedPart::Text {
                        text: content_str.to_string(),
                    });
                }

                let error_message = msg
                    .get("errorMessage")
                    .and_then(|e| e.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
                let stop_reason = msg.get("stopReason").and_then(|s| s.as_str());
                let is_error = is_assistant_error(error_message.as_deref(), stop_reason);
                let has_visible_content = parts.iter().any(|p| match p {
                    LoadedPart::Text { text } | LoadedPart::Thinking { text } => !text.is_empty(),
                    LoadedPart::Image { .. } => true,
                    LoadedPart::ToolCall { args, .. } => !args.is_empty(),
                    LoadedPart::ToolResult { output, .. } => !output.is_empty(),
                });
                if is_error && !has_visible_content {
                    if let Some(ref error) = error_message {
                        parts = vec![LoadedPart::Text {
                            text: format!("Error: {}", error),
                        }];
                    } else {
                        parts.push(LoadedPart::Text {
                            text: "Error: assistant response failed.".to_string(),
                        });
                    }
                }

                if !parts.is_empty() {
                    loaded.push(LoadedMessage {
                        id,
                        role: "assistant".to_string(),
                        parts,
                        error_message,
                        is_error,
                    });
                }
            }
            "toolResult" => {
                let tool_name = msg
                    .get("toolName")
                    .and_then(|n| n.as_str())
                    .unwrap_or("unknown");
                let mut output = String::new();
                if let Some(content) = msg.get("content") {
                    extract_text_parts(content, &mut output);
                }
                loaded.push(LoadedMessage {
                    id,
                    role: "tool".to_string(),
                    parts: vec![LoadedPart::ToolResult {
                        name: tool_name.to_string(),
                        output: truncate_str(&output, 5000),
                    }],
                    error_message: None,
                    is_error: false,
                });
            }
            "bashExecution" => {
                let command = msg
                    .get("command")
                    .and_then(|c| c.as_str())
                    .unwrap_or("unknown");
                let output = msg.get("output").and_then(|o| o.as_str()).unwrap_or("");
                let exit_code = msg.get("exitCode").and_then(|c| c.as_i64()).unwrap_or(-1);
                loaded.push(LoadedMessage {
                    id,
                    role: "bash".to_string(),
                    parts: vec![LoadedPart::ToolResult {
                        name: "bash".to_string(),
                        output: format!(
                            "`{}` (exit {})\n{}",
                            command,
                            exit_code,
                            truncate_str(output, 5000)
                        ),
                    }],
                    error_message: None,
                    is_error: false,
                });
            }
            _ => {
                log!("unknown message role in history: {}", role);
            }
        }
    }

    Some(loaded)
}

fn extract_text_parts(val: &serde_json::Value, out: &mut String) {
    if let Some(arr) = val.as_array() {
        for part in arr {
            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                out.push_str(text);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum PiRpcError {
    Io(std::io::Error),
    Serde(serde_json::Error),
    Stdout,
    Spawn(String),
    WebSocket(String),
    Runtime(String),
    Models(String),
    Skills(String),
    Extensions(String),
    Prompts(String),
    Bridge(String),
}

impl std::fmt::Display for PiRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PiRpcError::Io(e) => write!(f, "io error: {}", e),
            PiRpcError::Serde(e) => write!(f, "json error: {}", e),
            PiRpcError::Stdout => write!(f, "could not get stdout handle"),
            PiRpcError::Spawn(msg) => write!(f, "spawn error: {}", msg),
            PiRpcError::WebSocket(msg) => write!(f, "websocket error: {}", msg),
            PiRpcError::Runtime(msg) => write!(f, "runtime error: {}", msg),
            PiRpcError::Models(msg) => write!(f, "models error: {}", msg),
            PiRpcError::Skills(msg) => write!(f, "skills error: {}", msg),
            PiRpcError::Extensions(msg) => write!(f, "extensions error: {}", msg),
            PiRpcError::Prompts(msg) => write!(f, "prompts error: {}", msg),
            PiRpcError::Bridge(msg) => write!(f, "bridge error: {}", msg),
        }
    }
}

impl std::error::Error for PiRpcError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PiRpcError::Io(e) => Some(e),
            PiRpcError::Serde(e) => Some(e),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_text_delta_event() {
        let val = serde_json::json!({
            "type": "message_update",
            "assistantMessageEvent": {
                "type": "text_delta",
                "delta": "hello"
            }
        });
        let event = parse_pi_line_value(&val);
        assert!(
            matches!(event, Some(BridgeEvent::TextDelta { ref content }) if content == "hello"),
            "text_delta event should parse: {:?}",
            event
        );
    }

    #[test]
    fn parse_text_start_and_end_events() {
        let start = serde_json::json!({
            "type": "message_update",
            "assistantMessageEvent": { "type": "text_start" }
        });
        assert!(matches!(
            parse_pi_line_value(&start),
            Some(BridgeEvent::TextStart)
        ));

        let end = serde_json::json!({
            "type": "message_update",
            "assistantMessageEvent": {
                "type": "text_end",
                "content": "hello"
            }
        });
        assert!(
            matches!(parse_pi_line_value(&end), Some(BridgeEvent::TextEnd { content }) if content == "hello")
        );
    }

    #[test]
    fn parse_agent_end_with_messages() {
        let val = serde_json::json!({
            "type": "agent_end",
            "messages": [
                {
                    "id": "msg-1",
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "hello" }]
                }
            ]
        });
        let event = parse_pi_line_value(&val);
        match event {
            Some(BridgeEvent::AgentEnd {
                messages: Some(messages),
            }) => {
                assert_eq!(messages.len(), 1);
                assert_eq!(messages[0].role, "assistant");
                assert!(
                    matches!(&messages[0].parts[0], LoadedPart::Text { text } if text == "hello")
                );
            }
            other => panic!("expected AgentEnd with messages, got {:?}", other),
        }
    }

    #[test]
    fn parse_loaded_messages_with_error_message() {
        let val = serde_json::json!([
            {
                "id": "msg-1",
                "role": "assistant",
                "content": [{ "type": "text", "text": "" }],
                "stopReason": "error",
                "errorMessage": "Failed to resolve API key"
            }
        ]);
        let messages = parse_loaded_messages(&val).expect("should parse");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "assistant");
        assert!(messages[0].is_error);
        assert_eq!(
            messages[0].error_message.as_deref(),
            Some("Failed to resolve API key")
        );
        assert!(
            matches!(&messages[0].parts[0], LoadedPart::Text { text } if text == "Error: Failed to resolve API key"),
            "expected error text part, got {:?}",
            messages[0].parts
        );
    }

    #[test]
    fn parse_loaded_messages_with_error_stop_reason_no_message() {
        let val = serde_json::json!([
            {
                "id": "msg-1",
                "role": "assistant",
                "content": [],
                "stopReason": "error"
            }
        ]);
        let messages = parse_loaded_messages(&val).expect("should parse");
        assert_eq!(messages.len(), 1);
        assert!(messages[0].is_error);
        assert!(
            matches!(&messages[0].parts[0], LoadedPart::Text { text } if text == "Error: assistant response failed."),
            "expected fallback error text part, got {:?}",
            messages[0].parts
        );
    }

    #[test]
    fn parse_loaded_messages_with_api_error_stop_reason() {
        let val = serde_json::json!([
            {
                "id": "msg-1",
                "role": "assistant",
                "content": [],
                "stopReason": "api_error"
            }
        ]);
        let messages = parse_loaded_messages(&val).expect("should parse");
        assert_eq!(messages.len(), 1);
        assert!(messages[0].is_error);
        assert_eq!(messages[0].error_message, None);
        assert!(
            matches!(&messages[0].parts[0], LoadedPart::Text { text } if text == "Error: assistant response failed."),
            "expected fallback error text part, got {:?}",
            messages[0].parts
        );
    }

    #[test]
    fn parse_loaded_messages_ignores_empty_error_message() {
        let val = serde_json::json!([
            {
                "id": "msg-1",
                "role": "assistant",
                "content": [{ "type": "text", "text": "hello" }],
                "errorMessage": ""
            }
        ]);
        let messages = parse_loaded_messages(&val).expect("should parse");
        assert_eq!(messages.len(), 1);
        assert!(!messages[0].is_error);
        assert_eq!(messages[0].error_message, None);
        assert!(
            matches!(&messages[0].parts[0], LoadedPart::Text { text } if text == "hello"),
            "expected original text part, got {:?}",
            messages[0].parts
        );
    }

    #[test]
    fn parse_loaded_messages_preserves_error_with_visible_content() {
        let val = serde_json::json!([
            {
                "id": "msg-1",
                "role": "assistant",
                "content": [{ "type": "text", "text": "partial" }],
                "stopReason": "error",
                "errorMessage": "Something went wrong"
            }
        ]);
        let messages = parse_loaded_messages(&val).expect("should parse");
        assert_eq!(messages.len(), 1);
        assert!(messages[0].is_error);
        assert_eq!(
            messages[0].error_message.as_deref(),
            Some("Something went wrong")
        );
        // Visible content is preserved; error_message is surfaced separately.
        assert_eq!(messages[0].parts.len(), 1);
        assert!(
            matches!(&messages[0].parts[0], LoadedPart::Text { text } if text == "partial"),
            "expected original text part, got {:?}",
            messages[0].parts
        );
    }

    #[test]
    fn parse_loaded_messages_preserves_user_images() {
        let val = serde_json::json!([
            {
                "id": "msg-1",
                "role": "user",
                "content": [
                    { "type": "text", "text": "describe this" },
                    { "type": "image", "data": "base64data", "mimeType": "image/png" }
                ]
            }
        ]);
        let messages = parse_loaded_messages(&val).expect("should parse");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].parts.len(), 2);
        assert!(
            matches!(&messages[0].parts[0], LoadedPart::Text { text } if text == "describe this"),
            "expected text part, got {:?}",
            messages[0].parts
        );
        assert!(
            matches!(
                &messages[0].parts[1],
                LoadedPart::Image { data, mime_type }
                if data == "base64data" && mime_type == "image/png"
            ),
            "expected image part, got {:?}",
            messages[0].parts
        );
    }

    #[test]
    fn parse_tool_execution_end_with_details() {
        let val = serde_json::json!({
            "type": "tool_execution_end",
            "toolCallId": "call-1",
            "toolName": "send_file",
            "result": {
                "content": [{ "type": "text", "text": "Sent file: report.txt" }],
                "details": {
                    "path": "/workspace/report.txt",
                    "mime_type": "text/plain",
                    "size": 42
                }
            },
            "isError": false
        });
        match parse_pi_line_value(&val) {
            Some(BridgeEvent::ToolEnd {
                call_id,
                tool_name,
                output,
                is_error,
                details,
            }) => {
                assert_eq!(call_id, "call-1");
                assert_eq!(tool_name, "send_file");
                assert!(!is_error);
                assert!(output.contains("Sent file"));
                let details = details.expect("details should be present");
                assert_eq!(details["path"], "/workspace/report.txt");
                assert_eq!(details["mime_type"], "text/plain");
                assert_eq!(details["size"], 42);
            }
            other => panic!("expected ToolEnd with details, got {:?}", other),
        }
    }
}
