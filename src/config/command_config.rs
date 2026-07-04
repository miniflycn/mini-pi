use serde_json::Value;

use crate::rpc::pi_rpc::{PiBridge, PiRpcError};

#[derive(Clone, Debug)]
pub struct CommandItem {
    pub name: String,
    pub description: Option<String>,
    pub source: String,
}

/// Parse the `commands` array from a `get_commands` response object.
///
/// Expects `response` to be an object with a `commands` field holding the
/// array. Missing or malformed values return an empty vector.
pub fn parse_command_items(response: &Value) -> Vec<CommandItem> {
    let Some(arr) = response.get("commands").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    arr.iter()
        .filter_map(|cmd| {
            let name = cmd.get("name")?.as_str()?.to_string();
            let description = cmd
                .get("description")
                .and_then(|d| d.as_str())
                .map(|s| s.to_string());
            let source = cmd
                .get("source")
                .and_then(|s| s.as_str())
                .unwrap_or("unknown")
                .to_string();
            Some(CommandItem {
                name,
                description,
                source,
            })
        })
        .collect()
}

/// Load the effective slash-command list from the SDK bridge.
///
/// This uses a temporary session, so it can be called once at app startup
/// before any chat session exists.
pub fn load_commands(bridge: &PiBridge) -> Result<Vec<CommandItem>, PiRpcError> {
    let response = bridge.get_commands()?;
    Ok(parse_command_items(&response))
}

pub fn filter_command_items(items: &[CommandItem], query: &str) -> Vec<CommandItem> {
    if query.is_empty() {
        return items.iter().take(50).cloned().collect();
    }
    let q = query.to_lowercase();
    let mut matches: Vec<(CommandItem, usize)> = items
        .iter()
        .filter_map(|item| {
            let name_lower = item.name.to_lowercase();
            let desc_lower = item.description.as_deref().unwrap_or("").to_lowercase();
            if name_lower.contains(&q) || desc_lower.contains(&q) {
                let score = if name_lower.starts_with(&q) {
                    100
                } else if name_lower.contains(&q) {
                    50
                } else {
                    10
                };
                Some((item.clone(), score))
            } else {
                None
            }
        })
        .collect();
    matches.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name.cmp(&b.0.name)));
    matches.into_iter().take(50).map(|(item, _)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_missing_commands_returns_empty() {
        assert!(parse_command_items(&serde_json::json!({})).is_empty());
        assert!(parse_command_items(&serde_json::Value::Null).is_empty());
    }

    #[test]
    fn parse_array_input_returns_empty() {
        // parse_command_items expects the parent response object, not the
        // raw array.
        let input = serde_json::json!([
            { "name": "cmd" }
        ]);
        assert!(parse_command_items(&input).is_empty());
    }

    #[test]
    fn parse_object_with_commands() {
        let input = serde_json::json!({
            "commands": [
                { "name": "read", "description": "Read a file", "source": "fs" },
                { "name": "write", "source": "fs" },
                { "description": "missing name" },
                { "name": "edit" }
            ]
        });
        let items = parse_command_items(&input);
        assert_eq!(items.len(), 3);

        assert_eq!(items[0].name, "read");
        assert_eq!(items[0].description, Some("Read a file".to_string()));
        assert_eq!(items[0].source, "fs");

        assert_eq!(items[1].name, "write");
        assert_eq!(items[1].description, None);
        assert_eq!(items[1].source, "fs");

        assert_eq!(items[2].name, "edit");
        assert_eq!(items[2].description, None);
        assert_eq!(items[2].source, "unknown");
    }

    #[test]
    fn parse_empty_commands_array() {
        let input = serde_json::json!({ "commands": [] });
        assert!(parse_command_items(&input).is_empty());
    }

    #[test]
    fn filter_empty_query_takes_first_50() {
        let items: Vec<CommandItem> = (0..60)
            .map(|i| CommandItem {
                name: format!("cmd{}", i),
                description: None,
                source: "test".to_string(),
            })
            .collect();
        assert_eq!(filter_command_items(&items, "").len(), 50);
    }

    #[test]
    fn filter_prefix_scores_highest() {
        let items = vec![
            CommandItem {
                name: "search-files".to_string(),
                description: Some("Find files".to_string()),
                source: "test".to_string(),
            },
            CommandItem {
                name: "grep".to_string(),
                description: Some("Search inside files".to_string()),
                source: "test".to_string(),
            },
        ];
        let filtered = filter_command_items(&items, "search");
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].name, "search-files");
        assert_eq!(filtered[1].name, "grep");
    }

    #[test]
    fn filter_description_only_match() {
        let items = vec![CommandItem {
            name: "cat".to_string(),
            description: Some("Show file contents".to_string()),
            source: "test".to_string(),
        }];
        let filtered = filter_command_items(&items, "contents");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "cat");
    }

    #[test]
    fn filter_no_match_returns_empty() {
        let items = vec![CommandItem {
            name: "ls".to_string(),
            description: None,
            source: "test".to_string(),
        }];
        assert!(filter_command_items(&items, "xyz").is_empty());
    }
}
