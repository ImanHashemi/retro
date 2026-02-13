use crate::errors::CoreError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_analysis")]
    pub analysis: AnalysisConfig,
    #[serde(default = "default_ai")]
    pub ai: AiConfig,
    #[serde(default = "default_hooks")]
    pub hooks: HooksConfig,
    #[serde(default = "default_paths")]
    pub paths: PathsConfig,
    #[serde(default = "default_privacy")]
    pub privacy: PrivacyConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            analysis: default_analysis(),
            ai: default_ai(),
            hooks: default_hooks(),
            paths: default_paths(),
            privacy: default_privacy(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisConfig {
    #[serde(default = "default_window_days")]
    pub window_days: u32,
    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: f64,
    #[serde(default = "default_staleness_days")]
    pub staleness_days: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    #[serde(default = "default_backend")]
    pub backend: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_max_budget")]
    pub max_budget_per_call: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HooksConfig {
    #[serde(default = "default_cooldown")]
    pub auto_cooldown_minutes: u32,
    #[serde(default = "default_post_commit")]
    pub post_commit: String,
    #[serde(default = "default_post_merge")]
    pub post_merge: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathsConfig {
    #[serde(default = "default_claude_dir")]
    pub claude_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacyConfig {
    #[serde(default = "default_scrub_secrets")]
    pub scrub_secrets: bool,
    #[serde(default)]
    pub exclude_projects: Vec<String>,
}

fn default_analysis() -> AnalysisConfig {
    AnalysisConfig {
        window_days: default_window_days(),
        confidence_threshold: default_confidence_threshold(),
        staleness_days: default_staleness_days(),
    }
}

fn default_ai() -> AiConfig {
    AiConfig {
        backend: default_backend(),
        model: default_model(),
        max_budget_per_call: default_max_budget(),
    }
}

fn default_hooks() -> HooksConfig {
    HooksConfig {
        auto_cooldown_minutes: default_cooldown(),
        post_commit: default_post_commit(),
        post_merge: default_post_merge(),
    }
}

fn default_paths() -> PathsConfig {
    PathsConfig {
        claude_dir: default_claude_dir(),
    }
}

fn default_privacy() -> PrivacyConfig {
    PrivacyConfig {
        scrub_secrets: default_scrub_secrets(),
        exclude_projects: Vec::new(),
    }
}

fn default_window_days() -> u32 {
    14
}
fn default_confidence_threshold() -> f64 {
    0.7
}
fn default_staleness_days() -> u32 {
    28
}
fn default_backend() -> String {
    "claude-cli".to_string()
}
fn default_model() -> String {
    "sonnet".to_string()
}
fn default_max_budget() -> f64 {
    0.50
}
fn default_cooldown() -> u32 {
    60
}
fn default_post_commit() -> String {
    "ingest".to_string()
}
fn default_post_merge() -> String {
    "analyze".to_string()
}
fn default_claude_dir() -> String {
    "~/.claude".to_string()
}
fn default_scrub_secrets() -> bool {
    true
}

impl Config {
    /// Load config from the given path, or return defaults if file doesn't exist.
    pub fn load(path: &Path) -> Result<Self, CoreError> {
        if path.exists() {
            let contents = std::fs::read_to_string(path)
                .map_err(|e| CoreError::Io(format!("reading config: {e}")))?;
            let config: Config =
                toml::from_str(&contents).map_err(|e| CoreError::Config(e.to_string()))?;
            Ok(config)
        } else {
            Ok(Config::default())
        }
    }

    /// Write config to the given path.
    pub fn save(&self, path: &Path) -> Result<(), CoreError> {
        let contents =
            toml::to_string_pretty(self).map_err(|e| CoreError::Config(e.to_string()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CoreError::Io(format!("creating config dir: {e}")))?;
        }
        std::fs::write(path, contents)
            .map_err(|e| CoreError::Io(format!("writing config: {e}")))?;
        Ok(())
    }

    /// Resolve the claude_dir path, expanding ~ to home directory.
    pub fn claude_dir(&self) -> PathBuf {
        expand_tilde(&self.paths.claude_dir)
    }
}

/// Get the retro data directory (~/.retro/).
pub fn retro_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".retro")
}

/// Expand ~ at the start of a path.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(rest)
    } else if path == "~" {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
    } else {
        PathBuf::from(path)
    }
}
