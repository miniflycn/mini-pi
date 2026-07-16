use std::path::{Path, PathBuf};
use std::process::Command;

use reqwest::blocking;

/// Environment variable name used to pass a Cloudflare API/bearer token to the
/// cloudflared child process.
pub const CLOUDFLARE_API_TOKEN_VAR: &str = "CLOUDFLARE_API_TOKEN";

/// Returns the directory used for application data (`~/.mini-pi`).
pub fn app_data_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mini-pi")
}

/// Returns the path where the bundled cloudflared binary should live.
pub fn app_data_cloudflared_path() -> PathBuf {
    app_data_dir()
        .join("bin")
        .join(if cfg!(target_os = "windows") {
            "cloudflared.exe"
        } else {
            "cloudflared"
        })
}

/// Returns the official download URL for the current platform and architecture.
/// Replace these placeholders with the exact URLs you want to distribute.
pub fn download_url() -> Result<&'static str, String> {
    let url = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-darwin-arm64.tgz"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-darwin-amd64.tgz"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-arm64"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe"
    } else {
        return Err(format!(
            "unsupported platform: {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
    };
    Ok(url)
}

/// Downloads the cloudflared binary for the current platform, saves it to the
/// app data folder, makes it executable, and returns the absolute path.
///
/// Downloads are written to a temporary file first and atomically renamed into
/// place so an interrupted download cannot leave a corrupted binary behind.
/// On Windows the downloaded executable is also unblocked to avoid SmartScreen
/// warnings and its PE header is sanity-checked before installation.
pub fn download_and_install() -> Result<PathBuf, String> {
    let target = app_data_cloudflared_path();
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create directory: {}", e))?;
    }

    let url = download_url()?;
    let mut response = blocking::get(url).map_err(|e| format!("download failed: {}", e))?;
    if !response.status().is_success() {
        return Err(format!(
            "download failed: HTTP {} from {}",
            response.status(),
            url
        ));
    }

    #[cfg(target_os = "macos")]
    {
        // macOS releases are .tgz archives; stream to a temp file and extract.
        let tgz_path = temp_path_in_same_dir(&target, "tgz");
        {
            let mut temp_file = std::fs::File::create(&tgz_path)
                .map_err(|e| format!("failed to create temp file: {}", e))?;
            response
                .copy_to(&mut temp_file)
                .map_err(|e| format!("download failed: {}", e))?;
        }
        let binary_bytes = extract_cloudflared_tgz(&tgz_path)?;
        let _ = std::fs::remove_file(&tgz_path);
        if binary_bytes.is_empty() {
            return Err("extracted cloudflared binary is empty".to_string());
        }
        write_atomic(&target, &binary_bytes)?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Linux/Windows releases are raw executables; stream directly to disk.
        let tmp_path = temp_path_in_same_dir(&target, "tmp");
        {
            let mut file = std::fs::File::create(&tmp_path)
                .map_err(|e| format!("failed to create file: {}", e))?;
            response
                .copy_to(&mut file)
                .map_err(|e| format!("download failed: {}", e))?;
        }

        if let Err(e) = validate_raw_binary(&tmp_path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e);
        }

        #[cfg(windows)]
        unblock_windows_file(&tmp_path);

        std::fs::rename(&tmp_path, &target)
            .map_err(|e| format!("failed to install binary: {}", e))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&target)
            .map_err(|e| format!("failed to read permissions: {}", e))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&target, perms)
            .map_err(|e| format!("failed to set permissions: {}", e))?;
    }

    Ok(target)
}

fn temp_path_in_same_dir(target: &std::path::Path, extension: &str) -> PathBuf {
    let file_name = format!(
        ".cloudflared-download-{}.{}",
        uuid::Uuid::new_v4(),
        extension
    );
    target
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(file_name)
}

#[cfg(target_os = "macos")]
fn write_atomic(target: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    let tmp_path = temp_path_in_same_dir(target, "tmp");
    std::fs::write(&tmp_path, bytes).map_err(|e| format!("failed to write binary: {}", e))?;
    std::fs::rename(&tmp_path, target).map_err(|e| format!("failed to install binary: {}", e))?;
    Ok(())
}

/// Sanity-check a freshly downloaded raw executable.
#[cfg(windows)]
fn validate_raw_binary(path: &std::path::Path) -> Result<(), String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("failed to open downloaded binary: {}", e))?;
    let mut header = [0u8; 2];
    match file.read_exact(&mut header) {
        Ok(()) if &header == b"MZ" => Ok(()),
        Ok(()) => Err("downloaded file does not look like a Windows executable".to_string()),
        Err(e) => Err(format!(
            "downloaded cloudflared binary is empty or incomplete: {}",
            e
        )),
    }
}

#[cfg(not(windows))]
fn validate_raw_binary(path: &std::path::Path) -> Result<(), String> {
    let metadata = std::fs::metadata(path)
        .map_err(|e| format!("failed to inspect downloaded binary: {}", e))?;
    if metadata.len() == 0 {
        Err("downloaded cloudflared binary is empty".to_string())
    } else {
        Ok(())
    }
}

/// Remove the Mark-of-the-Web Zone.Identifier alternate data stream so Windows
/// does not show a SmartScreen "Windows protected your PC" prompt when the
/// freshly downloaded executable is spawned.
#[cfg(windows)]
fn unblock_windows_file(path: &std::path::Path) {
    use std::os::windows::process::CommandExt;

    let _ = std::process::Command::new("powershell.exe")
        .arg("-NoProfile")
        .arg("-Command")
        .arg("Unblock-File")
        .arg("-Path")
        .arg(path.as_os_str())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output();
}

#[cfg(target_os = "macos")]
fn extract_cloudflared_tgz(tgz_path: &std::path::Path) -> Result<Vec<u8>, String> {
    let file =
        std::fs::File::open(tgz_path).map_err(|e| format!("failed to open archive: {}", e))?;
    let tar = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(tar);
    let mut entries = archive
        .entries()
        .map_err(|e| format!("failed to read archive: {}", e))?;

    while let Some(entry) = entries.next() {
        let mut entry = entry.map_err(|e| format!("failed to read archive entry: {}", e))?;
        let path = entry
            .path()
            .map_err(|e| format!("failed to read entry path: {}", e))?;
        if path.file_name().and_then(|n| n.to_str()) == Some("cloudflared") {
            let mut buf = Vec::new();
            std::io::copy(&mut entry, &mut buf)
                .map_err(|e| format!("failed to extract binary: {}", e))?;
            return Ok(buf);
        }
    }

    Err("cloudflared binary not found in downloaded archive".to_string())
}

/// Resolve the configured cloudflared command into a path we can spawn.
///
/// If the command is already a path (absolute or relative with separators) it
/// is returned as-is when the file exists. Otherwise the bare command name is
/// first resolved against `PATH`; if that fails, the bundled binary in
/// `~/.mini-pi/bin` is used as a fallback. This lets users who downloaded
/// cloudflared through the app start remote control without adding anything to
/// their PATH.
pub fn resolve_cloudflared_command(command: &str) -> Result<String, String> {
    resolve_cloudflared_command_with_bundle(command, &app_data_cloudflared_path())
}

pub fn resolve_cloudflared_command_with_bundle(
    command: &str,
    bundled: &Path,
) -> Result<String, String> {
    let path = Path::new(command);

    // A path-like command (absolute or containing directory separators) is used
    // directly after an existence check.
    if path.is_absolute() || command.contains(std::path::is_separator) {
        if path.exists() {
            return Ok(command.to_string());
        }
        return Err(format!(
            "configured cloudflared command not found: {}",
            command
        ));
    }

    // Bare command name: try PATH first so a system-installed binary is
    // preferred.
    if Command::new(command).arg("--version").output().is_ok() {
        return Ok(command.to_string());
    }

    // Fall back to the bundled app-data binary.
    if bundled.exists() {
        return Ok(bundled.to_string_lossy().to_string());
    }

    Err("cloudflared not found".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_data_cloudflared_path_uses_mini_pi_bin() {
        let path = app_data_cloudflared_path();
        let parent = path.parent().expect("path has parent");
        assert!(path.to_string_lossy().contains(".mini-pi"));
        assert_eq!(parent.file_name().expect("parent has file name"), "bin");
    }

    #[test]
    fn temp_path_in_same_dir_uses_target_parent() {
        let target = PathBuf::from("/foo/bar/cloudflared.exe");
        let tmp = temp_path_in_same_dir(&target, "tmp");
        assert_eq!(tmp.parent(), target.parent());
        let file_name = tmp.file_name().unwrap().to_string_lossy();
        assert!(file_name.starts_with(".cloudflared-download-"));
        assert!(file_name.ends_with(".tmp"));
    }

    #[cfg(windows)]
    #[test]
    fn validate_raw_binary_accepts_pe_header() {
        let dir = std::env::temp_dir().join(format!("mini-pi-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fake.exe");
        std::fs::write(&path, b"MZsomebytes").unwrap();
        assert!(validate_raw_binary(&path).is_ok());
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn validate_raw_binary_rejects_non_pe_header() {
        let dir = std::env::temp_dir().join(format!("mini-pi-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fake.txt");
        std::fs::write(&path, b"hello world").unwrap();
        assert!(validate_raw_binary(&path).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn resolve_cloudflared_command_uses_absolute_path_when_present() {
        let dir = std::env::temp_dir().join(format!("mini-pi-ctrl-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(if cfg!(windows) {
            "cloudflared.exe"
        } else {
            "cloudflared"
        });
        std::fs::write(&exe, b"fake").unwrap();

        let result = resolve_cloudflared_command_with_bundle(exe.to_string_lossy().as_ref(), &dir);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), exe.to_string_lossy());

        std::fs::remove_file(&exe).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn resolve_cloudflared_command_errors_for_missing_absolute_path() {
        let missing = std::env::temp_dir().join("definitely-missing-cloudflared-12345678.exe");
        let result = resolve_cloudflared_command_with_bundle(
            missing.to_string_lossy().as_ref(),
            &std::env::temp_dir(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn resolve_cloudflared_command_falls_back_to_bundled_binary() {
        let dir = std::env::temp_dir().join(format!("mini-pi-ctrl-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let bundled = dir.join(if cfg!(windows) {
            "cloudflared.exe"
        } else {
            "cloudflared"
        });
        std::fs::write(&bundled, b"fake").unwrap();

        let result = resolve_cloudflared_command_with_bundle("cloudflared", &bundled);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), bundled.to_string_lossy());

        std::fs::remove_file(&bundled).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn resolve_cloudflared_command_errors_when_nothing_exists() {
        let dir = std::env::temp_dir().join(format!("mini-pi-ctrl-test-{}", uuid::Uuid::new_v4()));
        let bundled = dir.join(if cfg!(windows) {
            "cloudflared.exe"
        } else {
            "cloudflared"
        });
        let result = resolve_cloudflared_command_with_bundle("cloudflared", &bundled);
        assert!(result.is_err());
    }
}
