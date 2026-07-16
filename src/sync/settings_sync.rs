use crate::auth::state;
use crate::auth::supabase;
use crate::data::store::{Store, StoreError};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

fn with_sync_lock<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let lock_path = state::agent_dir().join(".sync.lock");
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|e| format!("failed to open sync lock: {}", e))?;
    lock_file
        .lock_exclusive()
        .map_err(|e| format!("failed to acquire sync lock: {}", e))?;
    let result = f();
    let _ = lock_file.unlock();
    result
}

const SYNC_META_KEY: &str = "sync_meta";

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SyncMeta {
    pub files: HashMap<String, FileSyncInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileSyncInfo {
    pub hash: String,
    pub last_synced: String,
}

pub fn load_sync_meta(store: &Store) -> SyncMeta {
    match store.get_user_setting(SYNC_META_KEY) {
        Ok(Some(value)) => serde_json::from_str(&value).unwrap_or_default(),
        Ok(None) | Err(_) => SyncMeta::default(),
    }
}

pub fn save_sync_meta(store: &Store, meta: &SyncMeta) -> Result<(), StoreError> {
    let value = serde_json::to_string_pretty(meta).map_err(|e| {
        StoreError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("failed to serialize sync meta: {}", e),
        ))
    })?;
    store.set_user_setting(SYNC_META_KEY, &value)
}

fn file_hash(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    format!("{:x}", result)
}

fn scan_agent_files() -> Vec<(String, PathBuf)> {
    let agent_dir = state::agent_dir();
    let mut files = Vec::new();
    scan_dir_recursive(&agent_dir, &agent_dir, &mut files);
    for (relative, _) in &mut files {
        // Supabase Storage keys require '/' separators, even on Windows.
        *relative = relative.replace('\\', "/");
    }
    files
        .into_iter()
        .filter(|(relative, _)| relative != ".sync.lock" && is_sync_whitelisted(relative))
        .collect()
}

fn scan_dir_recursive(base: &PathBuf, current: &PathBuf, files: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_dir_recursive(base, &path, files);
        } else {
            let relative = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            files.push((relative, path));
        }
    }
}

fn content_type_for(path: &str) -> &str {
    if path.ends_with(".json") {
        "application/json"
    } else if path.ends_with(".yaml") || path.ends_with(".yml") {
        "application/yaml"
    } else if path.ends_with(".toml") {
        "application/toml"
    } else if path.ends_with(".md") {
        "text/markdown"
    } else if path.ends_with(".txt") {
        "text/plain"
    } else {
        "application/octet-stream"
    }
}

fn is_sync_whitelisted(relative_path: &str) -> bool {
    relative_path == "auth.json"
        || relative_path == "models.json"
        || relative_path == "settings.json"
        || relative_path.starts_with("extensions/")
}

fn map_sync_error(e: supabase::SupabaseAuthError) -> String {
    let text = e.to_string();
    if text.to_lowercase().contains("not_found") {
        format!(
            "{} (hint: make sure the Supabase Storage bucket 'pi-sync' exists and has RLS policies allowing authenticated users to read/write.)",
            text
        )
    } else {
        text
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SyncStatus {
    Idle,
    Syncing,
    Synced,
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SyncOp {
    Upload { path: String },
    Download { path: String },
    DeleteRemote { path: String },
    DeleteLocal { path: String },
}

/// Pure three-way sync decision logic.
///
/// `local_hashes` maps relative paths to their current local SHA-256 hashes.
/// `remote_set` contains the relative paths that currently exist on Supabase.
/// `meta` records the files that were present on both sides at the last sync.
///
/// Returns the list of operations required to bring both sides into a
/// consistent state. Deletions are only propagated when the missing side
/// previously synced the file (i.e. it is in `meta`), which prevents a device
/// from wiping files that were created on another device before the first pull.
fn compute_sync_plan(
    local_hashes: HashMap<String, String>,
    remote_set: HashSet<String>,
    meta: &SyncMeta,
) -> Vec<SyncOp> {
    let mut ops = Vec::new();
    let local_set: HashSet<String> = local_hashes.keys().cloned().collect();
    let meta_set: HashSet<String> = meta.files.keys().cloned().collect();

    // Files present on both sides: upload when the local copy changed.
    for path in local_set.intersection(&remote_set) {
        let local_hash = local_hashes.get(path).expect("path came from local_hashes");
        let needs_upload = match meta.files.get(path) {
            Some(info) => info.hash != *local_hash,
            None => true,
        };
        if needs_upload {
            ops.push(SyncOp::Upload { path: path.clone() });
        }
    }

    // Remote-only files.
    for path in remote_set.difference(&local_set) {
        if meta_set.contains(path) {
            // Previously synced, then deleted locally.
            ops.push(SyncOp::DeleteRemote { path: path.clone() });
        } else {
            // New file created on another device.
            ops.push(SyncOp::Download { path: path.clone() });
        }
    }

    // Local-only files.
    for path in local_set.difference(&remote_set) {
        if meta_set.contains(path) {
            // Previously synced, then deleted remotely.
            ops.push(SyncOp::DeleteLocal { path: path.clone() });
        } else {
            // New file created locally.
            ops.push(SyncOp::Upload { path: path.clone() });
        }
    }

    ops
}

pub fn pull_from_remote(
    access_token: &str,
    user_id: &str,
    initial_meta: SyncMeta,
) -> Result<SyncMeta, String> {
    with_sync_lock(|| {
        let agent_dir = state::agent_dir();
        let remote_files = supabase::list_files(access_token, user_id).map_err(map_sync_error)?;

        let mut meta = initial_meta;
        let mut remote_set = HashSet::new();

        // Download every remote file, making local state match remote.
        for file in &remote_files {
            let Some(name) = &file.name else {
                continue;
            };
            if name.ends_with('/') || file.id.is_none() {
                continue;
            }

            let relative_path = name
                .strip_prefix(&format!("{}/", user_id))
                .unwrap_or(name)
                .to_string();

            if relative_path == ".sync.lock" || !is_sync_whitelisted(&relative_path) {
                continue;
            }

            let data = supabase::download_file(access_token, user_id, &relative_path)
                .map_err(map_sync_error)?;

            let local_path = agent_dir.join(&relative_path);
            if let Some(parent) = local_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&local_path, &data).map_err(|e| e.to_string())?;

            let hash = file_hash(&data);
            let now = chrono::Utc::now().to_rfc3339();
            meta.files.insert(
                relative_path.clone(),
                FileSyncInfo {
                    hash,
                    last_synced: now,
                },
            );
            remote_set.insert(relative_path);
        }

        // Delete local files that were synced but are no longer on the remote.
        let local_files = scan_agent_files();
        let local_set: HashSet<String> = local_files.into_iter().map(|(p, _)| p).collect();
        for path in local_set.difference(&remote_set) {
            if meta.files.contains_key(path) {
                let local_path = agent_dir.join(path);
                let _ = std::fs::remove_file(&local_path);
                meta.files.remove(path);
            }
        }

        Ok(meta)
    })
}

pub fn push_to_remote(
    access_token: &str,
    user_id: &str,
    initial_meta: SyncMeta,
) -> Result<SyncMeta, String> {
    with_sync_lock(|| {
        let local_files = scan_agent_files();
        let local_map: HashMap<String, PathBuf> = local_files.into_iter().collect();
        let local_set: HashSet<String> = local_map.keys().cloned().collect();
        let mut meta = initial_meta;

        // Delete remote files that were synced but are no longer local.
        let remote_files = supabase::list_files(access_token, user_id).map_err(map_sync_error)?;
        let remote_set: HashSet<String> = remote_files
            .iter()
            .filter_map(|f| {
                if f.id.is_none() {
                    return None;
                }
                f.name.as_ref().and_then(|name| {
                    let relative = name
                        .strip_prefix(&format!("{}/", user_id))
                        .unwrap_or(name)
                        .to_string();
                    if relative == ".sync.lock" || !is_sync_whitelisted(&relative) {
                        None
                    } else {
                        Some(relative)
                    }
                })
            })
            .collect();

        for path in remote_set.difference(&local_set) {
            if meta.files.contains_key(path) {
                supabase::delete_file(access_token, user_id, path).map_err(map_sync_error)?;
                meta.files.remove(path);
            }
        }

        // Upload every local file, making remote state match local.
        for (relative_path, full_path) in &local_map {
            let data = std::fs::read(full_path).map_err(|e| e.to_string())?;
            let content_type = content_type_for(relative_path);

            supabase::upload_file(access_token, user_id, relative_path, content_type, &data)
                .map_err(map_sync_error)?;

            let hash = file_hash(&data);
            let now = chrono::Utc::now().to_rfc3339();
            meta.files.insert(
                relative_path.clone(),
                FileSyncInfo {
                    hash,
                    last_synced: now,
                },
            );
        }

        Ok(meta)
    })
}

pub fn sync_changes(
    access_token: &str,
    user_id: &str,
    initial_meta: SyncMeta,
) -> Result<SyncMeta, String> {
    with_sync_lock(|| {
        let mut meta = initial_meta;
        let local_files = scan_agent_files();
        let local_map: HashMap<String, PathBuf> = local_files.into_iter().collect();

        // Compute local hashes once for the plan.
        let mut local_hashes: HashMap<String, String> = HashMap::new();
        for (relative_path, full_path) in &local_map {
            let data = match std::fs::read(full_path) {
                Ok(d) => d,
                Err(_) => continue,
            };
            local_hashes.insert(relative_path.clone(), file_hash(&data));
        }

        let remote_files = supabase::list_files(access_token, user_id).map_err(map_sync_error)?;
        let remote_set: HashSet<String> = remote_files
            .iter()
            .filter_map(|f| {
                if f.id.is_none() {
                    return None;
                }
                f.name.as_ref().and_then(|name| {
                    let relative = name
                        .strip_prefix(&format!("{}/", user_id))
                        .unwrap_or(name)
                        .to_string();
                    if relative == ".sync.lock" || !is_sync_whitelisted(&relative) {
                        None
                    } else {
                        Some(relative)
                    }
                })
            })
            .collect();

        let ops = compute_sync_plan(local_hashes, remote_set.clone(), &meta);

        for op in ops {
            match op {
                SyncOp::Upload { path } => {
                    let full_path = local_map
                        .get(&path)
                        .ok_or_else(|| format!("local file disappeared during sync: {}", path))?;
                    let data = std::fs::read(full_path).map_err(|e| e.to_string())?;
                    let content_type = content_type_for(&path);
                    supabase::upload_file(access_token, user_id, &path, content_type, &data)
                        .map_err(map_sync_error)?;
                    let hash = file_hash(&data);
                    let now = chrono::Utc::now().to_rfc3339();
                    meta.files.insert(
                        path.clone(),
                        FileSyncInfo {
                            hash,
                            last_synced: now,
                        },
                    );
                }
                SyncOp::Download { path } => {
                    let data = supabase::download_file(access_token, user_id, &path)
                        .map_err(map_sync_error)?;
                    let local_path = state::agent_dir().join(&path);
                    if let Some(parent) = local_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    std::fs::write(&local_path, &data).map_err(|e| e.to_string())?;
                    let hash = file_hash(&data);
                    let now = chrono::Utc::now().to_rfc3339();
                    meta.files.insert(
                        path.clone(),
                        FileSyncInfo {
                            hash,
                            last_synced: now,
                        },
                    );
                }
                SyncOp::DeleteRemote { path } => {
                    supabase::delete_file(access_token, user_id, &path).map_err(map_sync_error)?;
                    meta.files.remove(&path);
                }
                SyncOp::DeleteLocal { path } => {
                    let local_path = state::agent_dir().join(&path);
                    std::fs::remove_file(&local_path)
                        .map_err(|e| format!("failed to delete local file {}: {}", path, e))?;
                    meta.files.remove(&path);
                }
            }
        }

        // Final cleanup: files that vanished from both sides can be dropped from meta.
        let local_set: HashSet<String> = local_map.keys().cloned().collect();
        let stale: Vec<String> = meta
            .files
            .keys()
            .filter(|p| !local_set.contains(*p) && !remote_set.contains(*p))
            .cloned()
            .collect();
        for path in stale {
            meta.files.remove(&path);
        }

        Ok(meta)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta_with(paths: &[(&str, &str)]) -> SyncMeta {
        let mut meta = SyncMeta::default();
        for (path, hash) in paths {
            meta.files.insert(
                (*path).to_string(),
                FileSyncInfo {
                    hash: (*hash).to_string(),
                    last_synced: "2024-01-01T00:00:00Z".to_string(),
                },
            );
        }
        meta
    }

    #[test]
    fn local_deletion_of_synced_file_deletes_remote() {
        let local = HashMap::new();
        let remote: HashSet<String> = ["settings.json".to_string()].into_iter().collect();
        let meta = meta_with(&[("settings.json", "abc123")]);

        let ops = compute_sync_plan(local, remote, &meta);

        assert_eq!(
            ops,
            vec![SyncOp::DeleteRemote {
                path: "settings.json".to_string()
            }]
        );
    }

    #[test]
    fn remote_deletion_of_synced_file_deletes_local() {
        let mut local = HashMap::new();
        local.insert("extensions/foo.json".to_string(), "abc123".to_string());
        let remote = HashSet::new();
        let meta = meta_with(&[("extensions/foo.json", "abc123")]);

        let ops = compute_sync_plan(local, remote, &meta);

        assert_eq!(
            ops,
            vec![SyncOp::DeleteLocal {
                path: "extensions/foo.json".to_string()
            }]
        );
    }

    #[test]
    fn new_local_file_gets_uploaded() {
        let mut local = HashMap::new();
        local.insert("auth.json".to_string(), "hash1".to_string());
        let remote = HashSet::new();
        let meta = SyncMeta::default();

        let ops = compute_sync_plan(local, remote, &meta);

        assert_eq!(
            ops,
            vec![SyncOp::Upload {
                path: "auth.json".to_string()
            }]
        );
    }

    #[test]
    fn new_remote_file_gets_downloaded() {
        let local = HashMap::new();
        let remote: HashSet<String> = ["models.json".to_string()].into_iter().collect();
        let meta = SyncMeta::default();

        let ops = compute_sync_plan(local, remote, &meta);

        assert_eq!(
            ops,
            vec![SyncOp::Download {
                path: "models.json".to_string()
            }]
        );
    }

    #[test]
    fn modified_local_file_gets_uploaded() {
        let mut local = HashMap::new();
        local.insert("settings.json".to_string(), "newhash".to_string());
        let remote: HashSet<String> = ["settings.json".to_string()].into_iter().collect();
        let meta = meta_with(&[("settings.json", "oldhash")]);

        let ops = compute_sync_plan(local, remote, &meta);

        assert_eq!(
            ops,
            vec![SyncOp::Upload {
                path: "settings.json".to_string()
            }]
        );
    }

    #[test]
    fn unchanged_synced_file_generates_no_ops() {
        let mut local = HashMap::new();
        local.insert("settings.json".to_string(), "samehash".to_string());
        let remote: HashSet<String> = ["settings.json".to_string()].into_iter().collect();
        let meta = meta_with(&[("settings.json", "samehash")]);

        let ops = compute_sync_plan(local, remote, &meta);

        assert!(ops.is_empty());
    }

    #[test]
    fn file_missing_everywhere_generates_no_ops() {
        let local = HashMap::new();
        let remote = HashSet::new();
        let meta = meta_with(&[("settings.json", "abc123")]);

        let ops = compute_sync_plan(local, remote, &meta);

        assert!(ops.is_empty());
    }

    #[test]
    fn mixed_plan_orders_ops_by_kind() {
        let mut local = HashMap::new();
        local.insert("settings.json".to_string(), "newhash".to_string());
        local.insert("extensions/a.json".to_string(), "ahash".to_string());
        let remote: HashSet<String> = [
            "settings.json".to_string(),
            "extensions/b.json".to_string(),
            "auth.json".to_string(),
        ]
        .into_iter()
        .collect();
        let meta = meta_with(&[
            ("settings.json", "oldhash"),
            ("auth.json", "authhash"),
            ("extensions/a.json", "ahash"),
        ]);

        let ops = compute_sync_plan(local, remote, &meta);

        // settings.json: both sides, local changed -> Upload
        // extensions/a.json: local only, in meta -> DeleteLocal
        // extensions/b.json: remote only, not in meta -> Download
        // auth.json: remote only, in meta -> DeleteRemote
        let mut ops = ops;
        ops.sort_by(|a, b| format!("{:?}", a).cmp(&format!("{:?}", b)));
        let mut expected = vec![
            SyncOp::Upload {
                path: "settings.json".to_string(),
            },
            SyncOp::DeleteRemote {
                path: "auth.json".to_string(),
            },
            SyncOp::Download {
                path: "extensions/b.json".to_string(),
            },
            SyncOp::DeleteLocal {
                path: "extensions/a.json".to_string(),
            },
        ];
        expected.sort_by(|a, b| format!("{:?}", a).cmp(&format!("{:?}", b)));
        assert_eq!(ops, expected);
    }
}
