use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{AgentConfig, ApprovalEntry, CapsuleConfig, HookConfig, TriggerEntry};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct FileConfig {
    #[serde(default)]
    pub(crate) agent: AgentConfig,
    #[serde(default)]
    pub(crate) hooks: Option<HookConfig>,
    #[serde(default)]
    pub(crate) approvals: Vec<ApprovalEntry>,
    #[serde(default)]
    pub(crate) triggers: Vec<TriggerEntry>,
    #[serde(default)]
    pub(crate) oauth_google: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) oauth_microsoft: Option<serde_json::Value>,
}

pub(crate) fn config_file_path(workspace: &Path) -> PathBuf {
    workspace.join("config.json")
}

pub(crate) fn load_file_config(path: &Path) -> FileConfig {
    match std::fs::read_to_string(path) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
        Err(_) => FileConfig::default(),
    }
}

pub(crate) fn load_config_from_file(workspace: &Path) -> CapsuleConfig {
    let path = config_file_path(workspace);
    if !path.exists() {
        return CapsuleConfig::default();
    }

    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(_) => return CapsuleConfig::default(),
    };

    if let Ok(config) = serde_json::from_str::<CapsuleConfig>(&raw) {
        return config;
    }

    if let Ok(file_config) = serde_json::from_str::<FileConfig>(&raw) {
        return file_config.into_capsule_config();
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
        if let Some(index) = value.get("index") {
            if let Ok(config) = serde_json::from_value::<CapsuleConfig>(index.clone()) {
                return config;
            }
            if let Ok(file_config) = serde_json::from_value::<FileConfig>(index.clone()) {
                return file_config.into_capsule_config();
            }
        }
    }

    CapsuleConfig::default()
}

pub(crate) fn save_config_to_file(
    workspace: &Path,
    key: &str,
    value: serde_json::Value,
) -> Result<(), String> {
    let path = config_file_path(workspace);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    if key == "index" {
        let raw = if let Ok(config) = serde_json::from_value::<FileConfig>(value.clone()) {
            serde_json::to_value(config).map_err(|e| e.to_string())?
        } else if let Ok(config) = serde_json::from_value::<CapsuleConfig>(value.clone()) {
            serde_json::to_value(config).map_err(|e| e.to_string())?
        } else if value.is_object() {
            value
        } else {
            return Err("index config must be a JSON object".to_string());
        };

        let json = serde_json::to_string_pretty(&raw).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
        return Ok(());
    }

    let mut root = match std::fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str::<serde_json::Value>(&contents).unwrap_or_else(|_| serde_json::json!({})),
        Err(_) => serde_json::json!({}),
    };

    if !root.is_object() {
        root = serde_json::json!({});
    }

    if let serde_json::Value::Object(ref mut obj) = root {
        obj.insert(key.to_string(), value);
    }
    let json = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

impl FileConfig {
    fn into_capsule_config(self) -> CapsuleConfig {
        CapsuleConfig {
            context: None,
            collections: HashMap::new(),
            hooks: self.hooks,
            agent: Some(self.agent),
            extra: HashMap::new(),
        }
    }
}

pub(crate) fn save_file_config(
    path: &Path,
    config: &FileConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(config)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
