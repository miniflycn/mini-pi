use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc::Receiver};
use std::thread;
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::remote::cloudflared::CLOUDFLARE_API_TOKEN_VAR;

const TRY_CLOUDFLARE_SUFFIX: &str = ".trycloudflare.com";
pub const URL_TIMEOUT: Duration = Duration::from_secs(15);

/// Outcome produced by the tunnel process.
#[derive(Clone, Debug)]
pub enum TunnelOutcome {
    /// A public URL was discovered.
    Url(String),
    /// The process exited or produced an error before a URL was known.
    Error(String),
    /// A log line was emitted by the tunnel process.
    Log { level: String, message: String },
    /// The tunnel successfully registered a connection with the edge.
    Connected,
}

pub struct TunnelHandle {
    child: Arc<Mutex<Child>>,
    shutdown: Arc<AtomicBool>,
}

impl Drop for TunnelHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // The monitor thread only holds the lock briefly while polling
        // try_wait(), so this cannot deadlock.
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}

/// Start cloudflared and return a handle plus a channel that receives the public URL
/// (or an error) as soon as it is known.
///
/// # Named tunnels
/// For a named tunnel (`tunnel_token` is Some), cloudflared does not print the public
/// URL on stdout. The caller must also provide `hostname`. If `hostname` is missing,
/// this function returns an error immediately.
///
/// # Command semantics
/// `command` must be a single executable name or absolute path. It is passed directly
/// to `std::process::Command::new`; shell-style arguments or spaces are not parsed.
pub fn start(
    command: &str,
    tunnel_token: Option<&str>,
    hostname: Option<&str>,
    bearer_token: Option<&str>,
    local_port: u16,
) -> Result<(TunnelHandle, Receiver<TunnelOutcome>), String> {
    if tunnel_token.is_some() && hostname.is_none() {
        return Err(
            "named tunnel requires a hostname (set remote.cloudflared.hostname)".to_string(),
        );
    }

    let (url_tx, url_rx) = std::sync::mpsc::channel();

    // On macOS wrap cloudflared with caffeinate so the tunnel keeps the system
    // awake while remote control is enabled. -i prevents idle sleep, -s prevents
    // system sleep.
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut cmd = Command::new("caffeinate");
        cmd.arg("-is").arg(command);
        cmd
    };
    #[cfg(not(target_os = "macos"))]
    let mut cmd = Command::new(command);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(token) = bearer_token {
        cmd.env(CLOUDFLARE_API_TOKEN_VAR, token);
    }

    match tunnel_token {
        Some(token) => {
            cmd.arg("tunnel").arg("run").arg("--token").arg(token);
        }
        None => {
            cmd.arg("tunnel")
                .arg("--url")
                .arg(format!("http://127.0.0.1:{}", local_port));
        }
    }

    #[cfg(windows)]
    {
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let msg = if e.kind() == std::io::ErrorKind::NotFound {
                "cloudflared not found. Install it with `brew install cloudflared` (macOS) or download it from https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/".to_string()
            } else {
                format!("failed to spawn cloudflared: {}", e)
            };
            return Err(msg);
        }
    };

    let stdout = child
        .stdout
        .take()
        .ok_or("failed to capture cloudflared stdout")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("failed to capture cloudflared stderr")?;

    let child = Arc::new(Mutex::new(child));
    let shutdown = Arc::new(AtomicBool::new(false));
    let url_found = Arc::new(AtomicBool::new(false));
    let error_sent = Arc::new(AtomicBool::new(false));

    let child_for_monitor = child.clone();
    let shutdown_for_monitor = shutdown.clone();
    let error_sent_for_monitor = error_sent.clone();
    let url_tx_for_monitor = url_tx.clone();
    thread::spawn(move || {
        // Poll with try_wait() instead of blocking on wait() so we never hold
        // the child lock across a blocking syscall. This lets TunnelHandle::drop
        // acquire the lock and kill the process without deadlocking.
        loop {
            let done = {
                let mut child = child_for_monitor.lock().unwrap();
                match child.try_wait() {
                    Ok(Some(status)) => {
                        if !shutdown_for_monitor.load(Ordering::SeqCst)
                            && !error_sent_for_monitor.swap(true, Ordering::SeqCst)
                        {
                            let _ = url_tx_for_monitor.send(TunnelOutcome::Error(format!(
                                "cloudflared exited unexpectedly: {:?}",
                                status
                            )));
                        }
                        true
                    }
                    Ok(None) => false,
                    Err(e) => {
                        if !shutdown_for_monitor.load(Ordering::SeqCst)
                            && !error_sent_for_monitor.swap(true, Ordering::SeqCst)
                        {
                            let _ = url_tx_for_monitor.send(TunnelOutcome::Error(format!(
                                "failed to wait for cloudflared: {}",
                                e
                            )));
                        }
                        true
                    }
                }
            };
            if done {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    });

    let url_tx_for_stdout = url_tx.clone();
    let url_found_for_stdout = url_found.clone();
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            if let Ok(text) = line {
                eprintln!("[cloudflared] {}", text);
                if let Some(url) = extract_quick_url(&text) {
                    url_found_for_stdout.store(true, Ordering::SeqCst);
                    let _ = url_tx_for_stdout.send(TunnelOutcome::Url(url));
                } else if let Some((level, message)) = parse_cloudflared_log_line(&text) {
                    let is_connected = is_connected_log(&level, &message);
                    if is_surface_log(&level, &message) {
                        let _ = url_tx_for_stdout.send(TunnelOutcome::Log {
                            level: level.clone(),
                            message: message.clone(),
                        });
                    }
                    if is_connected {
                        let _ = url_tx_for_stdout.send(TunnelOutcome::Connected);
                    }
                }
            }
        }
    });

    let url_tx_for_stderr = url_tx.clone();
    let url_found_for_stderr = url_found.clone();
    thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            if let Ok(text) = line {
                eprintln!("[cloudflared] {}", text);
                if let Some(url) = extract_quick_url(&text) {
                    url_found_for_stderr.store(true, Ordering::SeqCst);
                    let _ = url_tx_for_stderr.send(TunnelOutcome::Url(url));
                } else if let Some((level, message)) = parse_cloudflared_log_line(&text) {
                    let is_connected = is_connected_log(&level, &message);
                    if is_surface_log(&level, &message) {
                        let _ = url_tx_for_stderr.send(TunnelOutcome::Log {
                            level: level.clone(),
                            message: message.clone(),
                        });
                    }
                    if is_connected {
                        let _ = url_tx_for_stderr.send(TunnelOutcome::Connected);
                    }
                }
            }
        }
    });

    // For named tunnels the hostname is the known public URL.
    if let (Some(_), Some(host)) = (tunnel_token, hostname) {
        url_found.store(true, Ordering::SeqCst);
        let _ = url_tx.send(TunnelOutcome::Url(host.to_string()));
    }

    Ok((TunnelHandle { child, shutdown }, url_rx))
}

/// Wait up to `URL_TIMEOUT` for a tunnel outcome. Returns `None` on timeout.
#[allow(dead_code)]
pub fn wait_for_outcome(rx: &Receiver<TunnelOutcome>) -> Option<TunnelOutcome> {
    rx.recv_timeout(URL_TIMEOUT).ok()
}

fn extract_quick_url(text: &str) -> Option<String> {
    // Quick tunnels print something like: https://foo.trycloudflare.com
    // Strip any ANSI escape sequences cloudflared may embed in the output.
    let cleaned = strip_ansi_codes(text);
    let start = cleaned.find("https://")?;
    let rest = &cleaned[start..];
    // Find the end of the URL: whitespace, pipe, or end of string.
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '|')
        .unwrap_or(rest.len());
    let url = &rest[..end];
    if url.ends_with(TRY_CLOUDFLARE_SUFFIX) {
        Some(url.to_string())
    } else {
        None
    }
}

/// Parse a cloudflared log line into `(level, message)`.
///
/// Expected format: `2026-06-21T06:12:03Z ERR some message key=value ...`
fn parse_cloudflared_log_line(text: &str) -> Option<(String, String)> {
    let cleaned = strip_ansi_codes(text);
    let mut parts = cleaned.split_whitespace();
    let _timestamp = parts.next()?;
    let level = parts.next()?;
    if !matches!(level, "ERR" | "WRN" | "INF" | "DBG") {
        return None;
    }
    let message = parts.collect::<Vec<_>>().join(" ");
    if message.is_empty() {
        return None;
    }
    Some((level.to_string(), message))
}

/// Returns true if a log line should be surfaced in the UI.
fn is_surface_log(level: &str, message: &str) -> bool {
    matches!(level, "ERR" | "WRN")
        || (level == "INF" && message.to_lowercase().contains("retry"))
        || is_connected_log(level, message)
}

/// Returns true if the log line signals a successful tunnel registration.
fn is_connected_log(level: &str, message: &str) -> bool {
    level == "INF" && message.starts_with("Registered tunnel connection")
}

/// Remove ANSI CSI escape sequences (e.g. color/bold codes) from a string.
fn strip_ansi_codes(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // CSI sequence: ESC [ ... final_byte
            if chars.next() == Some('[') {
                while let Some(c) = chars.next() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            result.push(ch);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_quick_url_finds_trycloudflare() {
        let line = "2026-01-01T00:00:00Z INF |  https://foo-bar.trycloudflare.com  |";
        assert_eq!(
            extract_quick_url(line),
            Some("https://foo-bar.trycloudflare.com".to_string())
        );
    }

    #[test]
    fn extract_quick_url_ignores_other_hosts() {
        assert_eq!(extract_quick_url("https://example.com"), None);
    }

    #[test]
    fn extract_quick_url_ignores_no_https() {
        assert_eq!(extract_quick_url("foo-bar.trycloudflare.com"), None);
    }

    #[test]
    fn extract_quick_url_handles_ansi_codes() {
        let line = "\x1b[36m\x1b[1m2026-01-01T00:00:00Z INF |  https://foo-bar.trycloudflare.com  |\x1b[0m";
        assert_eq!(
            extract_quick_url(line),
            Some("https://foo-bar.trycloudflare.com".to_string())
        );
    }

    #[test]
    fn extract_quick_url_handles_box_drawing_padding() {
        let line = "2026-06-17T14:40:42Z INF |  https://misc-mounting-hopkins-value.trycloudflare.com                                     |";
        assert_eq!(
            extract_quick_url(line),
            Some("https://misc-mounting-hopkins-value.trycloudflare.com".to_string())
        );
    }

    #[test]
    fn parse_log_line_extracts_err_and_message() {
        let line = "2026-06-21T06:12:03Z ERR Failed to dial a quic connection error=\"timeout\" connIndex=0";
        assert_eq!(
            parse_cloudflared_log_line(line),
            Some((
                "ERR".to_string(),
                "Failed to dial a quic connection error=\"timeout\" connIndex=0".to_string()
            ))
        );
    }

    #[test]
    fn parse_log_line_ignores_unexpected_levels() {
        let line = "2026-06-21T06:12:03Z FOO some message";
        assert_eq!(parse_cloudflared_log_line(line), None);
    }

    #[test]
    fn parse_log_line_handles_ansi_codes() {
        let line = "\x1b[36m\x1b[1m2026-06-21T06:12:03Z ERR Failed to dial\x1b[0m";
        assert_eq!(
            parse_cloudflared_log_line(line),
            Some(("ERR".to_string(), "Failed to dial".to_string()))
        );
    }

    #[test]
    fn is_surface_log_accepts_err_and_retry_info() {
        assert!(is_surface_log("ERR", "Failed to dial"));
        assert!(is_surface_log("WRN", "Something warning"));
        assert!(is_surface_log("INF", "Retrying connection in up to 4s"));
        assert!(!is_surface_log(
            "INF",
            "Tunnel connection curve preferences"
        ));
        assert!(!is_surface_log("DBG", "Debug info"));
    }

    #[test]
    fn is_connected_log_recognizes_registration() {
        assert!(is_connected_log(
            "INF",
            "Registered tunnel connection connIndex=0 connection=..."
        ));
        assert!(!is_connected_log("INF", "Retrying connection in up to 4s"));
        assert!(!is_connected_log("ERR", "Registered tunnel connection"));
    }
}
