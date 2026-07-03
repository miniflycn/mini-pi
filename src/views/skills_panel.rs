use std::path::Path;

use gpui::{
    Action, Context, IntoElement, Render, ScrollHandle, SharedString, Window, div, prelude::*, px,
};
use gpui_component::ActiveTheme;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::scroll::Scrollbar;
use gpui_component::{Icon, Sizable as _, Size};

use crate::core::actions::OpenInstallExtensionWindow;
use crate::core::app::AppStore;
use crate::rpc::pi_rpc::{BridgeExtension, BridgePrompt, BridgeSkill};
use crate::ui::loader::loader;

/// A panel that lists the effective skills, extensions, and prompts currently
/// loaded by the pi-bridge runtime.
pub struct SkillsPanel {
    skills: Vec<BridgeSkill>,
    extensions: Vec<BridgeExtension>,
    prompts: Vec<BridgePrompt>,
    loading: bool,
    loaded: bool,
    error: Option<String>,
    scroll_handle: ScrollHandle,
}

impl SkillsPanel {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            skills: Vec::new(),
            extensions: Vec::new(),
            prompts: Vec::new(),
            loading: false,
            loaded: false,
            error: None,
            scroll_handle: ScrollHandle::new(),
        }
    }

    pub fn load_if_needed(&mut self, cx: &mut Context<Self>) {
        if self.loaded || self.loading {
            return;
        }

        let bridge = cx.global::<AppStore>().pi_bridge.clone();
        if bridge.is_none() {
            self.loaded = true;
            self.loading = false;
            self.error = Some("SDK bridge is not connected.".to_string());
            cx.notify();
            return;
        }

        self.loading = true;
        cx.notify();

        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result: Result<
                (Vec<BridgeSkill>, Vec<BridgeExtension>, Vec<BridgePrompt>),
                String,
            > = smol::unblock(move || {
                let bridge = bridge.as_ref().unwrap();
                let skills = bridge.get_skills().map_err(|e| e.to_string())?;
                let extensions = bridge.get_extensions().map_err(|e| e.to_string())?;
                let prompts = bridge.get_prompts().map_err(|e| e.to_string())?;
                Ok((skills, extensions, prompts))
            })
            .await;

            let _ = weak.update(cx, |this, cx| {
                this.loading = false;
                this.loaded = true;
                match result {
                    Ok((skills, extensions, prompts)) => {
                        this.skills = skills;
                        this.extensions = extensions;
                        this.prompts = prompts;
                    }
                    Err(e) => {
                        this.error = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for SkillsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut content = div()
            .id("skills-panel-content")
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .p_4()
            .gap_4()
            .bg(cx.theme().background);

        if self.loading {
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .flex_1()
                    .gap_2()
                    .child(loader())
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Loading skills, extensions & prompts..."),
                    ),
            );
        } else if let Some(ref err) = self.error {
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .flex_1()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(cx.theme().danger)
                            .child("Failed to load resources"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(err.clone()),
                    ),
            );
        } else {
            let add_extension_button = Button::new("add-extension")
                .with_size(Size::Small)
                .ghost()
                .icon(
                    Icon::empty()
                        .path("icons/plus.svg")
                        .size(px(14.))
                        .text_color(cx.theme().muted_foreground),
                )
                .on_click(cx.listener(|_this, _, window, cx| {
                    window.dispatch_action(OpenInstallExtensionWindow.boxed_clone(), cx);
                }));

            content = content
                .child(render_section(
                    "Skills",
                    self.skills
                        .iter()
                        .map(|s| {
                            (
                                SharedString::from(s.name.clone()),
                                s.description.clone().map(SharedString::from),
                                s.raw
                                    .get("filePath")
                                    .and_then(|v| v.as_str())
                                    .map(SharedString::from),
                            )
                        })
                        .collect(),
                    div(),
                    cx,
                ))
                .child(render_section(
                    "Extensions",
                    self.extensions
                        .iter()
                        .map(|e| {
                            let basename = Path::new(&e.name)
                                .file_name()
                                .and_then(|f| f.to_str())
                                .unwrap_or(&e.name)
                                .to_string();
                            (
                                SharedString::from(basename),
                                Some(SharedString::from(e.name.clone())),
                                e.raw
                                    .get("resolvedPath")
                                    .and_then(|v| v.as_str())
                                    .or_else(|| e.raw.get("path").and_then(|v| v.as_str()))
                                    .map(SharedString::from),
                            )
                        })
                        .collect(),
                    add_extension_button,
                    cx,
                ))
                .child(render_section(
                    "Prompts",
                    self.prompts
                        .iter()
                        .map(|p| {
                            (
                                SharedString::from(p.name.clone()),
                                p.description.clone().map(SharedString::from),
                                p.raw
                                    .get("filePath")
                                    .and_then(|v| v.as_str())
                                    .map(SharedString::from),
                            )
                        })
                        .collect(),
                    div(),
                    cx,
                ));
        }

        div()
            .id("skills-panel")
            .relative()
            .size_full()
            .bg(cx.theme().background)
            .child(content)
            .child(
                div()
                    .absolute()
                    .top(px(0.))
                    .right(px(0.))
                    .bottom(px(0.))
                    .w(px(12.))
                    .child(Scrollbar::vertical(&self.scroll_handle)),
            )
    }
}

fn render_section(
    title: &str,
    items: Vec<(SharedString, Option<SharedString>, Option<SharedString>)>,
    right_child: impl IntoElement,
    cx: &mut gpui::Context<SkillsPanel>,
) -> impl IntoElement {
    let title_string = SharedString::from(title);
    let mut section = div()
        .id(SharedString::from(format!(
            "{}-section",
            title.to_lowercase()
        )))
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .px_2()
                .py_1()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title_string.clone()),
                )
                .child(right_child),
        );

    if items.is_empty() {
        section = section.child(
            div()
                .px_4()
                .py_3()
                .rounded_lg()
                .bg(cx.theme().secondary)
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(format!("No {} loaded.", title.to_lowercase())),
        );
    } else {
        for (name, description, path) in items {
            let display_name = name.clone();
            let mut card = div()
                .id(SharedString::from(format!(
                    "resource-item-{}",
                    display_name
                )))
                .flex()
                .flex_col()
                .gap_1()
                .px_4()
                .py_3()
                .rounded_lg()
                .bg(cx.theme().secondary)
                .hover(|style| style.bg(cx.theme().secondary_hover))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_sm()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .child(display_name.clone()),
                        )
                        .when_some(path.clone(), |this, path| {
                            this.child(
                                Button::new(SharedString::from(format!("reveal-{}", display_name)))
                                    .with_size(Size::XSmall)
                                    .ghost()
                                    .icon(
                                        Icon::empty()
                                            .path("icons/external-link.svg")
                                            .size(px(14.))
                                            .text_color(cx.theme().muted_foreground),
                                    )
                                    .on_click(cx.listener(move |_this, _, _window, cx| {
                                        cx.reveal_path(std::path::Path::new(path.as_ref()));
                                    })),
                            )
                        }),
                );

            if let Some(desc) = description {
                if !desc.is_empty() {
                    card = card.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(desc),
                    );
                }
            }

            section = section.child(card);
        }
    }

    section
}
