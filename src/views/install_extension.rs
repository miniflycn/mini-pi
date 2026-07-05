use std::time::Duration;

use gpui::{
    App, Bounds, Context, IntoElement, Render, ScrollHandle, SharedString, Styled, Window,
    WindowBounds, WindowDecorations, WindowOptions, div, prelude::*, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::notification::NotificationType;
use gpui_component::pagination::Pagination;
use gpui_component::scroll::Scrollbar;
use gpui_component::{ActiveTheme, Disableable as _, Icon, Root, Sizable as _, Size, TitleBar, WindowExt as _};

use crate::auth::state::{agent_dir, bun_cache_dir};
use crate::utils::paths::find_bun;

const SEARCH_URL_TEMPLATE: &str = "https://registry.npmjs.org/-/v1/search";
const DEFAULT_NPM_QUERY: &str = "@remnic/plugin-pi";
const PAGE_SIZE: usize = 20;

#[derive(Debug, Clone)]
struct NpmPackage {
    name: String,
    description: Option<String>,
    version: String,
}

pub struct InstallExtensionWindow {
    loading: bool,
    error: Option<String>,
    packages: Vec<NpmPackage>,
    current_query: String,
    current_page: usize,
    page_cursors: Vec<String>,
    has_more: bool,
    search_input: gpui::Entity<InputState>,
    _search_subscription: gpui::Subscription,
    installing: Option<String>,
    scroll_handle: ScrollHandle,
    search_generation: usize,
}

impl InstallExtensionWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search extensions...")
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
            let result = smol::unblock(move || fetch_npm_packages(None, None)).await;
            let _ = weak.update(cx, |this, cx| {
                if this.search_generation != initial_generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((packages, next_cursor)) => {
                        this.store_page_cursor(1, next_cursor);
                        this.packages = packages;
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
            packages: Vec::new(),
            current_query: String::new(),
            current_page: 1,
            page_cursors: Vec::new(),
            has_more: false,
            search_input,
            _search_subscription,
            installing: None,
            scroll_handle: ScrollHandle::new(),
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
            let result = smol::unblock(move || fetch_npm_packages(query, cursor)).await;
            let _ = weak.update(cx, |this, cx| {
                if this.search_generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((packages, next_cursor)) => {
                        this.store_page_cursor(this.current_page, next_cursor);
                        this.packages = packages;
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
            let result = smol::unblock(move || fetch_npm_packages(query, None)).await;
            let _ = weak.update(cx, |this, cx| {
                if this.search_generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((packages, next_cursor)) => {
                        this.store_page_cursor(1, next_cursor);
                        this.packages = packages;
                    }
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn install(&mut self, package_name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.installing = Some(package_name.clone());
        cx.notify();

        let agent_dir = agent_dir();
        let name_for_notification = package_name.clone();
        let window_handle = window.window_handle();
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = smol::unblock(move || run_bun_install(&package_name, &agent_dir)).await;
            let is_ok = result.is_ok();
            let detail = match result {
                Ok(output) => output,
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
                            format!("Installed extension '{}'", name_for_notification)
                        } else {
                            format!(
                                "Failed to install extension '{}': {}",
                                name_for_notification, detail
                            )
                        },
                    ),
                    cx,
                );
            });
        })
        .detach();
    }
}

impl Render for InstallExtensionWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        let mut body = div()
            .id("install-extension-body")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.font_family.clone());

        body = body.child(
            TitleBar::new().child(
                div().flex().flex_row().items_center().px_2().child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Install Extension"),
                ),
            ),
        );

        body = body.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .px_4()
                .gap_2()
                .child(Input::new(&self.search_input).w_full()),
        );

        if self.loading && self.packages.is_empty() {
            body = body.child(
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
                            .child("Searching registry..."),
                    ),
            );
        } else if let Some(ref err) = self.error {
            body = body
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(cx.theme().danger)
                        .child("Failed to search npm"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(err.clone()),
                );
        } else if self.packages.is_empty() && !self.loading {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .flex_1()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("No packages found."),
            );
        } else {
            let list = div()
                .id("package-list")
                .flex()
                .flex_col()
                .flex_1()
                .gap_2()
                .overflow_y_scroll()
                .track_scroll(&self.scroll_handle)
                .when(!self.loading, |this| {
                    this.children(self.packages.iter().map(|pkg| {
                        let is_installing = self
                            .installing
                            .as_ref()
                            .map(|n| n == &pkg.name)
                            .unwrap_or(false);

                    let card = div()
                        .id(SharedString::from(format!("package-{}", pkg.name)))
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
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .text_color(cx.theme().foreground)
                                                .whitespace_normal()
                                                .child(pkg.name.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .whitespace_normal()
                                                .child(format!("v{}", pkg.version)),
                                        )
                                        .when_some(
                                            pkg.description.clone().map(SharedString::from),
                                            |this, desc| {
                                                this.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .whitespace_normal()
                                                        .child(desc),
                                                )
                                            },
                                        ),
                                )
                                .child(
                                    Button::new(format!("install-{}", pkg.name))
                                        .with_size(gpui_component::Size::Small)
                                        .primary()
                                        .icon(
                                            Icon::empty()
                                                .path("icons/download.svg")
                                                .size(px(14.))
                                                .text_color(cx.theme().primary_foreground),
                                        )
                                        .loading(is_installing)
                                        .disabled(is_installing)
                                        .tooltip("Install extension")
                                        .on_click(cx.listener({
                                            let name = pkg.name.clone();
                                            move |this, _, window, cx| {
                                                this.install(name.clone(), window, cx);
                                            }
                                        })),
                                ),
                        );

                        card
                    }))
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
                                    .child("Searching registry..."),
                            ),
                    )
                });

            body = body.child(
                div()
                    .relative()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .min_h(px(0.))
                    .child(list)
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
                body = body.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .px_4()
                        .py_2()
                        .child(
                            Pagination::new("install-extension-pagination")
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

        body
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
            .children(Root::render_sheet_layer(window, cx))
    }
}

pub fn open_install_extension_window(cx: &mut App) {
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
        let view = cx.new(|cx| InstallExtensionWindow::new(window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    })
    .expect("failed to open the install extension window");
}

fn fetch_npm_packages(
    query: Option<String>,
    cursor: Option<String>,
) -> Result<(Vec<NpmPackage>, Option<String>), String> {
    let mut url = url::Url::parse(SEARCH_URL_TEMPLATE)
        .map_err(|e| format!("invalid search URL: {}", e))?;
    url.query_pairs_mut()
        .append_pair("text", query.as_deref().unwrap_or(DEFAULT_NPM_QUERY))
        .append_pair("size", &PAGE_SIZE.to_string());
    if let Some(cursor) = &cursor {
        url.query_pairs_mut().append_pair("from", cursor);
    }

    let response = reqwest::blocking::get(url.as_str())
        .and_then(|r| r.json::<serde_json::Value>())
        .map_err(|e| e.to_string())?;

    let objects = response
        .get("objects")
        .and_then(|v| v.as_array())
        .ok_or("unexpected npm search response")?;
    let total = response
        .get("total")
        .and_then(|v| v.as_u64())
        .unwrap_or(objects.len() as u64) as usize;

    let next_cursor = if objects.len() < PAGE_SIZE {
        None
    } else {
        let from = cursor
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);
        let next_from = from + objects.len();
        if next_from >= total {
            None
        } else {
            Some(next_from.to_string())
        }
    };

    let mut packages = Vec::new();
    for obj in objects {
        let package = obj
            .get("package")
            .and_then(|v| v.as_object())
            .ok_or("missing package object")?;
        let name = package
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or("missing package name")?
            .to_string();
        let description = package
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let version = package
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        packages.push(NpmPackage {
            name,
            description,
            version,
        });
    }

    Ok((packages, next_cursor))
}

fn run_bun_install(package_name: &str, cwd: &std::path::Path) -> Result<String, String> {
    let bun =
        find_bun().ok_or("Bun runtime not found. Make sure the app is installed correctly.")?;
    let cache_dir = bun_cache_dir();

    let mut cmd = std::process::Command::new(&bun);
    cmd.args(["install", package_name])
        .env("BUN_INSTALL", &cache_dir)
        .env("BUN_INSTALL_CACHE_DIR", cache_dir.join("install-cache"))
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    let output = cmd
        .output()
        .map_err(|e| format!("failed to run bun: {}", e))?;

    let mut text = String::new();
    if let Ok(stdout) = String::from_utf8(output.stdout.clone()) {
        text.push_str(&stdout);
    }
    if let Ok(stderr) = String::from_utf8(output.stderr.clone()) {
        if !stderr.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&stderr);
        }
    }

    if output.status.success() {
        Ok(text)
    } else {
        Err(format!("bun install failed: {}", text))
    }
}
