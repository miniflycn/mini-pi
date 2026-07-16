use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px, svg,
};

use crate::data::models::PartState;
use crate::ui::loader::spinner_with;
use gpui_component::ActiveTheme as _;
use gpui_component::collapsible::Collapsible;

/// A self-contained reasoning/thinking display component.
///
/// Manages its own collapsed/expanded state internally and renders via
/// `gpui_component::Collapsible`.
pub struct Reasoning {
    id: SharedString,
    content: SharedString,
    collapsed: bool,
    state: Option<PartState>,
}

impl Reasoning {
    pub fn new(
        id: impl Into<SharedString>,
        content: impl Into<SharedString>,
        state: Option<PartState>,
    ) -> Self {
        Self {
            id: id.into(),
            content: content.into(),
            collapsed: true,
            state,
        }
    }

    pub fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }

    pub fn set_content(&mut self, content: impl Into<SharedString>, state: Option<PartState>) {
        self.content = content.into();
        self.state = state;
    }
}

impl Render for Reasoning {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let collapsed = self.collapsed;
        let content = self.content.clone();
        let is_streaming = self.state == Some(PartState::Streaming);

        Collapsible::new()
            .open(!collapsed)
            .bg(cx.theme().secondary)
            .rounded_md()
            .w_full()
            .child(
                div()
                    .id(format!("reasoning-toggle-{}", self.id))
                    .px_2()
                    .py_1()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .items_center()
                    .cursor_pointer()
                    .child(
                        svg()
                            .path("icons/thinking.svg")
                            .size(px(12.))
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("Thinking {}", if collapsed { "▶" } else { "▼" })),
                    )
                    .child(div().flex_1())
                    .when(is_streaming, |this| {
                        this.child(spinner_with(
                            12.0,
                            u32::from(cx.theme().muted_foreground.to_rgb()) >> 8,
                        ))
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.collapsed = !this.collapsed;
                        cx.notify();
                    })),
            )
            .content(
                div()
                    .px_2()
                    .pb_2()
                    .text_xs()
                    .text_color(cx.theme().secondary_foreground)
                    .opacity(0.75)
                    .child(content),
            )
    }
}
