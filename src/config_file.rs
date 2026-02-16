use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{AgentConfig, ApprovalEntry, HookConfig, TriggerEntry};

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
