use gpui::{
    App, Bounds, Context, IntoElement, Render, ScrollHandle, SharedString, Styled, Window,
    WindowBounds, WindowDecorations, WindowOptions, div, prelude::*, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::scroll::Scrollbar;
use gpui_component::{ActiveTheme, Disableable as _, Sizable as _, TitleBar};

use crate::auth::state::agent_dir;

const SEARCH_URL: &str = "https://registry.npmjs.org/-/v1/search?text=@remnic/plugin-pi";

#[derive(Debug, Clone)]
struct NpmPackage {
    name: String,
    description: Option<String>,
    version: String,
}

#[derive(Debug, Clone)]
enum InstallResult {
    Success(String),
    Error(String),
}

pub struct InstallExtensionWindow {
    loading: bool,
    error: Option<String>,
    packages: Vec<NpmPackage>,
    installing: Option<String>,
    last_result: Option<(String, InstallResult)>,
    scroll_handle: ScrollHandle,
}

impl InstallExtensionWindow {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = smol::unblock(fetch_npm_packages).await;
            let _ = weak.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(packages) => this.packages = packages,
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
            installing: None,
            last_result: None,
            scroll_handle: ScrollHandle::new(),
        }
    }

    fn install(&mut self, package_name: String, cx: &mut Context<Self>) {
        self.installing = Some(package_name.clone());
        self.last_result = None;
        cx.notify();

        let agent_dir = agent_dir();
        let name_for_update = package_name.clone();
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = smol::unblock(move || run_npm_install(&package_name, &agent_dir)).await;
            let _ = weak.update(cx, |this, cx| {
                this.installing = None;
                this.last_result = Some((
                    name_for_update,
                    match result {
                        Ok(output) => InstallResult::Success(output),
                        Err(e) => InstallResult::Error(e),
                    },
                ));
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for InstallExtensionWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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

        if self.loading {
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
                            .child("Searching npm..."),
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
        } else if self.packages.is_empty() {
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
                .children(self.packages.iter().map(|pkg| {
                    let is_installing = self
                        .installing
                        .as_ref()
                        .map(|n| n == &pkg.name)
                        .unwrap_or(false);
                    let result = self.last_result.as_ref().filter(|(n, _)| n == &pkg.name);

                    let mut card = div()
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
                                        .label("Install")
                                        .disabled(is_installing)
                                        .on_click(cx.listener({
                                            let name = pkg.name.clone();
                                            move |this, _, _, cx| {
                                                this.install(name.clone(), cx);
                                            }
                                        })),
                                ),
                        );

                    if let Some((_, result)) = result {
                        card = card.child(match result {
                            InstallResult::Success(output) => div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(cx.theme().success)
                                        .child("Installation complete"),
                                )
                                .child(
                                    div()
                                        .max_h(px(80.))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(cx.theme().background)
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .whitespace_normal()
                                        .child(output.clone()),
                                ),
                            InstallResult::Error(error) => div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(cx.theme().danger)
                                        .child("Installation failed"),
                                )
                                .child(
                                    div()
                                        .max_h(px(80.))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(cx.theme().background)
                                        .text_xs()
                                        .text_color(cx.theme().danger)
                                        .whitespace_normal()
                                        .child(error.clone()),
                                ),
                        });
                    } else if is_installing {
                        card = card.child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(crate::ui::loader::loader_with(6.0, 0x888888))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Installing..."),
                                ),
                        );
                    }

                    card
                }));

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
        }

        body
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
        cx.new(|cx| InstallExtensionWindow::new(window, cx))
    })
    .expect("failed to open the install extension window");
}

fn fetch_npm_packages() -> Result<Vec<NpmPackage>, String> {
    let response = reqwest::blocking::get(SEARCH_URL)
        .and_then(|r| r.json::<serde_json::Value>())
        .map_err(|e| e.to_string())?;

    let objects = response
        .get("objects")
        .and_then(|v| v.as_array())
        .ok_or("unexpected npm search response")?;

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

    Ok(packages)
}

fn run_npm_install(package_name: &str, cwd: &std::path::Path) -> Result<String, String> {
    let programs: &[&str] = if cfg!(windows) {
        &["npm.cmd", "npm"]
    } else {
        &["npm"]
    };

    let mut last_error = String::new();
    for program in programs {
        let mut cmd = std::process::Command::new(program);
        cmd.args(["install", package_name])
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }

        match cmd.output() {
            Ok(output) => {
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
                    return Ok(text);
                } else {
                    last_error = format!("npm install failed: {}", text);
                }
            }
            Err(e) => {
                last_error = format!("failed to run {}: {}", program, e);
            }
        }
    }

    Err(last_error)
}
