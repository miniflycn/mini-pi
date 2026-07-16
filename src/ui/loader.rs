use gpui::{
    Animation, AnimationExt, IntoElement, ParentElement, SharedString, Styled, Transformation, div,
    percentage, prelude::*, px, rgb, svg,
};
use std::time::Duration;

/// Create a reusable animated loader / spinner.
///
/// Shows a small centered row of three dots that pulse in sequence.
/// Use it anywhere you need to indicate that something is happening
/// in the background.
///
/// # Example
/// ```rust
/// # use mini_pi::ui::loader::loader;
/// # use gpui::{div, ParentElement};
/// div().child(loader());
/// ```
pub fn loader() -> impl IntoElement {
    loader_with(8.0, 0x888888)
}

/// Create a loader with a custom dot size (in pixels) and colour (24-bit hex).
pub fn loader_with(size: f32, color: u32) -> impl IntoElement {
    let dot_size = px(size);
    let gap = px(size * 1.5);
    let color = rgb(color);

    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(gap)
        .child(animated_dot(dot_size, color, "loader-dot-1", 0.0))
        .child(animated_dot(dot_size, color, "loader-dot-2", 0.33))
        .child(animated_dot(dot_size, color, "loader-dot-3", 0.66))
}

/// Create a reusable animated spinner.
///
/// Shows a small rotating circular arc. Use it anywhere you need
/// to indicate that something is happening in the background.
///
/// # Example
/// ```rust
/// # use mini_pi::ui::loader::spinner;
/// # use gpui::{div, ParentElement};
/// div().child(spinner());
/// ```
pub fn spinner() -> impl IntoElement {
    spinner_with(14.0, 0x888888)
}

/// Create a spinner with a custom size (in pixels) and colour (24-bit hex).
pub fn spinner_with(size: f32, color: u32) -> impl IntoElement {
    svg()
        .path("icons/spinner.svg")
        .size(px(size))
        .text_color(rgb(color))
        .with_animation(
            "spinner-rotate",
            Animation::new(Duration::from_millis(800)).repeat(),
            |this, delta| this.with_transformation(Transformation::rotate(percentage(delta))),
        )
}

fn animated_dot(
    size: gpui::Pixels,
    color: gpui::Rgba,
    id: &'static str,
    phase_offset: f32,
) -> impl IntoElement {
    // Fixed-size container keeps the dot centered so surrounding layout doesn't
    // shift while the inner dot scales.
    div()
        .id(SharedString::from(format!("{}-container", id)))
        .size(size)
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .id(id)
                .size(size)
                .rounded_full()
                .bg(color)
                .with_animation(
                    id,
                    Animation::new(Duration::from_millis(1200)).repeat(),
                    move |this, progress| {
                        // Offset progress so dots pulse in sequence
                        let phased = (progress + phase_offset) % 1.0;
                        // Smooth sine wave: 0.3 -> 1.0 -> 0.3
                        let opacity = 0.3 + 0.7 * (phased * std::f32::consts::PI * 2.0).sin().abs();
                        // Dot size bounces between 60% and 100% for a stronger pulse
                        let scale = 0.6 + 0.4 * (phased * std::f32::consts::PI * 2.0).sin().abs();
                        let current_size = size * scale;
                        this.opacity(opacity).w(current_size).h(current_size)
                    },
                ),
        )
}

/// Create an inline text loader that animates through a snake/circle braille sequence.
///
/// Cycles through `⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏` every 800 ms.
/// Useful as a placeholder inside message bubbles while content is streaming.
///
/// # Example
/// ```rust
/// # use mini_pi::ui::loader::text_loader;
/// # use gpui::{div, ParentElement};
/// div().child(text_loader());
/// ```
pub fn text_loader() -> impl IntoElement {
    const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    div().id("text-loader").with_animation(
        "text-loader-anim",
        Animation::new(Duration::from_millis(800)).repeat(),
        move |this, progress| {
            let idx = (progress * FRAMES.len() as f32) as usize % FRAMES.len();
            this.child(SharedString::from(FRAMES[idx]))
        },
    )
}
