use serde::Deserialize;
use tokio::sync::{mpsc::UnboundedSender, oneshot};

/// A command plus a channel to return the response.
#[derive(Debug)]
pub struct CommandEnvelope {
    pub command: RemoteCommand,
    pub respond_to: oneshot::Sender<RemoteResponse>,
}

/// A command sent from the HTTP server thread into the GPUI main thread.
#[derive(Debug, Clone)]
pub enum RemoteCommand {
    Status,
    GetModels,
    ListWorkspaces,
    ListThreads {
        page: usize,
        per_page: usize,
    },
    CreateThread {
        workspace_id: Option<String>,
        model_id: Option<String>,
    },
    OpenThread {
        thread_id: String,
    },
    SendMessageStream {
        thread_id: String,
        message: String,
        sender: UnboundedSender<AiStreamEvent>,
    },
    GetMessages {
        thread_id: String,
        since_id: Option<String>,
    },
    Abort {
        thread_id: String,
    },
    SetModel {
        thread_id: String,
        model_id: String,
    },
    SetThinkingLevel {
        thread_id: String,
        thinking_level: String,
    },
    SetWorkspace {
        thread_id: String,
        workspace_id: String,
    },
    DownloadFile {
        path: String,
        mime_type: Option<String>,
    },
}

/// JSON response returned for ordinary (non-SSE) requests.
pub type RemoteResponse = serde_json::Value;

/// Data-only SSE events compatible with the AI SDK UI message stream protocol.
#[derive(Debug, Clone)]
pub enum AiStreamEvent {
    Chunk(serde_json::Value),
    Done,
}

impl AiStreamEvent {
    #[cfg(test)]
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Self::Chunk(data) => encode_data_sse(&data.to_string()),
            Self::Done => encode_data_sse("[DONE]"),
        }
    }

    /// Convert to an `axum` SSE event for streaming responses.
    pub fn to_axum_event(&self) -> axum::response::sse::Event {
        match self {
            Self::Chunk(data) => axum::response::sse::Event::default().data(data.to_string()),
            Self::Done => axum::response::sse::Event::default().data("[DONE]"),
        }
    }
}

/// Encode a data-only SSE frame. Multi-line data is split into multiple data lines.
#[cfg(test)]
fn encode_data_sse(data: &str) -> Vec<u8> {
    let mut out = String::new();
    for line in data.split('\n') {
        out.push_str(&format!("data: {}\n", line));
    }
    out.push('\n');
    out.into_bytes()
}

/// Request bodies.
#[derive(Debug, Deserialize)]
pub struct SendMessageBody {
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct SetModelBody {
    pub model_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SetThinkingLevelBody {
    pub thinking_level: String,
}

#[derive(Debug, Deserialize)]
pub struct SetWorkspaceBody {
    pub workspace_id: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateThreadBody {
    pub workspace_id: Option<String>,
    pub model_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DownloadFileBody {
    pub path: String,
    pub mime_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_stream_event_single_line() {
        let ev = AiStreamEvent::Chunk(serde_json::json!({"x": 1}));
        assert_eq!(
            String::from_utf8(ev.to_bytes()).unwrap(),
            "data: {\"x\":1}\n\n"
        );
    }

    #[test]
    fn ai_stream_done_is_data_frame() {
        assert_eq!(
            String::from_utf8(AiStreamEvent::Done.to_bytes()).unwrap(),
            "data: [DONE]\n\n"
        );
    }
}
