use crate::data::store::Store;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub enum AuthState {
    LoggedOut,
    LoggingIn,
    LoggedIn(SupabaseUser),
    #[allow(dead_code)]
    Error(String),
}

impl AuthState {
    pub fn user(&self) -> Option<&SupabaseUser> {
        match self {
            AuthState::LoggedIn(user) => Some(user),
            _ => None,
        }
    }

    pub fn is_logged_in(&self) -> bool {
        matches!(self, AuthState::LoggedIn(_))
    }

    pub fn initials(&self) -> String {
        match self {
            AuthState::LoggedIn(user) => {
                let email = &user.email;
                if let Some(at) = email.find('@') {
                    email[..at].to_uppercase()
                } else {
                    email.to_uppercase()
                }
                .chars()
                .take(2)
                .collect()
            }
            _ => "?".to_string(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SupabaseUser {
    pub id: String,
    pub email: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SupabaseSession {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    pub user: SupabaseUser,
}

impl SupabaseSession {
    pub fn is_expired(&self) -> bool {
        let now = chrono::Utc::now().timestamp();
        now >= self.expires_at
    }
}

fn auth_file_path() -> PathBuf {
    let dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mini-pi");
    dir.join("auth.json")
}

pub fn save_session(
    store: &Store,
    session: &SupabaseSession,
) -> Result<(), crate::data::store::StoreError> {
    let content = serde_json::to_string_pretty(session).map_err(|e| {
        crate::data::store::StoreError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    store.set_user_setting("supabase_session", &content)
}

pub fn load_session(store: &Store) -> Option<SupabaseSession> {
    // Try database first.
    if let Ok(Some(db_value)) = store.get_user_setting("supabase_session")
        && let Ok(session) = serde_json::from_str::<SupabaseSession>(&db_value)
    {
        // Clean up legacy file if it still exists.
        let legacy = auth_file_path();
        if legacy.exists() {
            let _ = std::fs::remove_file(&legacy);
        }
        return Some(session);
    }

    // One-time migration: fall back to legacy auth.json.
    let legacy = auth_file_path();
    if !legacy.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&legacy).ok()?;
    let session: SupabaseSession = serde_json::from_str(&content).ok()?;
    let _ = save_session(store, &session);
    let _ = std::fs::remove_file(&legacy);
    Some(session)
}

pub fn clear_session(store: &Store) -> Result<(), crate::data::store::StoreError> {
    store.delete_user_setting("supabase_session")?;
    let legacy = auth_file_path();
    if legacy.exists() {
        let _ = std::fs::remove_file(&legacy);
    }
    Ok(())
}

pub fn mini_pi_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mini-pi")
}

pub fn bun_cache_dir() -> PathBuf {
    let dir = mini_pi_dir().join("bun-cache");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn agent_dir() -> PathBuf {
    let dir = mini_pi_dir().join("agent");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn is_first_run() -> bool {
    !mini_pi_dir().exists()
}

pub fn pi_agent_source_dir() -> Option<PathBuf> {
    let dir = dirs::home_dir()?.join(".pi").join("agent");
    if dir.exists() { Some(dir) } else { None }
}

pub fn pi_dir_exists() -> bool {
    dirs::home_dir()
        .map(|h| h.join(".pi").exists())
        .unwrap_or(false)
}

const WHITELISTED_FILES: &[&str] = &["auth.json", "settings.json", "models.json"];

pub fn list_pi_agent_json_files() -> Vec<(String, PathBuf)> {
    let Some(source) = pi_agent_source_dir() else {
        return Vec::new();
    };
    list_pi_agent_json_files_in_dir(&source)
}

fn list_pi_agent_json_files_in_dir(dir: &PathBuf) -> Vec<(String, PathBuf)> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .to_string();
        if !WHITELISTED_FILES.contains(&name.as_str()) {
            continue;
        }
        files.push((name, path));
    }
    files
}

pub fn import_from_pi_agent() -> Result<usize, std::io::Error> {
    let files = list_pi_agent_json_files();
    if files.is_empty() {
        return Ok(0);
    }
    let target = agent_dir();
    let mut imported = 0;
    for (_rel, src) in files {
        let dst = target.join(src.file_name().unwrap_or(src.as_os_str()));
        std::fs::copy(&src, &dst)?;
        imported += 1;
    }
    Ok(imported)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn list_pi_agent_json_files_only_includes_whitelisted_files() {
        let temp = std::env::temp_dir().join(format!("mini-pi-agent-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let mut f = std::fs::File::create(temp.join("auth.json")).unwrap();
        f.write_all(b"{}").unwrap();

        let mut f = std::fs::File::create(temp.join("model.json")).unwrap();
        f.write_all(b"{}").unwrap();

        let mut f = std::fs::File::create(temp.join("providers.json")).unwrap();
        f.write_all(b"{}").unwrap();

        let files = list_pi_agent_json_files_in_dir(&temp);
        let _ = std::fs::remove_dir_all(&temp);

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "auth.json");
    }
}
