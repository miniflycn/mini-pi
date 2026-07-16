use std::path::PathBuf;
use std::process::Command;

/// Return the directory that contains the application's runtime resources
/// (`assets/`, `pi-bridge/`, etc.).
///
/// In release builds this is derived from the executable path:
/// - Windows / Linux: the directory that contains the binary.
/// - macOS: `Mini Pi.app/Contents/Resources` when running inside a bundle.
///
/// During development (e.g. `cargo run`) the executable lives in
/// `target/debug` or `target/release`, so we fall back to the source root
/// defined by `CARGO_MANIFEST_DIR`.
pub fn app_root() -> PathBuf {
    if let Some(root) = app_root_from_exe() {
        return root;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn app_root_from_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?.to_path_buf();

    #[cfg(target_os = "macos")]
    {
        // macOS app bundles keep resources in Contents/Resources.
        let resources = exe_dir.parent()?.join("Resources");
        if resources.join("assets").is_dir() {
            return Some(resources);
        }
    }

    if exe_dir.join("assets").is_dir() {
        return Some(exe_dir);
    }

    None
}

/// Locate the Bun runtime bundled with the application, falling back to a
/// system-installed `bun` on `PATH`.
///
/// Release builds ship the platform-specific Bun binary next to the app
/// resources (`Mini Pi.app/Contents/Resources/bun` on macOS, or in the same
/// directory as the executable on Windows/Linux). During development the
/// executable lives under `target/`, so the bundled binary is usually absent
/// and this helper falls back to a system `bun`.
pub fn find_bun() -> Option<PathBuf> {
    let root = app_root();

    let bundled = if cfg!(windows) {
        root.join("bun.exe")
    } else {
        root.join("bun")
    };
    if bundled.exists() {
        return Some(bundled);
    }

    // Development fallback: a system-installed bun.
    if Command::new("bun").arg("--version").output().is_ok() {
        return Some(PathBuf::from("bun"));
    }

    None
}
