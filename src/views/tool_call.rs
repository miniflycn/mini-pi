use std::path::PathBuf;

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Pixels, SharedString,
    StatefulInteractiveElement, Styled, div, prelude::FluentBuilder, px,
};

use gpui_component::button::{Button, ButtonVariants};
use gpui_component::tag::Tag;
use gpui_component::text::{TextView, TextViewState};
use gpui_component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, hover_card::HoverCard};

use crate::data::models::PartState;
use crate::ui::loader::spinner_with;
use crate::views::chat_window::ChatWindow;

/// Where to resolve relative `send_file` paths against. Passed in from the
/// active workspace so the rendered card can Open/Reveal files on disk.
pub type WorkspaceDir = Option<PathBuf>;

/// Color used for the tool-name badge. Tools not in the predefined map get a
/// neutral gray.
fn tool_name_color(name: &str) -> gpui::Hsla {
    static COLORS: std::sync::LazyLock<std::collections::HashMap<&'static str, gpui::Hsla>> =
        std::sync::LazyLock::new(|| {
            let mut m = std::collections::HashMap::new();
            m.insert("read", gpui::rgb(0x3b82f6).into());
            m.insert("write", gpui::rgb(0x22c55e).into());
            m.insert("edit", gpui::rgb(0xf59e0b).into());
            m.insert("bash", gpui::rgb(0xef4444).into());
            m.insert("send_file", gpui::rgb(0x8b5cf6).into());
            m
        });
    COLORS
        .get(name)
        .copied()
        .unwrap_or(gpui::rgb(0x888888).into())
}

fn format_file_size(size: usize) -> String {
    if size < 1024 {
        format!("{} B", size)
    } else if size < 1024 * 1024 {
        format!("{:.1} KB", size as f64 / 1024.0)
    } else if size < 1024 * 1024 * 1024 {
        format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", size as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn open_file(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/c", "start", "", path.to_string_lossy().as_ref()])
            .spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(path).spawn()?;
    }
    Ok(())
}

/// Returns a short title for a tool call based on its name and arguments.
/// `read`/`edit`/`write` show the `path`, `bash` shows the `command`, and all
/// other tools fall back to the full args.
fn format_tool_title(name: &str, args: &str) -> String {
    let key = match name {
        "bash" => Some("command"),
        "read" | "edit" | "write" => Some("path"),
        _ => None,
    };
    if let Some(key) = key {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(args) {
            if let Some(value) = json.get(key) {
                return value
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| value.to_string());
            }
        }
    }
    args.to_string()
}

/// Pretty-prints tool arguments if they are valid JSON; otherwise returns them
/// unchanged.
fn format_tool_input(args: &str) -> String {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(args) {
        serde_json::to_string_pretty(&json).unwrap_or_else(|_| args.to_string())
    } else {
        args.to_string()
    }
}

/// Which slice of a tool-call pair this view represents.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolCallKind {
    /// A `ToolCall` paired with its `ToolResult` (or text output), rendered as
    /// a single compact row with an output popover.
    Paired,
    /// A standalone `ToolCall` without a result, rendered as a muted badge.
    CallOnly,
    /// A standalone `ToolResult` without a call, rendered as a plain output
    /// block (or a file card for `send_file`).
    ResultOnly,
}

/// A self-contained tool-call display component.
///
/// Mirrors `crate::views::reasoning::Reasoning`: holds all the data needed to
/// render a tool-call / tool-result pair (or either side alone) without
/// reaching back into `ChatWindow`. The parent constructs a `ToolCall` per
/// message part during render and calls [`ToolCall::render`].
pub struct ToolCall {
    kind: ToolCallKind,
    name: SharedString,
    args: SharedString,
    output: Option<SharedString>,
    details: Option<serde_json::Value>,
    markdown_entity: Option<gpui::Entity<TextViewState>>,
    workspace_dir: WorkspaceDir,
    assistant_text_width: Pixels,
    state: Option<PartState>,
}

impl ToolCall {
    /// A `ToolCall` paired with its `ToolResult` (or a `Text` part acting as
    /// the result). Renders as a single row with an output popover.
    #[allow(clippy::too_many_arguments)]
    pub fn paired(
        name: SharedString,
        args: SharedString,
        output: Option<SharedString>,
        details: Option<serde_json::Value>,
        markdown_entity: Option<gpui::Entity<TextViewState>>,
        workspace_dir: WorkspaceDir,
        assistant_text_width: Pixels,
        state: Option<PartState>,
    ) -> Self {
        Self {
            kind: ToolCallKind::Paired,
            name,
            args,
            output,
            details,
            markdown_entity,
            workspace_dir,
            assistant_text_width,
            state,
        }
    }

    /// A standalone `ToolCall` part with no paired result.
    pub fn call_only(name: SharedString, args: SharedString, state: Option<PartState>) -> Self {
        Self {
            kind: ToolCallKind::CallOnly,
            name,
            args,
            output: None,
            details: None,
            markdown_entity: None,
            workspace_dir: None,
            assistant_text_width: px(0.),
            state,
        }
    }

    /// A standalone `ToolResult` part with no paired call.
    pub fn result_only(
        name: SharedString,
        output: SharedString,
        details: Option<serde_json::Value>,
        state: Option<PartState>,
        workspace_dir: WorkspaceDir,
    ) -> Self {
        Self {
            kind: ToolCallKind::ResultOnly,
            name,
            args: SharedString::default(),
            output: Some(output),
            details,
            markdown_entity: None,
            workspace_dir,
            assistant_text_width: px(0.),
            state,
        }
    }

    pub fn kind(&self) -> ToolCallKind {
        self.kind
    }

    pub fn render(&self, cx: &mut Context<ChatWindow>, msg_idx: usize) -> AnyElement {
        match self.kind {
            ToolCallKind::Paired => self.render_paired(cx, msg_idx),
            ToolCallKind::CallOnly => self.render_call_only(cx),
            ToolCallKind::ResultOnly => self.render_result_only(cx, msg_idx),
        }
    }

    fn render_paired(&self, cx: &mut Context<ChatWindow>, msg_idx: usize) -> AnyElement {
        if self.name == "send_file" {
            return self.render_send_file_card(cx, msg_idx);
        }

        let name = self.name.clone();
        let args = self.args.clone();
        let title = format_tool_title(name.as_ref(), args.as_ref());
        let input_text = format_tool_input(args.as_ref());
        let output = self.output.clone();
        let markdown_entity = self.markdown_entity.clone();
        let has_detail = !args.is_empty()
            || markdown_entity.is_some()
            || output.as_ref().map_or(false, |o| !o.is_empty());
        let output_text = output.clone();
        let output_markdown = markdown_entity.clone();
        let hover_width = self.assistant_text_width.min(px(480.));

        h_flex()
            .w_full()
            .min_w_0()
            .self_stretch()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(cx.theme().secondary)
                    .text_color(cx.theme().secondary_foreground)
                    .text_xs()
                    .w_full()
                    .min_w_0()
                    .child(
                        Icon::empty()
                            .path("icons/wrench.svg")
                            .size(px(12.))
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                Tag::custom(
                                    tool_name_color(name.as_ref()),
                                    tool_name_color(name.as_ref()),
                                    tool_name_color(name.as_ref()),
                                )
                                .outline()
                                .small()
                                .child(name.to_string()),
                            )
                            .child(
                                div()
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .flex_1()
                                    .min_w_0()
                                    .child(title),
                            ),
                    )
                    .when(has_detail, |this| {
                        this.child(
                            HoverCard::new(format!("tool-detail-{}", msg_idx))
                                .anchor(gpui::Anchor::TopRight)
                                .open_delay(std::time::Duration::from_millis(200))
                                .close_delay(std::time::Duration::from_millis(100))
                                .trigger(
                                    Button::new(format!("tool-detail-btn-{}", msg_idx))
                                        .ghost()
                                        .xsmall()
                                        .icon(
                                            Icon::empty()
                                                .path("icons/notepad-text.svg")
                                                .size(px(14.))
                                                .text_color(cx.theme().muted_foreground),
                                        ),
                                )
                                .content(move |_, _, cx| {
                                    let input_element: AnyElement = div()
                                        .w(hover_width)
                                        .min_w_0()
                                        .child(input_text.clone())
                                        .into_any_element();

                                    let output_element: AnyElement =
                                        if let Some(ref md) = output_markdown {
                                            div()
                                                .flex()
                                                .w(hover_width)
                                                .min_w_0()
                                                .child(div().flex_1().min_w_0().child(
                                                    TextView::new(md).selectable(true).w_full(),
                                                ))
                                                .into_any_element()
                                        } else if let Some(ref text) = output_text {
                                            div()
                                                .w(hover_width)
                                                .min_w_0()
                                                .child(text.to_string())
                                                .into_any_element()
                                        } else {
                                            div().into_any_element()
                                        };

                                    div()
                                        .id(format!("tool-detail-content-{}", msg_idx))
                                        .max_w(px(520.))
                                        .max_h(px(360.))
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .px_2()
                                        .child(
                                            div()
                                                .id(format!("tool-detail-scroll-{}", msg_idx))
                                                .flex_1()
                                                .h_full()
                                                .overflow_y_scroll()
                                                .pb_2()
                                                .flex()
                                                .flex_col()
                                                .gap_3()
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .gap_1()
                                                        .child(
                                                            div()
                                                                .font_weight(
                                                                    gpui::FontWeight::SEMIBOLD,
                                                                )
                                                                .child("Input"),
                                                        )
                                                        .child(
                                                            div()
                                                                .w_full()
                                                                .h_px()
                                                                .bg(cx.theme().border),
                                                        )
                                                        .child(input_element),
                                                )
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .gap_1()
                                                        .child(
                                                            div()
                                                                .font_weight(
                                                                    gpui::FontWeight::SEMIBOLD,
                                                                )
                                                                .child("Output"),
                                                        )
                                                        .child(
                                                            div()
                                                                .w_full()
                                                                .h_px()
                                                                .bg(cx.theme().border),
                                                        )
                                                        .child(output_element),
                                                ),
                                        )
                                }),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_call_only(&self, cx: &mut Context<ChatWindow>) -> AnyElement {
        let name = self.name.clone();
        let args = self.args.clone();
        let title = format_tool_title(name.as_ref(), args.as_ref());
        let is_streaming = self.state == Some(PartState::Streaming);
        div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .self_stretch()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(cx.theme().muted)
                    .text_color(cx.theme().muted_foreground)
                    .text_xs()
                    .w_full()
                    .min_w_0()
                    .child(
                        h_flex()
                            .w_full()
                            .gap_1()
                            .items_start()
                            .child(
                                Icon::empty()
                                    .path("icons/wrench.svg")
                                    .size(px(12.))
                                    .text_color(cx.theme().muted_foreground)
                                    .mt(px(1.)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .flex_1()
                                    .min_w_0()
                                    .items_start()
                                    .gap_1()
                                    .child(
                                        Tag::custom(
                                            tool_name_color(name.as_ref()),
                                            tool_name_color(name.as_ref()),
                                            tool_name_color(name.as_ref()),
                                        )
                                        .outline()
                                        .small()
                                        .child(name.to_string()),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .mt(px(1.))
                                            .opacity(0.75)
                                            .child(title),
                                    )
                                    .when(is_streaming, |this| {
                                        this.child(div().mt(px(1.)).child(spinner_with(
                                            12.0,
                                            u32::from(cx.theme().muted_foreground.to_rgb()) >> 8,
                                        )))
                                    }),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_result_only(&self, cx: &mut Context<ChatWindow>, msg_idx: usize) -> AnyElement {
        if self.name == "send_file" {
            return self.render_send_file_card(cx, msg_idx);
        }

        let output = self.output.clone().unwrap_or_default();
        let header = if self.name.is_empty() || self.name == "unknown" {
            "Output".to_string()
        } else {
            format!("{} Output", self.name)
        };

        div()
            .flex()
            .w_full()
            .justify_start()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(cx.theme().secondary)
                    .text_color(cx.theme().secondary_foreground)
                    .text_xs()
                    .w_full()
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(header))
                    .child(div().w_full().h_px().bg(cx.theme().border))
                    .child(div().opacity(0.75).child(output.to_string())),
            )
            .into_any_element()
    }

    /// Shared `send_file` card used by both `Paired` and `ResultOnly` kinds.
    /// When paired, `output`/`details` come from the `ToolResult`; when
    /// result-only, they come from the standalone result part.
    fn render_send_file_card(&self, cx: &mut Context<ChatWindow>, msg_idx: usize) -> AnyElement {
        let mut file_path = self
            .details
            .as_ref()
            .and_then(|d| d.get("path"))
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_default();
        if file_path.as_os_str().is_empty() {
            if let Ok(args_json) = serde_json::from_str::<serde_json::Value>(self.args.as_ref()) {
                if let Some(path) = args_json.get("path").and_then(|v| v.as_str()) {
                    file_path = PathBuf::from(path);
                }
            }
        }
        if !file_path.is_absolute() {
            if let Some(ws) = &self.workspace_dir {
                file_path = ws.join(file_path);
            }
        }
        let output = self.output.clone().unwrap_or_default();
        let (file_name, mime_type, size) = if file_path.as_os_str().is_empty() {
            (
                if output.is_empty() {
                    "Sent file".to_string()
                } else {
                    output.to_string()
                },
                "application/octet-stream".to_string(),
                0,
            )
        } else {
            (
                file_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                self.details
                    .as_ref()
                    .and_then(|d| d.get("mime_type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("application/octet-stream")
                    .to_string(),
                self.details
                    .as_ref()
                    .and_then(|d| d.get("size"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize,
            )
        };
        let file_path_for_reveal = file_path.clone();
        let file_path_for_open = file_path.clone();
        div()
            .flex()
            .w_full()
            .justify_start()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_2()
                    .rounded_md()
                    .bg(cx.theme().secondary)
                    .text_color(cx.theme().secondary_foreground)
                    .w_full()
                    .child(
                        Icon::empty()
                            .path("icons/file.svg")
                            .size(px(20.))
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(file_name)
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{} • {}", mime_type, format_file_size(size))),
                            ),
                    )
                    .child(
                        Button::new(("open-file", msg_idx as u64))
                            .with_size(Size::XSmall)
                            .label("Open")
                            .on_click(cx.listener(move |_this, _, _window, _cx| {
                                let _ = open_file(&file_path_for_open);
                            })),
                    )
                    .child(
                        Button::new(("reveal-file", msg_idx as u64))
                            .with_size(Size::XSmall)
                            .label("Reveal")
                            .on_click(cx.listener(move |_this, _, _window, cx| {
                                cx.reveal_path(&file_path_for_reveal);
                            })),
                    ),
            )
            .into_any_element()
    }
}
