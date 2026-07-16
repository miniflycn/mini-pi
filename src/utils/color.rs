use std::hash::{Hash, Hasher};

/// Palette used to color workspace tags and buttons. Each workspace gets a
/// stable color derived from a hash of its name.
static WORKSPACE_COLORS: std::sync::LazyLock<[gpui::Hsla; 16]> = std::sync::LazyLock::new(|| {
    [
        gpui::rgb(0xef4444).into(), // red-500
        gpui::rgb(0xf97316).into(), // orange-500
        gpui::rgb(0xf59e0b).into(), // amber-500
        gpui::rgb(0x84cc16).into(), // lime-500
        gpui::rgb(0x22c55e).into(), // green-500
        gpui::rgb(0x10b981).into(), // emerald-500
        gpui::rgb(0x14b8a6).into(), // teal-500
        gpui::rgb(0x06b6d4).into(), // cyan-500
        gpui::rgb(0x0ea5e9).into(), // sky-500
        gpui::rgb(0x3b82f6).into(), // blue-500
        gpui::rgb(0x6366f1).into(), // indigo-500
        gpui::rgb(0x8b5cf6).into(), // violet-500
        gpui::rgb(0xa855f7).into(), // purple-500
        gpui::rgb(0xd946ef).into(), // fuchsia-500
        gpui::rgb(0xec4899).into(), // pink-500
        gpui::rgb(0xf43f5e).into(), // rose-500
    ]
});

/// Returns a stable color for a workspace name.
pub fn workspace_color(name: &str) -> gpui::Hsla {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hasher);
    let hash = hasher.finish();
    let colors = &*WORKSPACE_COLORS;
    colors[hash as usize % colors.len()]
}

/// Returns a foreground color (black or white) that contrasts with the given
/// background color.
pub fn workspace_foreground(background: gpui::Hsla) -> gpui::Hsla {
    let rgba = background.to_rgb();
    // Perceived luminance; use black text on light backgrounds and white on
    // dark ones.
    let luminance = 0.299 * rgba.r + 0.587 * rgba.g + 0.114 * rgba.b;
    if luminance > 0.55 {
        gpui::black()
    } else {
        gpui::white()
    }
}
