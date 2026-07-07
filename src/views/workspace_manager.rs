use gpui::{
    App, ClickEvent, Context, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_component::{ActiveTheme as _, Icon, Sizable as _, Size, WindowExt as _};

use crate::data::store::WorkspaceMeta;

#[derive(Clone)]
pub enum WorkspaceManagerEvent {
    AddRequested,
    CloseRequested,
    DeleteRequested { workspace_id: String },
}

pub struct WorkspaceManager {
    workspaces: Vec<WorkspaceMeta>,
    confirming_id: Option<String>,
}

impl WorkspaceManager {
    pub fn new(workspaces: Vec<WorkspaceMeta>) -> Self {
        Self {
            workspaces,
            confirming_id: None,
        }
    }

    pub fn set_workspaces(&mut self, workspaces: Vec<WorkspaceMeta>) {
        self.workspaces = workspaces;
        self.confirming_id = None;
    }

    pub fn set_confirming(&mut self, workspace_id: Option<String>) {
        self.confirming_id = workspace_id;
    }

    pub fn render_dialog_content(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let entity = cx.entity();
        let theme = cx.theme().clone();
        let confirming_id = self.confirming_id.clone();
        let filtered: Vec<_> = self
            .workspaces
            .iter()
            .filter(|ws| ws.name != "Default")
            .collect();

        if filtered.is_empty() {
            div()
                .id("workspace-modal-list")
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .p_3()
                .h(px(120.))
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("No workspaces yet"),
                )
                .child(
                    Button::new("modal-add-workspace-btn-empty")
                        .label("+ Add Workspace")
                        .with_size(Size::Small)
                        .custom(
                            ButtonCustomVariant::new(cx)
                                .color(theme.primary.into())
                                .foreground(theme.primary_foreground.into())
                                .hover(theme.primary_hover.into())
                                .active(theme.primary_active.into()),
                        )
                        .on_click({
                            let entity = entity.clone();
                            move |_: &ClickEvent, _window: &mut Window, cx: &mut App| {
                                entity.update(cx, |_this, cx| {
                                    cx.emit(WorkspaceManagerEvent::AddRequested);
                                });
                            }
                        }),
                )
                .into_any_element()
        } else {
            let theme_for_rows = theme.clone();
            let entity_for_rows = entity.clone();
            div()
                .id("workspace-modal-list")
                .flex()
                .flex_col()
                .gap_1()
                .p_3()
                .overflow_y_scroll()
                .child(
                    Button::new("modal-add-workspace-btn")
                        .label("+ Add Workspace")
                        .with_size(Size::Small)
                        .custom(
                            ButtonCustomVariant::new(cx)
                                .color(theme.primary.into())
                                .foreground(theme.primary_foreground.into())
                                .hover(theme.primary_hover.into())
                                .active(theme.primary_active.into()),
                        )
                        .on_click({
                            let entity = entity.clone();
                            move |_: &ClickEvent, _window: &mut Window, cx: &mut App| {
                                entity.update(cx, |_this, cx| {
                                    cx.emit(WorkspaceManagerEvent::AddRequested);
                                });
                            }
                        }),
                )
                .children(filtered.into_iter().map(move |ws| {
                    let ws_id = ws.id.clone();
                    let is_confirming = confirming_id.as_ref() == Some(&ws_id);
                    div()
                        .id(SharedString::from(format!("ws-modal-{ws_id}")))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(theme_for_rows.secondary)
                        .hover(|style| style.bg(theme_for_rows.secondary_hover))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme_for_rows.foreground)
                                        .child(ws.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme_for_rows.muted_foreground)
                                        .child(ws.path.clone()),
                                ),
                        )
                        .child(if !is_confirming {
                            Button::new(SharedString::from(format!("ws-delete-{ws_id}")))
                                .with_size(Size::XSmall)
                                .custom(
                                    ButtonCustomVariant::new(cx)
                                        .color(gpui::rgba(0x00000000).into())
                                        .foreground(theme_for_rows.muted_foreground.into())
                                        .hover(theme_for_rows.danger_hover.into())
                                        .active(theme_for_rows.danger_active.into()),
                                )
                                .icon(
                                    Icon::empty()
                                        .path("icons/close.svg")
                                        .size(px(12.))
                                        .text_color(theme_for_rows.muted_foreground),
                                )
                                .on_click({
                                    let entity = entity_for_rows.clone();
                                    let ws_id = ws_id.clone();
                                    move |_: &ClickEvent, _window: &mut Window, cx: &mut App| {
                                        cx.stop_propagation();
                                        entity.update(cx, |this, cx| {
                                            this.confirming_id = Some(ws_id.clone());
                                            cx.notify();
                                        });
                                    }
                                })
                                .into_any_element()
                        } else {
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme_for_rows.danger)
                                        .child("Delete?"),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "ws-confirm-delete-{ws_id}"
                                    )))
                                    .label("Yes")
                                    .with_size(Size::XSmall)
                                    .danger()
                                    .on_click({
                                        let entity = entity_for_rows.clone();
                                        let ws_id = ws_id.clone();
                                        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                                            cx.stop_propagation();
                                            window.close_dialog(cx);
                                            entity.update(cx, |this, cx| {
                                                this.confirming_id = None;
                                                cx.emit(WorkspaceManagerEvent::DeleteRequested {
                                                    workspace_id: ws_id.clone(),
                                                });
                                            });
                                        }
                                    }),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "ws-cancel-delete-{ws_id}"
                                    )))
                                    .label("No")
                                    .with_size(Size::XSmall)
                                    .on_click({
                                        let entity = entity_for_rows.clone();
                                        move |_: &ClickEvent, _window: &mut Window, cx: &mut App| {
                                            cx.stop_propagation();
                                            entity.update(cx, |this, cx| {
                                                this.confirming_id = None;
                                                cx.notify();
                                            });
                                        }
                                    }),
                                )
                                .into_any_element()
                        })
                }))
                .into_any_element()
        }
    }
}

impl EventEmitter<WorkspaceManagerEvent> for WorkspaceManager {}
