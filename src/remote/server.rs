use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{Method, StatusCode, header},
    middleware,
    response::{IntoResponse, Json, Response, Sse},
    routing::{get, post},
};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tower_http::cors::{Any, CorsLayer};

use crate::remote::auth::require_bearer_token;
use crate::remote::types::{
    CommandEnvelope, CreateThreadBody, DownloadFileBody, RemoteCommand, RemoteResponse,
    SendMessageBody, SetModelBody, SetThinkingLevelBody, SetWorkspaceBody,
};

const BODY_LIMIT_BYTES: usize = 1024 * 1024; // 1 MiB
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

pub struct RemoteServerHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
    runtime: Option<tokio::runtime::Runtime>,
}

impl Drop for RemoteServerHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        // Gracefully shut down the Tokio runtime from a background thread so the
        // GPUI main thread is not blocked. Active SSE connections get up to two
        // seconds to close before the runtime is forcefully terminated.
        if let Some(runtime) = self.runtime.take() {
            std::thread::spawn(move || {
                runtime.shutdown_timeout(Duration::from_secs(2));
            });
        }
    }
}

pub struct AppState {
    pub bearer_token: Option<String>,
    pub command_sender: mpsc::UnboundedSender<CommandEnvelope>,
}

pub fn start(
    port: u16,
    bearer_token: Option<String>,
    command_sender: mpsc::UnboundedSender<CommandEnvelope>,
) -> Result<(RemoteServerHandle, u16), String> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to create tokio runtime: {}", e))?;

    let state = Arc::new(AppState {
        bearer_token,
        command_sender,
    });

    let addr: SocketAddr = format!("127.0.0.1:{}", port)
        .parse()
        .map_err(|e| format!("invalid bind address: {}", e))?;
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind(addr))
        .map_err(|e| format!("failed to bind {}: {}", addr, e))?;
    let actual_port = listener
        .local_addr()
        .map_err(|e| format!("failed to get local address: {}", e))?
        .port();

    let app = build_router(state);

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    runtime.spawn(async move {
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
        {
            eprintln!("[remote] axum server error: {}", e);
        }
    });

    Ok((
        RemoteServerHandle {
            shutdown_tx: Some(shutdown_tx),
            runtime: Some(runtime),
        },
        actual_port,
    ))
}

fn build_router(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);

    Router::new()
        .route("/status", get(status_handler))
        .route("/models", get(list_models))
        .route("/workspaces", get(list_workspaces))
        .route("/threads", get(list_threads).post(create_thread))
        .route("/threads/:id/open", post(open_thread))
        .route("/threads/:id/messages", get(get_messages))
        .route("/threads/:id/message", post(send_message))
        .route("/threads/:id/abort", post(abort))
        .route("/threads/:id/model", post(set_model))
        .route("/threads/:id/thinking-level", post(set_thinking_level))
        .route("/threads/:id/workspace", post(set_workspace))
        .route("/files/download", post(download_file))
        .layer(DefaultBodyLimit::max(BODY_LIMIT_BYTES))
        .layer(cors)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer_token,
        ))
        .with_state(state)
}

async fn status_handler(State(state): State<Arc<AppState>>) -> Response {
    let value = send_command(RemoteCommand::Status, &state.command_sender).await;
    json_response(StatusCode::OK, value)
}

async fn list_models(State(state): State<Arc<AppState>>) -> Response {
    let value = send_command(RemoteCommand::GetModels, &state.command_sender).await;
    json_response(http_status_for(&value, 200), value)
}

async fn list_workspaces(State(state): State<Arc<AppState>>) -> Response {
    let value = send_command(RemoteCommand::ListWorkspaces, &state.command_sender).await;
    json_response(http_status_for(&value, 200), value)
}

#[derive(Deserialize)]
struct ThreadsQuery {
    #[serde(default = "default_page")]
    page: usize,
    #[serde(default = "default_per_page")]
    per_page: usize,
}

fn default_page() -> usize {
    1
}

fn default_per_page() -> usize {
    20
}

async fn list_threads(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ThreadsQuery>,
) -> Response {
    let value = send_command(
        RemoteCommand::ListThreads {
            page: query.page,
            per_page: query.per_page,
        },
        &state.command_sender,
    )
    .await;
    json_response(http_status_for(&value, 200), value)
}

async fn create_thread(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload: CreateThreadBody = match parse_json_body(&body) {
        Ok(p) => p,
        Err(response) => return response.into(),
    };
    let value = send_command(
        RemoteCommand::CreateThread {
            workspace_id: payload.workspace_id,
            model_id: payload.model_id,
        },
        &state.command_sender,
    )
    .await;
    json_response(http_status_for(&value, 201), value)
}

async fn open_thread(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let thread_id = match parse_thread_id(&id) {
        Ok(id) => id,
        Err(response) => return response.into(),
    };
    let value = send_command(
        RemoteCommand::OpenThread { thread_id },
        &state.command_sender,
    )
    .await;
    json_response(http_status_for(&value, 200), value)
}

#[derive(Deserialize)]
struct MessagesQuery {
    since_id: Option<String>,
}

async fn get_messages(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<MessagesQuery>,
) -> Response {
    let thread_id = match parse_thread_id(&id) {
        Ok(id) => id,
        Err(response) => return response.into(),
    };
    let value = send_command(
        RemoteCommand::GetMessages {
            thread_id,
            since_id: query.since_id,
        },
        &state.command_sender,
    )
    .await;
    json_response(http_status_for(&value, 200), value)
}

async fn send_message(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let thread_id = match parse_thread_id(&id) {
        Ok(id) => id,
        Err(response) => return response.into(),
    };
    let payload: SendMessageBody = match parse_json_body(&body) {
        Ok(p) => p,
        Err(response) => return response.into(),
    };
    let (stream_tx, stream_rx) = mpsc::unbounded_channel();
    let value = send_command(
        RemoteCommand::SendMessageStream {
            thread_id,
            message: payload.message,
            sender: stream_tx,
        },
        &state.command_sender,
    )
    .await;

    if value.get("error").is_some() {
        return json_response(http_status_for(&value, 500), value);
    }

    let stream = UnboundedReceiverStream::new(stream_rx)
        .map(|event| Ok::<_, std::convert::Infallible>(event.to_axum_event()));

    ai_stream_response(
        Sse::new(stream).keep_alive(
            axum::response::sse::KeepAlive::new()
                .interval(HEARTBEAT_INTERVAL)
                .text("ping"),
        ),
    )
}

async fn abort(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let thread_id = match parse_thread_id(&id) {
        Ok(id) => id,
        Err(response) => return response.into(),
    };
    let value = send_command(RemoteCommand::Abort { thread_id }, &state.command_sender).await;
    json_response(http_status_for(&value, 200), value)
}

async fn set_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let thread_id = match parse_thread_id(&id) {
        Ok(id) => id,
        Err(response) => return response.into(),
    };
    let payload: SetModelBody = match parse_json_body(&body) {
        Ok(p) => p,
        Err(response) => return response.into(),
    };
    let value = send_command(
        RemoteCommand::SetModel {
            thread_id,
            model_id: payload.model_id,
        },
        &state.command_sender,
    )
    .await;
    json_response(http_status_for(&value, 200), value)
}

async fn set_thinking_level(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let thread_id = match parse_thread_id(&id) {
        Ok(id) => id,
        Err(response) => return response.into(),
    };
    let payload: SetThinkingLevelBody = match parse_json_body(&body) {
        Ok(p) => p,
        Err(response) => return response.into(),
    };
    let value = send_command(
        RemoteCommand::SetThinkingLevel {
            thread_id,
            thinking_level: payload.thinking_level,
        },
        &state.command_sender,
    )
    .await;
    json_response(http_status_for(&value, 200), value)
}

async fn set_workspace(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let thread_id = match parse_thread_id(&id) {
        Ok(id) => id,
        Err(response) => return response.into(),
    };
    let payload: SetWorkspaceBody = match parse_json_body(&body) {
        Ok(p) => p,
        Err(response) => return response.into(),
    };
    let value = send_command(
        RemoteCommand::SetWorkspace {
            thread_id,
            workspace_id: payload.workspace_id,
        },
        &state.command_sender,
    )
    .await;
    json_response(http_status_for(&value, 200), value)
}

async fn download_file(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let payload: DownloadFileBody = match parse_json_body(&body) {
        Ok(p) => p,
        Err(response) => return response.into(),
    };
    let value = send_command(
        RemoteCommand::DownloadFile {
            path: payload.path,
            mime_type: payload.mime_type,
        },
        &state.command_sender,
    )
    .await;

    if value.get("error").is_some() {
        return json_response(http_status_for(&value, 200), value);
    }

    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("download");
    let mime_type = value
        .get("mime_type")
        .and_then(|v| v.as_str())
        .unwrap_or("application/octet-stream");
    let data = match value.get("data").and_then(|v| v.as_str()).and_then(|s| {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.decode(s).ok()
    }) {
        Some(bytes) => bytes,
        None => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "invalid file data" }),
            );
        }
    };

    let mut response = (StatusCode::OK, data).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_str(mime_type)
            .unwrap_or_else(|_| header::HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        header::HeaderValue::from_str(&format!("attachment; filename=\"{}\"", name))
            .unwrap_or_else(|_| {
                header::HeaderValue::from_static("attachment; filename=\"download\"")
            }),
    );
    response
}

async fn send_command(
    command: RemoteCommand,
    command_sender: &mpsc::UnboundedSender<CommandEnvelope>,
) -> RemoteResponse {
    let (tx, rx) = oneshot::channel();
    if command_sender
        .send(CommandEnvelope {
            command,
            respond_to: tx,
        })
        .is_err()
    {
        return json!({ "error": "remote controller unavailable" });
    }
    match rx.await {
        Ok(value) => value,
        Err(_) => json!({ "error": "remote controller dropped" }),
    }
}

#[allow(dead_code)]
enum ParseError {
    BadThreadId,
    InvalidBody(String),
}

impl From<ParseError> for Response {
    fn from(err: ParseError) -> Self {
        match err {
            ParseError::BadThreadId => json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "invalid thread id" }),
            ),
            ParseError::InvalidBody(msg) => {
                json_response(StatusCode::BAD_REQUEST, json!({ "error": msg }))
            }
        }
    }
}

fn parse_thread_id(id: &str) -> Result<String, ParseError> {
    Ok(id.to_string())
}

fn parse_json_body<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, ParseError> {
    let body = String::from_utf8(body.to_vec())
        .map_err(|_| ParseError::InvalidBody("invalid UTF-8 body".to_string()))?;
    serde_json::from_str(&body).map_err(|e| ParseError::InvalidBody(format!("invalid body: {}", e)))
}

fn json_response(status: StatusCode, value: RemoteResponse) -> Response {
    (status, Json(value)).into_response()
}

fn ai_stream_response<S, E>(sse: Sse<S>) -> Response
where
    S: futures::Stream<Item = Result<axum::response::sse::Event, E>> + Send + 'static,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let mut response = sse.into_response();
    let headers = response.headers_mut();
    headers.insert(
        "x-vercel-ai-ui-message-stream",
        header::HeaderValue::from_static("v1"),
    );
    headers.insert("x-accel-buffering", header::HeaderValue::from_static("no"));
    headers.insert(
        header::CONNECTION,
        header::HeaderValue::from_static("keep-alive"),
    );
    response
}

/// Map a command result to an HTTP status code. Errors that clearly indicate a
/// client mistake become 4xx; everything else is treated as a server-side failure.
fn http_status_for(value: &RemoteResponse, ok_status: u16) -> StatusCode {
    if value.get("error").is_none() {
        return StatusCode::from_u16(ok_status).unwrap_or(StatusCode::OK);
    }
    let error = value.get("error").and_then(|e| e.as_str()).unwrap_or("");
    if error == "thread not found"
        || (error.starts_with("workspace ") && error.contains("not found"))
    {
        StatusCode::NOT_FOUND
    } else if error.starts_with("unknown model_id") || error == "invalid thread id" {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::types::AiStreamEvent;
    use futures::executor::block_on;

    fn make_dummy_messages() -> Vec<serde_json::Value> {
        vec![
            json!({
                "id": "msg1",
                "entry_id": "entry1",
                "role": "user",
                "parts": [{"type": "text", "text": "hello", "state": null}],
            }),
            json!({
                "id": "msg2",
                "entry_id": "entry2",
                "role": "assistant",
                "parts": [{"type": "text", "text": "hi", "state": null}],
            }),
            json!({
                "id": "msg3",
                "entry_id": "entry3",
                "role": "user",
                "parts": [{"type": "text", "text": "again", "state": null}],
            }),
        ]
    }

    fn spawn_dummy_controller(mut rx: tokio::sync::mpsc::UnboundedReceiver<CommandEnvelope>) {
        use base64::Engine;
        std::thread::spawn(move || {
            while let Some(envelope) = block_on(rx.recv()) {
                let value = match envelope.command {
                    RemoteCommand::Status => {
                        json!({ "enabled": true, "status": "running", "tunnel_url": "https://example.com" })
                    }
                    RemoteCommand::ListThreads { page, per_page } => {
                        let all_threads: Vec<serde_json::Value> = (1..=5)
                            .map(|id| {
                                let id = format!("thread-{}", id);
                                json!({
                                    "id": id,
                                    "title": format!("thread {}", id),
                                    "preview": null,
                                    "session_file": null,
                                    "model": null,
                                    "thinking_level": null,
                                    "pinned": false,
                                    "metadata": {},
                                    "created_at": "2024-01-01T00:00:00Z",
                                    "updated_at": "2024-01-01T00:00:00Z",
                                })
                            })
                            .collect();
                        let offset = (page - 1) * per_page;
                        let threads = all_threads
                            .into_iter()
                            .skip(offset)
                            .take(per_page)
                            .collect::<Vec<_>>();
                        let total_pages = (5 + per_page - 1) / per_page;
                        json!({
                            "threads": threads,
                            "pagination": {
                                "page": page,
                                "per_page": per_page,
                                "total": 5,
                                "total_pages": total_pages,
                            }
                        })
                    }
                    RemoteCommand::GetMessages { since_id, .. } => {
                        let messages = make_dummy_messages();
                        let start_idx = since_id
                            .as_ref()
                            .and_then(|sid| {
                                messages
                                    .iter()
                                    .position(|m| m["id"].as_str() == Some(sid.as_str()))
                            })
                            .map(|i| i + 1)
                            .unwrap_or(0);
                        json!(messages.into_iter().skip(start_idx).collect::<Vec<_>>())
                    }
                    RemoteCommand::SendMessageStream { sender, .. } => {
                        let _ = sender.send(AiStreamEvent::Chunk(json!({
                            "type": "start",
                            "messageId": "assistant-1",
                        })));
                        let _ = sender.send(AiStreamEvent::Chunk(json!({
                            "type": "text-start",
                            "id": "text-0",
                        })));
                        let _ = sender.send(AiStreamEvent::Chunk(json!({
                            "type": "text-delta",
                            "id": "text-0",
                            "delta": "hello",
                        })));
                        let _ = sender.send(AiStreamEvent::Chunk(json!({
                            "type": "text-end",
                            "id": "text-0",
                        })));
                        let _ = sender.send(AiStreamEvent::Chunk(json!({
                            "type": "finish-step",
                        })));
                        let _ = sender.send(AiStreamEvent::Chunk(json!({
                            "type": "finish",
                            "finishReason": "stop",
                        })));
                        let _ = sender.send(AiStreamEvent::Done);
                        json!({ "status": "streaming" })
                    }
                    RemoteCommand::ListWorkspaces => {
                        json!({
                            "workspaces": [
                                {
                                    "id": "ws-1",
                                    "name": "Default",
                                    "path": "/home/user/.mini-pi/workspace",
                                    "created_at": "2024-01-01T00:00:00Z",
                                    "updated_at": "2024-01-01T00:00:00Z",
                                }
                            ]
                        })
                    }
                    RemoteCommand::DownloadFile { mime_type, .. } => {
                        let data = base64::engine::general_purpose::STANDARD.encode("hello file");
                        json!({
                            "name": "report.txt",
                            "mime_type": mime_type.unwrap_or_else(|| "text/plain".to_string()),
                            "size": data.len(),
                            "data": data,
                        })
                    }
                    _ => json!({ "error": "unexpected command" }),
                };
                let _ = envelope.respond_to.send(value);
            }
        });
    }

    #[test]
    fn server_responds_to_status() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        spawn_dummy_controller(rx);
        let (_handle, port) = start(0, None, tx).expect("server should start");

        let client = reqwest::blocking::Client::new();
        let response = client
            .get(&format!("http://127.0.0.1:{}/status", port))
            .send()
            .expect("request should succeed");
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().expect("response should be JSON");
        assert_eq!(body["status"], "running");
    }

    #[test]
    fn server_rejects_bearer_token() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        spawn_dummy_controller(rx);
        let (_handle, port) =
            start(0, Some("secret".to_string()), tx).expect("server should start");

        let client = reqwest::blocking::Client::new();
        let response = client
            .get(&format!("http://127.0.0.1:{}/status", port))
            .send()
            .expect("request should succeed");
        assert_eq!(response.status(), 401);
    }

    #[test]
    fn server_allows_with_bearer_token() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        spawn_dummy_controller(rx);
        let (_handle, port) =
            start(0, Some("secret".to_string()), tx).expect("server should start");

        let client = reqwest::blocking::Client::new();
        let response = client
            .get(&format!("http://127.0.0.1:{}/status", port))
            .bearer_auth("secret")
            .send()
            .expect("request should succeed");
        assert_eq!(response.status(), 200);
    }

    #[test]
    fn options_preflight_succeeds_without_bearer_token() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        spawn_dummy_controller(rx);
        let (_handle, port) =
            start(0, Some("secret".to_string()), tx).expect("server should start");

        let client = reqwest::blocking::Client::new();
        let response = client
            .request(
                reqwest::Method::OPTIONS,
                &format!("http://127.0.0.1:{}/threads/1/message", port),
            )
            .header("Origin", "https://example.com")
            .header("Access-Control-Request-Method", "POST")
            .header(
                "Access-Control-Request-Headers",
                "authorization,content-type",
            )
            .send()
            .expect("request should succeed");

        assert_eq!(response.status(), 200);
        assert!(
            response
                .headers()
                .contains_key("access-control-allow-origin"),
            "missing CORS allow-origin header"
        );
        assert!(
            response
                .headers()
                .get("access-control-allow-methods")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .contains("POST"),
            "missing CORS allow-methods header"
        );
    }

    #[test]
    fn list_threads_uses_pagination_defaults() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        spawn_dummy_controller(rx);
        let (_handle, port) = start(0, None, tx).expect("server should start");

        let client = reqwest::blocking::Client::new();
        let response = client
            .get(&format!("http://127.0.0.1:{}/threads", port))
            .send()
            .expect("request should succeed");
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().expect("response should be JSON");
        assert!(body["threads"].is_array());
        assert_eq!(body["pagination"]["page"], 1);
        assert_eq!(body["pagination"]["per_page"], 20);
        assert_eq!(body["pagination"]["total"], 5);
    }

    #[test]
    fn list_threads_honors_pagination_params() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        spawn_dummy_controller(rx);
        let (_handle, port) = start(0, None, tx).expect("server should start");

        let client = reqwest::blocking::Client::new();
        let response = client
            .get(&format!(
                "http://127.0.0.1:{}/threads?page=2&per_page=2",
                port
            ))
            .send()
            .expect("request should succeed");
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().expect("response should be JSON");
        let ids: Vec<&str> = body["threads"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["thread-3", "thread-4"]);
        assert_eq!(body["pagination"]["page"], 2);
        assert_eq!(body["pagination"]["per_page"], 2);
        assert_eq!(body["pagination"]["total_pages"], 3);
    }

    #[test]
    fn list_workspaces_returns_workspaces() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        spawn_dummy_controller(rx);
        let (_handle, port) = start(0, None, tx).expect("server should start");

        let client = reqwest::blocking::Client::new();
        let response = client
            .get(&format!("http://127.0.0.1:{}/workspaces", port))
            .send()
            .expect("request should succeed");
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().expect("response should be JSON");
        let workspaces = body["workspaces"]
            .as_array()
            .expect("workspaces should be an array");
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0]["id"], "ws-1");
        assert_eq!(workspaces[0]["name"], "Default");
    }

    #[test]
    fn messages_since_id_returns_only_newer() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        spawn_dummy_controller(rx);
        let (_handle, port) = start(0, None, tx).expect("server should start");

        let client = reqwest::blocking::Client::new();
        let response = client
            .get(&format!(
                "http://127.0.0.1:{}/threads/1/messages?since_id=msg1",
                port
            ))
            .send()
            .expect("request should succeed");
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().expect("response should be JSON");
        let ids: Vec<&str> = body
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["msg2", "msg3"]);
    }

    #[test]
    fn post_message_stream_returns_ai_sdk_sse() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        spawn_dummy_controller(rx);
        let (_handle, port) = start(0, None, tx).expect("server should start");

        let client = reqwest::blocking::Client::new();
        let response = client
            .post(&format!("http://127.0.0.1:{}/threads/1/message", port))
            .header("Origin", "https://example.com")
            .json(&json!({ "message": "hello" }))
            .send()
            .expect("request should succeed");

        assert_eq!(response.status(), 200);
        assert_eq!(
            response
                .headers()
                .get("x-vercel-ai-ui-message-stream")
                .and_then(|value| value.to_str().ok()),
            Some("v1")
        );
        assert_eq!(
            response
                .headers()
                .get("x-accel-buffering")
                .and_then(|value| value.to_str().ok()),
            Some("no")
        );
        assert!(
            response
                .headers()
                .contains_key("access-control-allow-origin"),
            "missing CORS header"
        );

        let body = response.text().expect("response body should be readable");
        assert!(body.contains("\"type\":\"start\""));
        assert!(body.contains("\"messageId\":\"assistant-1\""));
        assert!(body.contains("\"type\":\"text-delta\""));
        assert!(body.contains("\"id\":\"text-0\""));
        assert!(body.contains("\"delta\":\"hello\""));
        assert_eq!(
            body.matches("\"type\":\"text-delta\"").count(),
            1,
            "text deltas should be coalesced into a single event"
        );
        assert!(body.contains("data: [DONE]"));
    }

    #[test]
    fn download_file_returns_bytes_with_headers() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        spawn_dummy_controller(rx);
        let (_handle, port) = start(0, None, tx).expect("server should start");

        let client = reqwest::blocking::Client::new();
        let response = client
            .post(&format!("http://127.0.0.1:{}/files/download", port))
            .json(&json!({ "path": "/home/user/.mini-pi/workspace/report.txt" }))
            .send()
            .expect("request should succeed");

        assert_eq!(response.status(), 200);
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("text/plain")
        );
        assert!(
            response
                .headers()
                .get("content-disposition")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .contains("report.txt")
        );
        assert_eq!(response.text().expect("body should be text"), "hello file");
    }
}
