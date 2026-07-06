use std::path::Path;

use gpui::{
    Action, App, Bounds, Context, IntoElement, Render, ScrollHandle, SharedString, Window,
    WindowBounds, WindowDecorations, WindowOptions, div, prelude::*, px, size,
};
use gpui_component::ActiveTheme;
use gpui_component::accordion::Accordion;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::notification::Notification;
use gpui_component::scroll::Scrollbar;
use gpui_component::{Icon, Root, Sizable as _, Size, TitleBar, WindowExt as _};

use crate::auth::state::agent_dir;
use crate::core::actions::OpenInstallExtensionWindow;
use crate::core::app::AppStore;
use crate::rpc::pi_rpc::{BridgeExtension, BridgePrompt, BridgeSkill};
use crate::ui::loader::loader;
use crate::views::install_skill::open_install_skill_window;

type ResourceLoadResult =
    Result<(Vec<BridgeSkill>, Vec<BridgeExtension>, Vec<BridgePrompt>), String>;

/// A panel that lists the effective skills, extensions, and prompts currently
/// loaded by the pi-bridge runtime.
pub struct SkillsPanel {
    pub(crate) skills: Vec<BridgeSkill>,
    pub(crate) extensions: Vec<BridgeExtension>,
    pub(crate) prompts: Vec<BridgePrompt>,
    pub(crate) loading: bool,
    loaded: bool,
    pub(crate) error: Option<String>,
    scroll_handle: ScrollHandle,
    open_ixs: Vec<usize>,
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
            open_ixs: vec![0, 1, 2],
        }
    }

    fn toggle_accordion(&mut self, open_ixs: Vec<usize>, _: &mut Window, cx: &mut Context<Self>) {
        self.open_ixs = open_ixs;
        cx.notify();
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
            let result: ResourceLoadResult = smol::unblock(move || {
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
            let skills_panel = cx.entity();
            let skills_panel_for_skill = skills_panel.clone();

            let add_skill_button = Button::new("add-skill")
                .with_size(Size::Small)
                .ghost()
                .icon(
                    Icon::empty()
                        .path("icons/plus.svg")
                        .size(px(14.))
                        .text_color(cx.theme().muted_foreground),
                )
                .on_click(cx.listener(move |_this, _, _window, cx| {
                    open_install_skill_window(cx, skills_panel_for_skill.clone());
                }));

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

            let add_prompt_button = Button::new("add-prompt")
                .with_size(Size::Small)
                .ghost()
                .icon(
                    Icon::empty()
                        .path("icons/plus.svg")
                        .size(px(14.))
                        .text_color(cx.theme().muted_foreground),
                )
                .on_click(cx.listener(move |_this, _, _window, cx| {
                    open_create_prompt_window(cx, skills_panel.clone());
                }));

            let skill_items: Vec<_> = self
                .skills
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
                .collect();
            let prompt_items: Vec<_> = self
                .prompts
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
                .collect();
            let extension_items: Vec<_> = self
                .extensions
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
                .collect();

            content = content.child(
                div().flex().flex_col().child(
                    Accordion::new("skills-accordion")
                        .multiple(true)
                        .with_size(Size::Small)
                        .item(|this| {
                            this.open(self.open_ixs.contains(&0))
                                .title(section_title("Skills", add_skill_button, cx))
                                .child(render_resource_list("skills", skill_items, cx))
                        })
                        .item(|this| {
                            this.open(self.open_ixs.contains(&1))
                                .title(section_title("Prompts", add_prompt_button, cx))
                                .child(render_resource_list("prompts", prompt_items, cx))
                        })
                        .item(|this| {
                            this.open(self.open_ixs.contains(&2))
                                .title(section_title("Extensions", add_extension_button, cx))
                                .child(render_resource_list("extensions", extension_items, cx))
                        })
                        .on_toggle_click(cx.listener(|this, open_ixs: &[usize], window, cx| {
                            this.toggle_accordion(open_ixs.to_vec(), window, cx);
                        })),
                ),
            );
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

fn section_title(
    title: &str,
    right_child: impl IntoElement,
    cx: &mut gpui::Context<SkillsPanel>,
) -> impl IntoElement {
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
                .child(SharedString::from(title)),
        )
        .child(right_child)
}

fn render_resource_list(
    title: &str,
    items: Vec<(SharedString, Option<SharedString>, Option<SharedString>)>,
    cx: &mut gpui::Context<SkillsPanel>,
) -> impl IntoElement {
    let mut section = div()
        .id(SharedString::from(format!(
            "{}-section",
            title.to_lowercase()
        )))
        .flex()
        .flex_col()
        .gap_2();

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

            if let Some(desc) = description
                && !desc.is_empty()
            {
                card = card.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(desc),
                );
            }

            section = section.child(card);
        }
    }

    section
}

fn sanitize_prompt_filename(title: &str) -> String {
    let mut sanitized: String = title
        .chars()
        .map(|c| {
            if c.is_whitespace() || c == '_' {
                '-'
            } else {
                c
            }
        })
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect();

    // Collapse consecutive hyphens.
    let mut collapsed = String::with_capacity(sanitized.len());
    let mut prev = None;
    for c in sanitized.chars() {
        if c == '-' && prev == Some('-') {
            continue;
        }
        collapsed.push(c);
        prev = Some(c);
    }
    sanitized = collapsed;

    sanitized = sanitized.trim_matches('-').to_string();
    if sanitized.is_empty() {
        "prompt".to_string()
    } else {
        sanitized
    }
}

// -----------------------------------------------------------------------------
// Create-prompt window
// -----------------------------------------------------------------------------

pub struct CreatePromptWindow {
    title_input: gpui::Entity<InputState>,
    body_input: gpui::Entity<InputState>,
    error: Option<String>,
    skills_panel: gpui::Entity<SkillsPanel>,
    _title_sub: gpui::Subscription,
    _body_sub: gpui::Subscription,
}

impl CreatePromptWindow {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        skills_panel: gpui::Entity<SkillsPanel>,
    ) -> Self {
        let title_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Prompt name")
                .multi_line(false)
        });
        let body_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Prompt content...")
                .multi_line(true)
                .rows(10)
                .submit_on_enter(false)
        });

        let _title_sub = cx.observe(&title_input, |_, _, cx| {
            cx.notify();
        });
        let _body_sub = cx.observe(&body_input, |_, _, cx| {
            cx.notify();
        });

        Self {
            title_input,
            body_input,
            error: None,
            skills_panel,
            _title_sub,
            _body_sub,
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title_input.read(cx).value().to_string();
        let body = self.body_input.read(cx).value().to_string();

        if title.trim().is_empty() {
            self.error = Some("Prompt title is required.".to_string());
            cx.notify();
            return;
        }
        if body.trim().is_empty() {
            self.error = Some("Prompt content is required.".to_string());
            cx.notify();
            return;
        }

        let file_name = format!("{}.md", sanitize_prompt_filename(&title));
        let prompts_dir = agent_dir().join("prompts");
        let file_path = prompts_dir.join(&file_name);

        let bridge = cx.global::<AppStore>().pi_bridge.clone();
        if bridge.is_none() {
            self.error = Some("SDK bridge is not connected.".to_string());
            cx.notify();
            return;
        }
        let bridge = bridge.unwrap();

        self.error = None;
        cx.notify();

        let title_for_notif = title.trim().to_string();
        let view = cx.entity();
        let skills_panel = self.skills_panel.clone();
        let main_window = cx.global::<AppStore>().main_window;
        let window_handle = window.window_handle();
        cx.spawn(async move |_, cx| {
            let result: Result<Vec<BridgePrompt>, String> = smol::unblock(move || {
                std::fs::create_dir_all(&prompts_dir)
                    .map_err(|e| format!("failed to create prompts directory: {}", e))?;
                if file_path.exists() {
                    return Err(format!(
                        "A prompt named '{}' already exists.",
                        file_name.strip_suffix(".md").unwrap_or(&file_name)
                    ));
                }
                std::fs::write(&file_path, &body)
                    .map_err(|e| format!("failed to write prompt file: {}", e))?;
                bridge.get_prompts().map_err(|e| e.to_string())
            })
            .await;

            match result {
                Ok(prompts) => {
                    let _ = skills_panel.update(cx, |panel, cx| {
                        panel.prompts = prompts;
                        panel.loading = false;
                        cx.notify();
                    });
                    if let Some(main) = main_window {
                        let _ = cx.update_window(main, |_, window, cx| {
                            window.push_notification(
                                Notification::success(format!(
                                    "Prompt '{}' saved.",
                                    title_for_notif
                                )),
                                cx,
                            );
                        });
                    }
                    let _ = window_handle.update(cx, |_, window, _cx| {
                        window.remove_window();
                    });
                }
                Err(e) => {
                    let _ = view.update(cx, |this, cx| {
                        this.error = Some(e);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }
}

impl Render for CreatePromptWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let error_msg = self.error.clone().map(SharedString::from);

        div()
            .id("create-prompt-window")
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.font_family.clone())
            .child(
                TitleBar::new().child(
                    div().flex().flex_row().items_center().px_2().child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("New Prompt"),
                    ),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .p_4()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("TITLE"),
                            )
                            .child(Input::new(&self.title_input).w_full()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .flex_1()
                            .min_h(px(120.))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("PROMPT"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .child(Input::new(&self.body_input).w_full().h_full()),
                            ),
                    )
                    .when_some(error_msg, |this, err| {
                        this.child(div().text_xs().text_color(cx.theme().danger).child(err))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("create-prompt-cancel")
                                    .label("Cancel")
                                    .with_size(Size::Small)
                                    .ghost()
                                    .on_click(cx.listener(|_this, _, window, _cx| {
                                        window.remove_window();
                                    })),
                            )
                            .child(
                                Button::new("create-prompt-save")
                                    .label("Save")
                                    .with_size(Size::Small)
                                    .primary()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save(window, cx);
                                    })),
                            ),
                    ),
            )
    }
}

fn open_create_prompt_window(cx: &mut App, skills_panel: gpui::Entity<SkillsPanel>) {
    let width = px(520.0);
    let height = px(420.0);
    let bounds = Bounds::centered(None, size(width, height), cx);
    let window_options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(360.0), px(300.0))),
        titlebar: Some(TitleBar::title_bar_options()),
        window_decorations: if cfg!(target_os = "macos") {
            None
        } else {
            Some(WindowDecorations::Client)
        },
        ..Default::default()
    };

    cx.open_window(window_options, |window, cx| {
        let view = cx.new(|cx| CreatePromptWindow::new(window, cx, skills_panel.clone()));
        cx.new(|cx| Root::new(view, window, cx))
    })
    .expect("failed to open the create prompt window");
}
