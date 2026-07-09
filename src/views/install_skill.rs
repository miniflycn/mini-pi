use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{
    App, AsyncApp, Bounds, Context, IntoElement, Render, ScrollHandle, SharedString, Styled,
    Window, WindowBounds, WindowDecorations, WindowOptions, div, prelude::*, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::notification::NotificationType;
use gpui_component::pagination::Pagination;
use gpui_component::scroll::Scrollbar;
use gpui_component::{
    ActiveTheme, Disableable as _, Icon, Root, Sizable as _, Size, TitleBar, WindowExt as _,
};

use crate::auth::state::agent_dir;
use crate::core::app::AppStore;
use crate::rpc::pi_rpc::BridgeSkill;
use crate::views::skills_panel::SkillsPanel;

const CLAWHUB_BASE: &str = "https://clawhub.ai";
const LIST_LIMIT: usize = 20;
const LIST_URL_TEMPLATE: &str = "https://clawhub.ai/api/v1/packages?family=skill&sort=recommended";

#[derive(Debug, Clone)]
struct ClawHubSkill {
    name: String,
    display_name: String,
    summary: Option<String>,
    owner_handle: String,
    version: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GitHubSkillDownloadHandoff {
    source_ref: String,
    repo: String,
    commit: String,
    path: String,
    content_hash: String,
    archive_url: String,
}

pub struct InstallSkillWindow {
    loading: bool,
    error: Option<String>,
    skills: Vec<ClawHubSkill>,
    current_query: String,
    current_page: usize,
    page_cursors: Vec<String>,
    has_more: bool,
    search_input: gpui::Entity<InputState>,
    _search_subscription: gpui::Subscription,
    installing: Option<String>,
    scroll_handle: ScrollHandle,
    skills_panel: gpui::Entity<SkillsPanel>,
    search_generation: usize,
}

impl InstallSkillWindow {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        skills_panel: gpui::Entity<SkillsPanel>,
    ) -> Self {
        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search skills...")
                .multi_line(false)
        });
        let search_input_for_sub = search_input.clone();
        let _search_subscription = cx.subscribe(
            &search_input_for_sub,
            |this, _state, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.schedule_search(cx);
                }
            },
        );

        let initial_generation = 1;
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = smol::unblock(move || fetch_clawhub_skills(None, None)).await;
            let _ = weak.update(cx, |this, cx| {
                if this.search_generation != initial_generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((skills, next_cursor)) => {
                        this.store_page_cursor(1, next_cursor);
                        this.skills = skills;
                    }
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();

        Self {
            loading: true,
            error: None,
            skills: Vec::new(),
            current_query: String::new(),
            current_page: 1,
            page_cursors: Vec::new(),
            has_more: false,
            search_input,
            _search_subscription,
            installing: None,
            scroll_handle: ScrollHandle::new(),
            skills_panel,
            search_generation: initial_generation,
        }
    }

    fn total_pages(&self) -> usize {
        if self.has_more {
            self.page_cursors.len() + 1
        } else {
            self.current_page.max(1)
        }
    }

    fn cursor_for_page(&self, page: usize) -> Option<String> {
        if page <= 1 {
            None
        } else {
            self.page_cursors.get(page.saturating_sub(2)).cloned()
        }
    }

    fn store_page_cursor(&mut self, page: usize, next_cursor: Option<String>) {
        self.has_more = next_cursor.is_some();
        if let Some(cursor) = next_cursor {
            let idx = page.saturating_sub(1);
            if self.page_cursors.len() < idx + 1 {
                self.page_cursors.resize(idx + 1, String::new());
            }
            self.page_cursors[idx] = cursor;
        }
    }

    fn next_search_generation(&mut self) -> usize {
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_generation
    }

    fn go_to_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let target = page.max(1);
        if target == self.current_page {
            return;
        }
        // Clamp to the last page we can actually reach.
        let max_reachable = self.total_pages();
        let target = target.min(max_reachable);
        if target == self.current_page {
            return;
        }

        let cursor = self.cursor_for_page(target);
        self.current_page = target;
        self.loading = true;
        self.error = None;
        let generation = self.next_search_generation();
        cx.notify();

        let query = if self.current_query.is_empty() {
            None
        } else {
            Some(self.current_query.clone())
        };
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = smol::unblock(move || fetch_clawhub_skills(query, cursor)).await;
            let _ = weak.update(cx, |this, cx| {
                if this.search_generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((skills, next_cursor)) => {
                        this.store_page_cursor(this.current_page, next_cursor);
                        this.skills = skills;
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

    fn schedule_search(&mut self, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).value().to_string();
        self.current_query = query.trim().to_string();
        self.current_page = 1;
        self.page_cursors.clear();
        self.loading = true;
        self.error = None;
        let generation = self.next_search_generation();
        let query = if self.current_query.is_empty() {
            None
        } else {
            Some(self.current_query.clone())
        };
        cx.notify();

        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            smol::Timer::after(Duration::from_millis(300)).await;
            let should_run = weak
                .update(cx, |this, _cx| this.search_generation == generation)
                .unwrap_or(false);
            if !should_run {
                return;
            }
            let result = smol::unblock(move || fetch_clawhub_skills(query, None)).await;
            let _ = weak.update(cx, |this, cx| {
                if this.search_generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((skills, next_cursor)) => {
                        this.store_page_cursor(1, next_cursor);
                        this.skills = skills;
                    }
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn install(&mut self, skill: ClawHubSkill, window: &mut Window, cx: &mut Context<Self>) {
        self.installing = Some(skill.name.clone());
        cx.notify();

        let name = skill.name.clone();
        let display_name = skill.display_name.clone();
        let owner = skill.owner_handle.clone();
        let window_handle = window.window_handle();
        let weak = cx.entity().downgrade();
        let skills_panel = self.skills_panel.clone();
        cx.spawn(async move |_, cx| {
            let result = smol::unblock(move || install_skill_from_registry(&name, &owner)).await;
            let is_ok = result.is_ok();
            let detail = match result {
                Ok(msg) => msg,
                Err(e) => e,
            };
            let _ = weak.update(cx, |this, cx| {
                this.installing = None;
                cx.notify();
            });
            let _ = cx.update_window(window_handle, |_, window, cx| {
                window.push_notification(
                    (
                        if is_ok {
                            NotificationType::Success
                        } else {
                            NotificationType::Error
                        },
                        if is_ok {
                            format!("Installed skill '{}'", display_name)
                        } else {
                            format!("Failed to install skill '{}': {}", display_name, detail)
                        },
                    ),
                    cx,
                );
            });
            if is_ok {
                refresh_skills_panel(skills_panel, cx).await;
            }
        })
        .detach();
    }

    fn install_local(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });

        let window_handle = window.window_handle();
        let skills_panel = self.skills_panel.clone();
        cx.spawn_in(window, async move |this, cx| {
            let paths = match rx.await {
                Ok(Ok(Some(paths))) => paths,
                _ => return,
            };
            if paths.is_empty() {
                return;
            }
            let src = paths.into_iter().next().unwrap();
            let display_name = src
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("skill")
                .to_string();
            let result = smol::unblock(move || install_local_skill(&src)).await;
            let is_ok = result.is_ok();
            let detail = match result {
                Ok(msg) => msg,
                Err(e) => e,
            };
            let _ = this.update_in(cx, |this, _window, cx| {
                this.installing = None;
                cx.notify();
            });
            let _ = cx.update_window(window_handle, |_, window, cx| {
                window.push_notification(
                    (
                        if is_ok {
                            NotificationType::Success
                        } else {
                            NotificationType::Error
                        },
                        if is_ok {
                            format!("Installed skill '{}'", display_name)
                        } else {
                            format!("Failed to install skill '{}': {}", display_name, detail)
                        },
                    ),
                    cx,
                );
            });
            if is_ok {
                refresh_skills_panel(skills_panel, cx).await;
            }
        })
        .detach();
    }
}

impl Render for InstallSkillWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        let mut body = div()
            .id("install-skill-body")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.font_family.clone());

        body = body.child(
            TitleBar::new().child(
                div().flex().flex_row().items_center().px_2().child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Install Skill"),
                ),
            ),
        );

        let mut content = div()
            .id("install-skill-content")
            .flex()
            .flex_col()
            .flex_1()
            .gap_4();

        content = content.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .px_4()
                .gap_2()
                .child(Input::new(&self.search_input).w_full())
                .child(
                    Button::new("install-from-local")
                        .label("Install from local")
                        .primary()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.install_local(window, cx);
                        })),
                ),
        );

        if let Some(ref err) = self.error {
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
                            .child("Failed to load catalog"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(err.clone()),
                    ),
            );
        } else if self.skills.is_empty() && !self.loading {
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .flex_1()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("No skills found."),
            );
        }

        if !self.skills.is_empty() || self.loading {
            content = content.child(
                div()
                    .id("install-skill-list-container")
                    .relative()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .min_h(px(0.))
                    .when(!self.loading, |this| {
                        this.child(
                            div()
                                .id("skill-list")
                                .flex()
                                .flex_col()
                                .flex_1()
                                .gap_2()
                                .overflow_y_scroll()
                                .track_scroll(&self.scroll_handle)
                                .children(self.skills.iter().map(|skill| {
                                    let is_installing = self
                                        .installing
                                        .as_ref()
                                        .map(|n| n == &skill.name)
                                        .unwrap_or(false);

                                    let card = div()
                                        .id(SharedString::from(format!("skill-{}", skill.name)))
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .px_4()
                                        .py_3()
                                        .rounded_lg()
                                        .bg(cx.theme().secondary)
                                        .child(
                                            div()
                                                .flex()
                                                .flex_row()
                                                .items_center()
                                                .justify_between()
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .flex_1()
                                                        .gap_1()
                                                        .min_w(px(0.))
                                                        .child(
                                                            div()
                                                                .text_sm()
                                                                .font_weight(
                                                                    gpui::FontWeight::SEMIBOLD,
                                                                )
                                                                .text_color(cx.theme().foreground)
                                                                .whitespace_normal()
                                                                .child(skill.display_name.clone()),
                                                        )
                                                        .when_some(
                                                            skill.version.clone(),
                                                            |this, version| {
                                                                this.child(
                                                                    div()
                                                                        .text_xs()
                                                                        .text_color(
                                                                            cx.theme()
                                                                                .muted_foreground,
                                                                        )
                                                                        .whitespace_normal()
                                                                        .child(format!(
                                                                            "v{}",
                                                                            version
                                                                        )),
                                                                )
                                                            },
                                                        )
                                                        .when_some(
                                                            skill
                                                                .summary
                                                                .clone()
                                                                .map(SharedString::from),
                                                            |this, summary| {
                                                                this.child(
                                                                    div()
                                                                        .text_xs()
                                                                        .text_color(
                                                                            cx.theme()
                                                                                .muted_foreground,
                                                                        )
                                                                        .whitespace_normal()
                                                                        .child(summary),
                                                                )
                                                            },
                                                        ),
                                                )
                                                .child(
                                                    Button::new(format!("install-{}", skill.name))
                                                        .with_size(Size::Small)
                                                        .primary()
                                                        .icon(
                                                            Icon::empty()
                                                                .path("icons/download.svg")
                                                                .size(px(14.))
                                                                .text_color(
                                                                    cx.theme().primary_foreground,
                                                                ),
                                                        )
                                                        .loading(is_installing)
                                                        .disabled(is_installing)
                                                        .tooltip("Install skill")
                                                        .on_click(cx.listener({
                                                            let skill = skill.clone();
                                                            move |this, _, window, cx| {
                                                                this.install(
                                                                    skill.clone(),
                                                                    window,
                                                                    cx,
                                                                );
                                                            }
                                                        })),
                                                ),
                                        );

                                    card
                                })),
                        )
                    })
                    .when(self.loading, |this| {
                        this.child(
                            div()
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .flex_1()
                                .gap_2()
                                .child(crate::ui::loader::loader())
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Loading skill catalog..."),
                                ),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .top(px(0.))
                            .right(px(0.))
                            .bottom(px(0.))
                            .w(px(12.))
                            .child(Scrollbar::vertical(&self.scroll_handle)),
                    ),
            );

            if self.total_pages() > 1 {
                let entity = cx.entity();
                content = content.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .px_4()
                        .py_2()
                        .child(
                            Pagination::new("install-skill-pagination")
                                .current_page(self.current_page)
                                .total_pages(self.total_pages())
                                .with_size(Size::Small)
                                .disabled(self.loading)
                                .on_click({
                                    let entity = entity.clone();
                                    move |page, _, cx| {
                                        entity.update(cx, |this, cx| {
                                            this.go_to_page(*page, cx);
                                        });
                                    }
                                }),
                        ),
                );
            }
        }

        body.child(content)
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
            .children(Root::render_sheet_layer(window, cx))
    }
}

pub fn open_install_skill_window(cx: &mut App, skills_panel: gpui::Entity<SkillsPanel>) {
    let width = px(520.0);
    let height = px(420.0);
    let bounds = Bounds::centered(None, size(width, height), cx);
    let window_options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(300.0), px(300.0))),
        titlebar: Some(TitleBar::title_bar_options()),
        window_decorations: if cfg!(target_os = "macos") {
            None
        } else {
            Some(WindowDecorations::Client)
        },
        ..Default::default()
    };

    cx.open_window(window_options, |window, cx| {
        let view = cx.new(|cx| InstallSkillWindow::new(window, cx, skills_panel.clone()));
        cx.new(|cx| Root::new(view, window, cx))
    })
    .expect("failed to open the install skill window");
}

fn parse_clawhub_skill(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<ClawHubSkill, String> {
    let name = obj
        .get("name")
        .or_else(|| obj.get("slug"))
        .and_then(|v| v.as_str())
        .ok_or("missing skill name")?
        .to_string();
    let display_name = obj
        .get("displayName")
        .and_then(|v| v.as_str())
        .unwrap_or(&name)
        .to_string();
    let summary = obj
        .get("summary")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let owner_handle = obj
        .get("ownerHandle")
        .and_then(|v| v.as_str())
        .ok_or("missing ownerHandle")?
        .to_string();
    let version = obj
        .get("latestVersion")
        .and_then(|v| v.get("version"))
        .or_else(|| obj.get("version"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Ok(ClawHubSkill {
        name,
        display_name,
        summary,
        owner_handle,
        version,
    })
}

fn fetch_clawhub_skills(
    query: Option<String>,
    cursor: Option<String>,
) -> Result<(Vec<ClawHubSkill>, Option<String>), String> {
    let is_search = query.as_ref().map(|q| !q.is_empty()).unwrap_or(false);
    let mut url = if is_search {
        url::Url::parse(&format!("{}/api/v1/search", CLAWHUB_BASE))
            .map_err(|e| format!("invalid search URL: {}", e))?
    } else {
        url::Url::parse(LIST_URL_TEMPLATE).map_err(|e| format!("invalid catalog URL: {}", e))?
    };

    if is_search {
        url.query_pairs_mut()
            .append_pair("q", query.as_deref().unwrap_or(""))
            .append_pair("limit", &LIST_LIMIT.to_string());
    } else {
        url.query_pairs_mut()
            .append_pair("limit", &LIST_LIMIT.to_string());
        if let Some(cursor) = &cursor {
            url.query_pairs_mut().append_pair("cursor", cursor);
        }
    }

    let response = reqwest::blocking::get(url.as_str())
        .and_then(|r| r.json::<serde_json::Value>())
        .map_err(|e| e.to_string())?;

    if is_search {
        let results = response
            .get("results")
            .and_then(|v| v.as_array())
            .ok_or("unexpected search response")?;
        let mut skills = Vec::new();
        for item in results {
            let obj = item.as_object().ok_or("invalid search result")?;
            skills.push(parse_clawhub_skill(obj)?);
        }
        Ok((skills, None))
    } else {
        let items = response
            .get("items")
            .and_then(|v| v.as_array())
            .ok_or("unexpected catalog response")?;
        let next_cursor = response
            .get("nextCursor")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let mut skills = Vec::new();
        for item in items {
            let obj = item.as_object().ok_or("invalid catalog item")?;
            skills.push(parse_clawhub_skill(obj)?);
        }
        Ok((skills, next_cursor))
    }
}

fn install_skill_from_registry(slug: &str, owner_handle: &str) -> Result<String, String> {
    let mut url = url::Url::parse(&format!("{}/api/v1/download", CLAWHUB_BASE))
        .map_err(|e| format!("invalid registry URL: {}", e))?;
    url.query_pairs_mut()
        .append_pair("slug", slug)
        .append_pair("ownerHandle", owner_handle);

    let response = reqwest::blocking::get(url.as_str()).map_err(|e| e.to_string())?;
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let target = prepare_skill_target(slug)?;

    if content_type.starts_with("application/json") {
        let handoff: GitHubSkillDownloadHandoff = response.json().map_err(|e| e.to_string())?;
        install_from_github_handoff(&handoff, &target)?;
    } else {
        let bytes = response.bytes().map_err(|e| e.to_string())?;
        extract_zip(&bytes, &target)?;
        normalize_skill_directory(&target)?;
    }

    if !target.join("SKILL.md").exists() {
        let _ = std::fs::remove_dir_all(&target);
        return Err("Downloaded archive does not contain a SKILL.md file.".to_string());
    }

    Ok(format!("Installed '{}' to {}", slug, target.display()))
}

fn install_from_github_handoff(
    handoff: &GitHubSkillDownloadHandoff,
    target: &Path,
) -> Result<(), String> {
    let response = reqwest::blocking::get(&handoff.archive_url).map_err(|e| e.to_string())?;
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = response.bytes().map_err(|e| e.to_string())?;

    if content_type.contains("zip") || handoff.archive_url.ends_with(".zip") {
        extract_zip(&bytes, target)?;
    } else {
        extract_tar_gz(&bytes, target)?;
    }

    normalize_skill_directory(target)?;

    // If the handoff points to a subdirectory inside the repo archive, hoist it.
    if !target.join("SKILL.md").exists() {
        let sub = target.join(&handoff.path);
        if sub.join("SKILL.md").exists() {
            hoist_directory_contents(&sub, target)?;
        }
    }

    Ok(())
}

fn install_local_skill(src: &Path) -> Result<String, String> {
    if !src.join("SKILL.md").exists() {
        return Err("Selected directory does not contain a SKILL.md file.".to_string());
    }

    let dir_name = src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("invalid directory name")?;
    let target = prepare_skill_target(dir_name)?;

    copy_dir_all(src, &target).map_err(|e| format!("failed to copy skill: {}", e))?;

    Ok(format!("Installed '{}' to {}", dir_name, target.display()))
}

fn prepare_skill_target(name: &str) -> Result<PathBuf, String> {
    let skills_dir = agent_dir().join("skills");
    std::fs::create_dir_all(&skills_dir).map_err(|e| e.to_string())?;
    let target = skills_dir.join(name);
    if target.exists() {
        return Err(format!("A skill named '{}' is already installed.", name));
    }
    std::fs::create_dir(&target).map_err(|e| e.to_string())?;
    Ok(target)
}

fn extract_zip(bytes: &[u8], target: &Path) -> Result<(), String> {
    let reader = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| e.to_string())?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        let outpath = target.join(file.mangled_name());
        if file.name().ends_with('/') {
            std::fs::create_dir_all(&outpath).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = outpath.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut outfile = std::fs::File::create(&outpath).map_err(|e| e.to_string())?;
            std::io::copy(&mut file, &mut outfile).map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}

fn extract_tar_gz(bytes: &[u8], target: &Path) -> Result<(), String> {
    let reader = std::io::Cursor::new(bytes);
    let tar = flate2::read::GzDecoder::new(reader);
    let mut archive = tar::Archive::new(tar);
    archive.unpack(target).map_err(|e| e.to_string())?;
    Ok(())
}

fn normalize_skill_directory(target: &Path) -> Result<(), String> {
    if target.join("SKILL.md").exists() {
        return Ok(());
    }

    let entries = std::fs::read_dir(target).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("SKILL.md").exists() {
            hoist_directory_contents(&path, target)?;
            return Ok(());
        }
    }

    Err("Extracted archive does not contain a SKILL.md file.".to_string())
}

fn hoist_directory_contents(src: &Path, dst: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(src).map_err(|e| e.to_string())?.flatten() {
        let dest = dst.join(entry.file_name());
        std::fs::rename(entry.path(), dest).map_err(|e| e.to_string())?;
    }
    std::fs::remove_dir(src).map_err(|e| e.to_string())?;
    Ok(())
}

fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<(), std::io::Error> {
    std::fs::create_dir_all(&dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            std::fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(())
}

async fn refresh_skills_panel(panel: gpui::Entity<SkillsPanel>, cx: &mut AsyncApp) {
    let bridge = cx.update(|cx| cx.global::<AppStore>().pi_bridge.clone());
    if let Some(bridge) = bridge {
        let result: Result<Vec<BridgeSkill>, String> =
            smol::unblock(move || bridge.get_skills().map_err(|e| e.to_string())).await;
        let _ = panel.update(cx, |panel, cx| {
            match result {
                Ok(skills) => panel.skills = skills,
                Err(e) => panel.error = Some(e),
            }
            cx.notify();
        });
    }
}
