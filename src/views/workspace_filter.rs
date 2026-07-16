use std::sync::Arc;

use gpui::{
    App, ElementId, InteractiveElement, IntoElement, MouseButton, ParentElement, RenderOnce,
    SharedString, Styled, Window, div, px,
};

use gpui_component::{
    ActiveTheme, Icon, Sizable as _, Size,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    tag::Tag,
};

use crate::data::store::WorkspaceMeta;
use crate::utils::color::{workspace_color, workspace_foreground};

/// A closable tag showing the active workspace filter.
#[derive(IntoElement)]
pub struct WorkspaceFilterTag {
    workspace: WorkspaceMeta,
    on_clear: Arc<dyn Fn(&mut Window, &mut App)>,
}

impl WorkspaceFilterTag {
    pub fn new(
        workspace: WorkspaceMeta,
        on_clear: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            workspace,
            on_clear: Arc::new(on_clear),
        }
    }
}

impl RenderOnce for WorkspaceFilterTag {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let on_clear = self.on_clear.clone();

        let bg = workspace_color(&self.workspace.name);
        let fg = workspace_foreground(bg);

        h_flex()
            .gap_1()
            .items_center()
            .child(Tag::custom(bg, fg, bg).small().child(self.workspace.name))
            .child(
                Button::new("clear-workspace-filter")
                    .with_size(Size::XSmall)
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(gpui::rgba(0x00000000).into())
                            .foreground(theme.muted_foreground.into())
                            .hover(theme.secondary_hover.into())
                            .active(theme.secondary_active.into()),
                    )
                    .icon(
                        Icon::empty()
                            .path("icons/close.svg")
                            .size(px(12.))
                            .text_color(theme.muted_foreground),
                    )
                    .cursor_default()
                    .on_click(move |_event, window, cx| {
                        on_clear(window, cx);
                    }),
            )
    }
}

/// A floating popover listing workspaces. Selecting a workspace invokes
/// `on_select` with the chosen workspace; selecting "All workspaces" invokes
/// it with `None`.
#[derive(IntoElement)]
pub struct WorkspaceFilterPopover {
    workspaces: Vec<WorkspaceMeta>,
    on_select: Arc<dyn Fn(Option<WorkspaceMeta>, &mut Window, &mut App)>,
}

impl WorkspaceFilterPopover {
    pub fn new(
        workspaces: Vec<WorkspaceMeta>,
        on_select: impl Fn(Option<WorkspaceMeta>, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            workspaces,
            on_select: Arc::new(on_select),
        }
    }
}

impl RenderOnce for WorkspaceFilterPopover {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let on_select = self.on_select.clone();

        let mut workspaces = self.workspaces;
        workspaces.sort_by(|a, b| {
            if a.name == "Default" {
                std::cmp::Ordering::Less
            } else if b.name == "Default" {
                std::cmp::Ordering::Greater
            } else {
                a.name.cmp(&b.name)
            }
        });

        let workspace_rows: Vec<_> = workspaces
            .into_iter()
            .map(|ws| {
                let ws_id = ws.id.clone();
                let name = ws.name.clone();
                let on_select = on_select.clone();
                let ws_for_callback = ws.clone();
                let bg = workspace_color(&name);
                let fg = workspace_foreground(bg);
                div()
                    .id(ElementId::from(SharedString::from(format!(
                        "workspace-filter-{}",
                        ws_id
                    ))))
                    .cursor_pointer()
                    .child(Tag::custom(bg, fg, bg).small().child(name))
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        on_select(Some(ws_for_callback.clone()), window, cx);
                    })
            })
            .collect();

        h_flex()
            .flex_wrap()
            .gap_1()
            .px_2()
            .py_1()
            .children(workspace_rows)
    }
}
