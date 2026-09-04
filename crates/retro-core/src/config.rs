use crate::errors::CoreError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_analysis")]
    pub analysis: AnalysisConfig,
    #[serde(default = "default_ai")]
    pub ai: AiConfig,
    #[serde(default = "default_paths")]
    pub paths: PathsConfig,
    #[serde(default = "default_privacy")]
    pub privacy: PrivacyConfig,
    #[serde(default = "default_runner")]
    pub runner: RunnerConfig,
    #[serde(default = "default_knowledge")]
    pub knowledge: KnowledgeConfig,
    #[serde(default = "default_ui")]
    pub ui: UiConfig,
    #[serde(default = "default_curator")]
    pub curator: CuratorConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            analysis: default_analysis(),
            ai: default_ai(),
            paths: default_paths(),
            privacy: default_privacy(),
            runner: default_runner(),
            knowledge: default_knowledge(),
            ui: default_ui(),
            curator: default_curator(),
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
pub struct RunnerConfig {
    #[serde(default = "default_max_ai_calls_per_day")]
    pub max_ai_calls_per_day: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeConfig {
    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: f64,
    #[serde(default = "default_global_promotion_threshold")]
    pub global_promotion_threshold: f64,
}

/// v3 dashboard server settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default = "default_ui_port")]
    pub port: u16,
}

fn default_ui_port() -> u16 {
    7777
}

/// What the curator is allowed to do with one class of finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CuratorPolicy {
    /// Apply silently, under the per-run cap.
    Auto,
    /// Queue for the user to decide.
    Review,
    /// Skip detection entirely.
    Off,
}

/// Context-curator behaviour. Each op is independently settable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuratorConfig {
    #[serde(default = "default_merge_duplicates")]
    pub merge_duplicates: CuratorPolicy,
    #[serde(default = "default_archive_stale")]
    pub archive_stale: CuratorPolicy,
    #[serde(default = "default_skills_policy")]
    pub skills: CuratorPolicy,
    /// Near-duplicate similarity threshold; also drives the length pre-filter.
    #[serde(default = "default_merge_similarity")]
    pub merge_similarity: f64,
    #[serde(default = "default_max_auto_per_run")]
    pub max_auto_merges_per_run: u32,
    #[serde(default = "default_max_auto_per_run")]
    pub max_auto_archives_per_run: u32,
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
fn default_claude_dir() -> String {
    "~/.claude".to_string()
}
fn default_scrub_secrets() -> bool {
    true
}

fn default_max_ai_calls_per_day() -> u32 {
    10
}
fn default_global_promotion_threshold() -> f64 {
    0.85
}

fn default_runner() -> RunnerConfig {
    RunnerConfig {
        max_ai_calls_per_day: default_max_ai_calls_per_day(),
    }
}

fn default_knowledge() -> KnowledgeConfig {
    KnowledgeConfig {
        confidence_threshold: default_confidence_threshold(),
        global_promotion_threshold: default_global_promotion_threshold(),
    }
}

fn default_ui() -> UiConfig {
    UiConfig {
        port: default_ui_port(),
    }
}

fn default_merge_duplicates() -> CuratorPolicy {
    CuratorPolicy::Auto
}
fn default_archive_stale() -> CuratorPolicy {
    CuratorPolicy::Auto
}
fn default_skills_policy() -> CuratorPolicy {
    CuratorPolicy::Review
}
fn default_merge_similarity() -> f64 {
    0.8
}
fn default_max_auto_per_run() -> u32 {
    3
}

fn default_curator() -> CuratorConfig {
    CuratorConfig {
        merge_duplicates: default_merge_duplicates(),
        archive_stale: default_archive_stale(),
        skills: default_skills_policy(),
        merge_similarity: default_merge_similarity(),
        max_auto_merges_per_run: default_max_auto_per_run(),
        max_auto_archives_per_run: default_max_auto_per_run(),
    }
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

/// Get the retro data directory.
/// Uses `RETRO_HOME` env var if set, otherwise defaults to `~/.retro/`.
pub fn retro_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("RETRO_HOME") {
        return PathBuf::from(dir);
    }
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
    fn test_runner_config_defaults() {
        let config = Config::default();
        assert_eq!(config.runner.max_ai_calls_per_day, 10);
    }

    #[test]
    fn test_knowledge_config_defaults() {
        let config = Config::default();
        assert_eq!(config.knowledge.confidence_threshold, 0.7);
        assert_eq!(config.knowledge.global_promotion_threshold, 0.85);
    }

    #[test]
    fn test_runner_and_knowledge_config_deserialize() {
        let toml_str = r#"
[runner]
max_ai_calls_per_day = 5

[knowledge]
confidence_threshold = 0.8
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.runner.max_ai_calls_per_day, 5);
        assert_eq!(config.knowledge.confidence_threshold, 0.8);
    }

    #[test]
    fn test_config_with_removed_sections_still_loads() {
        // Old config.toml files may still have [hooks]/[trust]/[claude_md]/[v3]
        // sections from earlier retro versions — serde ignores unknown keys,
        // so these must not fail to parse.
        let toml_str = r#"
[analysis]
window_days = 7

[hooks]
ingest_cooldown_minutes = 10
auto_apply = false

[trust]
mode = "auto"

[claude_md]
full_management = true

[v3]
enabled = true
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.analysis.window_days, 7);
        // current sections still get their defaults
        assert_eq!(config.runner.max_ai_calls_per_day, 10);
        assert_eq!(config.knowledge.confidence_threshold, 0.7);
    }

    #[test]
    fn test_retro_dir_default() {
        // SAFETY: single-threaded test, no concurrent env access
        unsafe { std::env::remove_var("RETRO_HOME") };
        let dir = retro_dir();
        assert!(dir.to_string_lossy().ends_with(".retro"));
    }

    #[test]
    fn test_retro_dir_override() {
        let original = std::env::var("RETRO_HOME").ok();
        // SAFETY: single-threaded test, no concurrent env access
        unsafe { std::env::set_var("RETRO_HOME", "/tmp/test-retro") };
        let dir = retro_dir();
        assert_eq!(dir, PathBuf::from("/tmp/test-retro"));
        // SAFETY: restoring env to original state
        unsafe {
            match original {
                Some(val) => std::env::set_var("RETRO_HOME", val),
                None => std::env::remove_var("RETRO_HOME"),
            }
        }
    }

    #[test]
    fn ui_section_defaults_and_roundtrips() {
        let config = Config::default();
        assert_eq!(config.ui.port, 7777);
        let parsed: Config = toml::from_str("").unwrap();
        assert_eq!(parsed.ui.port, 7777);
        let parsed: Config = toml::from_str("[ui]\nport = 9000\n").unwrap();
        assert_eq!(parsed.ui.port, 9000);
    }

    #[test]
    fn curator_defaults_match_the_spec() {
        let c = Config::default();
        assert_eq!(c.curator.merge_duplicates, CuratorPolicy::Auto);
        assert_eq!(c.curator.archive_stale, CuratorPolicy::Auto);
        assert_eq!(c.curator.skills, CuratorPolicy::Review);
        assert_eq!(c.curator.merge_similarity, 0.8);
        assert_eq!(c.curator.max_auto_merges_per_run, 3);
        assert_eq!(c.curator.max_auto_archives_per_run, 3);
    }

    #[test]
    fn curator_section_parses_lowercase_policies() {
        let toml_src = r#"
[curator]
merge_duplicates = "review"
archive_stale = "off"
skills = "auto"
merge_similarity = 0.75
max_auto_merges_per_run = 5
"#;
        let c: Config = toml::from_str(toml_src).unwrap();
        assert_eq!(c.curator.merge_duplicates, CuratorPolicy::Review);
        assert_eq!(c.curator.archive_stale, CuratorPolicy::Off);
        assert_eq!(c.curator.skills, CuratorPolicy::Auto);
        assert_eq!(c.curator.merge_similarity, 0.75);
        assert_eq!(c.curator.max_auto_merges_per_run, 5);
        // omitted key falls back to its default
        assert_eq!(c.curator.max_auto_archives_per_run, 3);
    }

    #[test]
    fn config_without_curator_section_still_loads() {
        let c: Config = toml::from_str("[runner]\nmax_ai_calls_per_day = 20\n").unwrap();
        assert_eq!(c.runner.max_ai_calls_per_day, 20);
        assert_eq!(c.curator.merge_duplicates, CuratorPolicy::Auto);
    }

    #[test]
    fn curator_config_roundtrips_through_save_and_load() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let config = Config::default();
        config.save(&path).unwrap();

        // The lowercase-serialized policy strings must appear verbatim in
        // the saved file — pins `#[serde(rename_all = "lowercase")]`.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("merge_duplicates = \"auto\""));
        assert!(raw.contains("archive_stale = \"auto\""));
        assert!(raw.contains("skills = \"review\""));

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.curator.merge_duplicates, CuratorPolicy::Auto);
        assert_eq!(loaded.curator.archive_stale, CuratorPolicy::Auto);
        assert_eq!(loaded.curator.skills, CuratorPolicy::Review);
        assert_eq!(loaded.curator.merge_similarity, 0.8);
        assert_eq!(loaded.curator.max_auto_merges_per_run, 3);
        assert_eq!(loaded.curator.max_auto_archives_per_run, 3);
    }

    #[test]
    fn invalid_curator_policy_string_is_a_parse_error() {
        let toml_src = r#"
[curator]
merge_duplicates = "sometimes"
"#;
        let result = toml::from_str::<Config>(toml_src);
        assert!(
            result.is_err(),
            "an unrecognised policy string must fail to parse, not silently default"
        );
    }
}
