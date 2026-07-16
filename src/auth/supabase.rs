use crate::auth::state::{SupabaseSession, SupabaseUser};
use reqwest::blocking::Client;
use serde::Deserialize;
use std::sync::OnceLock;

const SUPABASE_URL: &str = "https://xgazvyjwnjwablelrrsc.supabase.co";

const SUPABASE_ANON_KEY: &str = "sb_publishable_wbwXXEx1TFLEz7zKTFHkOQ_HQaHIwAF";

fn anon_key() -> &'static str {
    SUPABASE_ANON_KEY
}

/// Reuse a single process-wide `reqwest::blocking::Client` so the connection
/// pool and TLS state are shared across calls. Building a fresh client per
/// request is expensive and was the previous behavior.
fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .pool_idle_timeout(std::time::Duration::from_secs(30))
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_else(|_| Client::new())
    })
}

/// Read the body as text and turn non-2xx responses into a typed error.
fn ensure_success(
    resp: reqwest::blocking::Response,
) -> Result<reqwest::blocking::Response, SupabaseAuthError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let status_code = status.as_u16();
    let body = resp.text().unwrap_or_default();
    let msg = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| {
            v.get("message")
                .or(v.get("error_description"))
                .or(v.get("msg"))
                .or(v.get("error"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| v.as_str().map(|s| s.to_string()))
        })
        .unwrap_or_else(|| body.clone());
    Err(SupabaseAuthError::Api {
        msg,
        status: status_code,
    })
}

#[derive(Debug)]
pub enum SupabaseAuthError {
    Http(reqwest::Error),
    Api { msg: String, status: u16 },
    Json(serde_json::Error),
    Io(std::io::Error),
}

impl std::fmt::Display for SupabaseAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SupabaseAuthError::Http(e) => write!(f, "HTTP error: {}", e),
            SupabaseAuthError::Api { msg, status } => {
                write!(f, "API error ({}): {}", status, msg)
            }
            SupabaseAuthError::Json(e) => write!(f, "JSON error: {}", e),
            SupabaseAuthError::Io(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl From<reqwest::Error> for SupabaseAuthError {
    fn from(e: reqwest::Error) -> Self {
        SupabaseAuthError::Http(e)
    }
}

impl From<serde_json::Error> for SupabaseAuthError {
    fn from(e: serde_json::Error) -> Self {
        SupabaseAuthError::Json(e)
    }
}

impl From<std::io::Error> for SupabaseAuthError {
    fn from(e: std::io::Error) -> Self {
        SupabaseAuthError::Io(e)
    }
}

#[derive(Deserialize)]
struct AuthResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    user: Option<AuthUserResponse>,
}

#[derive(Deserialize)]
struct AuthUserResponse {
    id: String,
    email: String,
    created_at: String,
}

pub fn signup(email: &str, password: &str) -> Result<SupabaseSession, SupabaseAuthError> {
    let resp = client()
        .post(format!("{}/auth/v1/signup", SUPABASE_URL))
        .header("apikey", anon_key())
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "email": email,
            "password": password,
        }))
        .send()?;

    let status = resp.status().as_u16();
    let body = resp.text().map_err(SupabaseAuthError::Http)?;

    if status >= 400 {
        let msg = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                v.get("msg")
                    .or(v.get("error_description"))
                    .or(v.get("error"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| v.as_str().map(|s| s.to_string()))
            })
            .unwrap_or_else(|| format!("Signup failed (status {})", status));
        return Err(SupabaseAuthError::Api { msg, status });
    }

    let auth_resp: AuthResponse = serde_json::from_str(&body)?;
    let expires_in = auth_resp.expires_in.unwrap_or(3600);
    let expires_at = chrono::Utc::now().timestamp() + expires_in;

    match (auth_resp.access_token, auth_resp.user) {
        (Some(access_token), Some(user)) => Ok(SupabaseSession {
            access_token,
            refresh_token: auth_resp.refresh_token.unwrap_or_default(),
            expires_at,
            user: SupabaseUser {
                id: user.id,
                email: user.email,
                created_at: user.created_at,
            },
        }),
        _ => Err(SupabaseAuthError::Api {
            msg: "Please check your email to confirm your account, then sign in.".to_string(),
            status: 200,
        }),
    }
}

pub fn login(email: &str, password: &str) -> Result<SupabaseSession, SupabaseAuthError> {
    let resp = client()
        .post(format!(
            "{}/auth/v1/token?grant_type=password",
            SUPABASE_URL
        ))
        .header("apikey", anon_key())
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "email": email,
            "password": password,
        }))
        .send()?;

    let status = resp.status().as_u16();
    let body = resp.text().map_err(SupabaseAuthError::Http)?;

    if status >= 400 {
        let msg = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                v.get("error_description")
                    .or(v.get("msg"))
                    .or(v.get("error"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| v.as_str().map(|s| s.to_string()))
            })
            .unwrap_or_else(|| format!("Login failed (status {})", status));
        return Err(SupabaseAuthError::Api { msg, status });
    }

    let auth_resp: AuthResponse = serde_json::from_str(&body)?;
    let expires_in = auth_resp.expires_in.unwrap_or(3600);
    let expires_at = chrono::Utc::now().timestamp() + expires_in;

    let user = auth_resp.user.unwrap();
    Ok(SupabaseSession {
        access_token: auth_resp.access_token.unwrap(),
        refresh_token: auth_resp.refresh_token.unwrap_or_default(),
        expires_at,
        user: SupabaseUser {
            id: user.id,
            email: user.email,
            created_at: user.created_at,
        },
    })
}

pub fn refresh_session(refresh_token: &str) -> Result<SupabaseSession, SupabaseAuthError> {
    let resp = client()
        .post(format!(
            "{}/auth/v1/token?grant_type=refresh_token",
            SUPABASE_URL
        ))
        .header("apikey", anon_key())
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "refresh_token": refresh_token,
        }))
        .send()?;

    let status = resp.status().as_u16();
    let body = resp.text().map_err(SupabaseAuthError::Http)?;

    if status >= 400 {
        let msg = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                v.get("error_description")
                    .or(v.get("msg"))
                    .or(v.get("error"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| v.as_str().map(|s| s.to_string()))
            })
            .unwrap_or_else(|| format!("Token refresh failed (status {})", status));
        return Err(SupabaseAuthError::Api { msg, status });
    }

    let auth_resp: AuthResponse = serde_json::from_str(&body)?;
    let expires_in = auth_resp.expires_in.unwrap_or(3600);
    let expires_at = chrono::Utc::now().timestamp() + expires_in;

    let user = auth_resp.user.unwrap();
    Ok(SupabaseSession {
        access_token: auth_resp.access_token.unwrap(),
        refresh_token: auth_resp.refresh_token.unwrap_or_default(),
        expires_at,
        user: SupabaseUser {
            id: user.id,
            email: user.email,
            created_at: user.created_at,
        },
    })
}

pub fn get_user(access_token: &str) -> Result<SupabaseUser, SupabaseAuthError> {
    let resp = client()
        .get(format!("{}/auth/v1/user", SUPABASE_URL))
        .header("apikey", anon_key())
        .header("Authorization", format!("Bearer {}", access_token))
        .send()?;

    let status = resp.status().as_u16();
    let body = resp.text().map_err(SupabaseAuthError::Http)?;

    if status >= 400 {
        let msg = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .or(v.get("msg").and_then(|v| v.as_str().map(|s| s.to_string())))
            })
            .unwrap_or_else(|| format!("Get user failed (status {})", status));
        return Err(SupabaseAuthError::Api { msg, status });
    }

    let user_resp: AuthUserResponse = serde_json::from_str(&body)?;

    Ok(SupabaseUser {
        id: user_resp.id,
        email: user_resp.email,
        created_at: user_resp.created_at,
    })
}

pub fn logout(access_token: &str) -> Result<(), SupabaseAuthError> {
    let resp = client()
        .post(format!("{}/auth/v1/logout", SUPABASE_URL))
        .header("apikey", anon_key())
        .header("Authorization", format!("Bearer {}", access_token))
        .send()?;
    ensure_success(resp)?;
    Ok(())
}

pub fn upload_file(
    access_token: &str,
    user_id: &str,
    file_path: &str,
    content_type: &str,
    data: &[u8],
) -> Result<(), SupabaseAuthError> {
    let bucket_path = format!("{}/{}", user_id, file_path);

    let body = data.to_vec();
    let url = format!("{}/storage/v1/object/pi-sync/{}", SUPABASE_URL, bucket_path);

    let resp = client()
        .post(url)
        .header("apikey", anon_key())
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", content_type)
        .header("x-upsert", "true")
        .body(body)
        .send()?;
    ensure_success(resp)?;
    Ok(())
}

/// List every file stored under a user's folder in the `pi-sync` bucket.
///
/// Supabase Storage's `list` endpoint is **not** recursive: listing a prefix
/// returns the files directly under it plus placeholder entries for immediate
/// subfolders (these have a `null` id and no trailing slash). To enumerate
/// nested files (e.g. `extensions/<name>.json`) we must descend into each
/// subfolder ourselves.
///
/// The returned `StorageFile::name` is rewritten to the path relative to the
/// user folder (e.g. `auth.json`, `extensions/foo.json`) so callers get a
/// consistent, fully-qualified relative path regardless of nesting depth.
pub fn list_files(
    access_token: &str,
    user_id: &str,
) -> Result<Vec<StorageFile>, SupabaseAuthError> {
    let mut files = Vec::new();
    list_files_recursive(access_token, user_id, "", &mut files)?;
    Ok(files)
}

/// List one folder level under `{user_id}/{sub_prefix}` and recurse into any
/// subfolders found. `sub_prefix` is relative to the user folder and is either
/// empty or ends with `/` (e.g. "" or "extensions/").
fn list_files_recursive(
    access_token: &str,
    user_id: &str,
    sub_prefix: &str,
    out: &mut Vec<StorageFile>,
) -> Result<(), SupabaseAuthError> {
    let url = format!("{}/storage/v1/object/list/pi-sync", SUPABASE_URL);
    let body_json = serde_json::json!({
        "prefix": format!("{}/{}", user_id, sub_prefix),
        "limit": 1000,
    });

    let resp = client()
        .post(url)
        .header("apikey", anon_key())
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .json(&body_json)
        .send()?;

    let resp = ensure_success(resp)?;
    let body = resp.text().map_err(SupabaseAuthError::Http)?;
    let entries: Vec<StorageFile> =
        serde_json::from_str(&body).map_err(|e| SupabaseAuthError::Api {
            msg: format!("failed to parse file list: {} (body: {})", e, body),
            status: 200,
        })?;

    for entry in entries {
        let Some(name) = entry.name.clone() else {
            continue;
        };
        if name.is_empty() {
            continue;
        }

        if entry.id.is_none() {
            // Folder placeholder: descend into it.
            let next_prefix = format!("{}{}/", sub_prefix, name);
            list_files_recursive(access_token, user_id, &next_prefix, out)?;
            continue;
        }

        // Skip the zero-byte marker Supabase creates for folders made via the
        // dashboard; it is not a real synced file.
        if name == ".emptyFolderPlaceholder" {
            continue;
        }

        // Rewrite the name to the full path relative to the user folder.
        let mut file = entry;
        file.name = Some(format!("{}{}", sub_prefix, name));
        out.push(file);
    }

    Ok(())
}

pub fn download_file(
    access_token: &str,
    user_id: &str,
    file_path: &str,
) -> Result<Vec<u8>, SupabaseAuthError> {
    let bucket_path = format!("{}/{}", user_id, file_path);
    let url = format!("{}/storage/v1/object/pi-sync/{}", SUPABASE_URL, bucket_path);

    let resp = client()
        .get(url)
        .header("apikey", anon_key())
        .header("Authorization", format!("Bearer {}", access_token))
        .send()?;

    let resp = ensure_success(resp)?;
    let data = resp.bytes().map_err(SupabaseAuthError::Http)?;
    Ok(data.to_vec())
}

pub fn delete_file(
    access_token: &str,
    user_id: &str,
    file_path: &str,
) -> Result<(), SupabaseAuthError> {
    let bucket_path = format!("{}/{}", user_id, file_path);
    let url = format!("{}/storage/v1/object/pi-sync/{}", SUPABASE_URL, bucket_path);

    let resp = client()
        .delete(url)
        .header("apikey", anon_key())
        .header("Authorization", format!("Bearer {}", access_token))
        .send()?;
    ensure_success(resp)?;
    Ok(())
}

#[derive(Clone, Debug, Deserialize)]
pub struct StorageFile {
    pub name: Option<String>,
    pub id: Option<String>,
    pub updated_at: Option<String>,
    pub created_at: Option<String>,
    pub last_accessed_at: Option<String>,
    pub metadata: Option<serde_json::Value>,
}
