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
    #[serde(default = "default_claude_md")]
    pub claude_md: ClaudeMdConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            analysis: default_analysis(),
            ai: default_ai(),
            hooks: default_hooks(),
            paths: default_paths(),
            privacy: default_privacy(),
            claude_md: default_claude_md(),
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
    #[serde(default = "default_rolling_window")]
    pub rolling_window: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    #[serde(default = "default_backend")]
    pub backend: String,
    #[serde(default = "default_model")]
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HooksConfig {
    #[serde(default = "default_ingest_cooldown")]
    pub ingest_cooldown_minutes: u32,
    #[serde(default = "default_analyze_cooldown")]
    pub analyze_cooldown_minutes: u32,
    #[serde(default = "default_apply_cooldown")]
    pub apply_cooldown_minutes: u32,
    #[serde(default = "default_auto_apply")]
    pub auto_apply: bool,
    #[serde(default = "default_post_commit")]
    pub post_commit: String,
    #[serde(default = "default_post_merge")]
    pub post_merge: String,
    #[serde(default = "default_auto_analyze_max_sessions")]
    pub auto_analyze_max_sessions: u32,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeMdConfig {
    #[serde(default = "default_full_management")]
    pub full_management: bool,
}

fn default_analysis() -> AnalysisConfig {
    AnalysisConfig {
        window_days: default_window_days(),
        confidence_threshold: default_confidence_threshold(),
        staleness_days: default_staleness_days(),
        rolling_window: default_rolling_window(),
    }
}

fn default_ai() -> AiConfig {
    AiConfig {
        backend: default_backend(),
        model: default_model(),
    }
}

fn default_hooks() -> HooksConfig {
    HooksConfig {
        ingest_cooldown_minutes: default_ingest_cooldown(),
        analyze_cooldown_minutes: default_analyze_cooldown(),
        apply_cooldown_minutes: default_apply_cooldown(),
        auto_apply: default_auto_apply(),
        post_commit: default_post_commit(),
        post_merge: default_post_merge(),
        auto_analyze_max_sessions: default_auto_analyze_max_sessions(),
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
fn default_rolling_window() -> bool {
    true
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
fn default_ingest_cooldown() -> u32 {
    5
}
fn default_analyze_cooldown() -> u32 {
    1440
}
fn default_apply_cooldown() -> u32 {
    1440
}
fn default_auto_apply() -> bool {
    true
}
fn default_post_commit() -> String {
    "ingest".to_string()
}
fn default_post_merge() -> String {
    "analyze".to_string()
}
fn default_auto_analyze_max_sessions() -> u32 {
    15
}
fn default_claude_dir() -> String {
    "~/.claude".to_string()
}
fn default_scrub_secrets() -> bool {
    true
}

fn default_claude_md() -> ClaudeMdConfig {
    ClaudeMdConfig {
        full_management: default_full_management(),
    }
}

fn default_full_management() -> bool {
    false
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hooks_config_defaults() {
        let config = default_hooks();
        assert_eq!(config.ingest_cooldown_minutes, 5);
        assert_eq!(config.analyze_cooldown_minutes, 1440);
        assert_eq!(config.apply_cooldown_minutes, 1440);
        assert!(config.auto_apply);
    }

    #[test]
    fn test_hooks_config_new_fields_deserialize() {
        let toml_str = r#"
[hooks]
ingest_cooldown_minutes = 10
analyze_cooldown_minutes = 720
apply_cooldown_minutes = 2880
auto_apply = false
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.hooks.ingest_cooldown_minutes, 10);
        assert_eq!(config.hooks.analyze_cooldown_minutes, 720);
        assert_eq!(config.hooks.apply_cooldown_minutes, 2880);
        assert!(!config.hooks.auto_apply);
    }

    #[test]
    fn test_hooks_config_partial_deserialize() {
        // Config with only some fields should fill defaults for the rest
        let toml_str = r#"
[hooks]
ingest_cooldown_minutes = 10
auto_apply = false
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.hooks.ingest_cooldown_minutes, 10);
        assert_eq!(config.hooks.analyze_cooldown_minutes, 1440); // default
        assert_eq!(config.hooks.apply_cooldown_minutes, 1440); // default
        assert!(!config.hooks.auto_apply);
    }

    #[test]
    fn test_hooks_config_max_sessions_default() {
        let config = Config::default();
        assert_eq!(config.hooks.auto_analyze_max_sessions, 15);
    }

    #[test]
    fn test_hooks_config_max_sessions_custom() {
        let toml_str = r#"
[hooks]
auto_analyze_max_sessions = 5
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.hooks.auto_analyze_max_sessions, 5);
    }

    #[test]
    fn test_claude_md_config_defaults() {
        let config = Config::default();
        assert!(!config.claude_md.full_management);
    }

    #[test]
    fn test_claude_md_config_custom() {
        let toml_str = r#"
[claude_md]
full_management = true
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.claude_md.full_management);
    }

    #[test]
    fn test_claude_md_config_absent() {
        let toml_str = r#"
[analysis]
window_days = 7
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(!config.claude_md.full_management);
    }
}
