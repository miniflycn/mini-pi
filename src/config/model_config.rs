use std::collections::HashMap;
use std::sync::Arc;

use gpui::SharedString;

use crate::rpc::pi_rpc::{BridgeModel, PiBridge, PiRpcError};

#[derive(Clone, Debug)]
pub struct ModelInfo {
    pub name: String,
    pub id: String,
    pub thinking_level_map: Option<HashMap<String, Option<String>>>,
}

pub fn load_models(bridge: &Arc<PiBridge>) -> Result<Vec<ModelInfo>, PiRpcError> {
    let bridge_models = bridge.get_models()?;
    Ok(bridge_models
        .into_iter()
        .map(|m: BridgeModel| ModelInfo {
            id: format!("{}:{}", m.provider, m.id),
            name: m.name,
            thinking_level_map: m.thinking_level_map,
        })
        .collect())
}

pub fn list_models(models: &[ModelInfo]) -> &[ModelInfo] {
    models
}

pub fn get_model_name<'a>(models: &'a [ModelInfo], id: &str) -> Option<&'a str> {
    models.iter().find(|m| m.id == id).map(|m| m.name.as_str())
}

pub fn get_model_id<'a>(models: &'a [ModelInfo], name: &str) -> Option<&'a str> {
    models
        .iter()
        .find(|m| m.name == name)
        .map(|m| m.id.as_str())
}

pub fn all_models(models: &[ModelInfo]) -> &[ModelInfo] {
    models
}

pub fn parse_model_id(id: &str) -> Option<(&str, &str)> {
    id.split_once(':')
}

/// Reconstruct a full "provider:model" id from the SDK's stored default model.
/// If a provider is supplied and `provider:model_id` exists in the available
/// models, that full id is returned. Otherwise we fall back to any available
/// model whose bare id matches.
pub fn resolve_full_model_id(
    models: &[ModelInfo],
    provider: Option<&str>,
    model_id: Option<&str>,
) -> Option<String> {
    let model_id = model_id?;
    if model_id.is_empty() {
        return None;
    }

    if let Some(provider) = provider {
        let full_id = format!("{}:{}", provider, model_id);
        if models.iter().any(|m| m.id == full_id) {
            return Some(full_id);
        }
    }

    models
        .iter()
        .find(|m| {
            parse_model_id(&m.id)
                .map(|(_, id)| id == model_id)
                .unwrap_or(false)
        })
        .map(|m| m.id.clone())
}

pub fn model_display_name(models: &[ModelInfo], id: Option<&str>) -> SharedString {
    match id {
        Some(id) => get_model_name(models, id)
            .map(|name| SharedString::from(name.to_string()))
            .unwrap_or_else(|| SharedString::from(id.to_string())),
        None => SharedString::from("Select Model"),
    }
}
