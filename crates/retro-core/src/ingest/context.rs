use crate::config::Config;
use crate::errors::CoreError;
use crate::models::{AgentFile, ContextSnapshot, PluginSkillSummary, SkillFile};
use std::path::Path;

/// Snapshot the current context: CLAUDE.md, skills, MEMORY.md, global agents.
pub fn snapshot_context(
    config: &Config,
    project_path: &str,
) -> Result<ContextSnapshot, CoreError> {
    let project = Path::new(project_path);
    let claude_dir = config.claude_dir();

    // Read CLAUDE.md from project root
    let claude_md = read_optional_file(&project.join("CLAUDE.md"));

    // Read project-level skills
    let skills_dir = project.join(".claude").join("skills");
    let skills = read_skills(&skills_dir);

    // Read MEMORY.md from claude projects dir
    let encoded_path = crate::ingest::encode_project_path(project_path);
    let memory_path = claude_dir
        .join("projects")
        .join(&encoded_path)
        .join("memory")
        .join("MEMORY.md");
    let memory_md = read_optional_file(&memory_path);

    // Read global agents
    let agents_dir = claude_dir.join("agents");
    let global_agents = read_agents(&agents_dir);

    // Read plugin skills
    let plugin_skills = read_plugin_skills(&claude_dir);

    Ok(ContextSnapshot {
        claude_md,
        skills,
        memory_md,
        global_agents,
        plugin_skills,
    })
}

fn read_optional_file(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn read_skills(dir: &Path) -> Vec<SkillFile> {
    let mut skills = Vec::new();

    if !dir.exists() {
        return skills;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return skills,
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        if !entry.path().is_dir() {
            continue;
        }

        let skill_md = entry.path().join("SKILL.md");
        if let Ok(content) = std::fs::read_to_string(&skill_md) {
            skills.push(SkillFile {
                path: skill_md.to_string_lossy().to_string(),
                content,
            });
        }
    }

    skills
}

/// Extract `name` and `description` from `---`-delimited YAML frontmatter.
/// Simple string parsing — no YAML crate needed.
pub fn parse_skill_frontmatter(content: &str) -> Option<(String, String)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }

    // Find closing ---
    let after_open = &trimmed[3..];
    let close_idx = after_open.find("\n---")?;
    let frontmatter = &after_open[..close_idx];

    let mut name = None;
    let mut description = None;

    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name:") {
            let val = rest.trim().trim_matches('"').trim_matches('\'');
            if !val.is_empty() {
                name = Some(val.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("description:") {
            let val = rest.trim().trim_matches('"').trim_matches('\'');
            if !val.is_empty() {
                description = Some(val.to_string());
            }
        }
    }

    match (name, description) {
        (Some(n), Some(d)) => Some((n, d)),
        _ => None,
    }
}

/// Read plugin skills from installed_plugins.json.
/// Returns empty vec if file is missing or unparseable.
fn read_plugin_skills(claude_dir: &Path) -> Vec<PluginSkillSummary> {
    let plugins_file = claude_dir.join("plugins").join("installed_plugins.json");
    let content = match std::fs::read_to_string(&plugins_file) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let parsed: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let obj = match parsed.as_object() {
        Some(o) => o,
        None => return Vec::new(),
    };

    let mut result = Vec::new();

    for (plugin_name, plugin_value) in obj {
        // Each plugin entry is an array; take first element's installPath
        let install_path = plugin_value
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|entry| entry.get("installPath"))
            .and_then(|v| v.as_str());

        let install_path = match install_path {
            Some(p) => p,
            None => continue,
        };

        // Glob for skills in this plugin
        let skills_pattern = Path::new(install_path).join("skills").join("*").join("SKILL.md");
        let pattern_str = skills_pattern.to_string_lossy();

        let paths = match glob::glob(&pattern_str) {
            Ok(p) => p,
            Err(_) => continue,
        };

        for path in paths.filter_map(|r| r.ok()) {
            if let Ok(skill_content) = std::fs::read_to_string(&path) {
                if let Some((skill_name, description)) = parse_skill_frontmatter(&skill_content) {
                    result.push(PluginSkillSummary {
                        plugin_name: plugin_name.clone(),
                        skill_name,
                        description,
                    });
                }
            }
        }
    }

    result
}

fn read_agents(dir: &Path) -> Vec<AgentFile> {
    let mut agents = Vec::new();

    if !dir.exists() {
        return agents;
    }

    let pattern = dir.join("*.md");
    let pattern_str = pattern.to_string_lossy();

    if let Ok(paths) = glob::glob(&pattern_str) {
        for path in paths.filter_map(|r| r.ok()) {
            if let Ok(content) = std::fs::read_to_string(&path) {
                agents.push(AgentFile {
                    path: path.to_string_lossy().to_string(),
                    content,
                });
            }
        }
    }

    agents
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_skill_frontmatter_standard() {
        let content = r#"---
name: brainstorming
description: Explores user intent, requirements and design
---

# Brainstorming

Some content here.
"#;
        let result = parse_skill_frontmatter(content);
        assert_eq!(
            result,
            Some(("brainstorming".to_string(), "Explores user intent, requirements and design".to_string()))
        );
    }

    #[test]
    fn test_parse_skill_frontmatter_quoted() {
        let content = r#"---
name: "my-skill"
description: "A skill with quotes"
---
body
"#;
        let result = parse_skill_frontmatter(content);
        assert_eq!(
            result,
            Some(("my-skill".to_string(), "A skill with quotes".to_string()))
        );
    }

    #[test]
    fn test_parse_skill_frontmatter_single_quoted() {
        let content = "---\nname: 'test'\ndescription: 'A test skill'\n---\n";
        let result = parse_skill_frontmatter(content);
        assert_eq!(
            result,
            Some(("test".to_string(), "A test skill".to_string()))
        );
    }

    #[test]
    fn test_parse_skill_frontmatter_no_frontmatter() {
        let content = "# Just a heading\nNo frontmatter here.";
        assert_eq!(parse_skill_frontmatter(content), None);
    }

    #[test]
    fn test_parse_skill_frontmatter_missing_description() {
        let content = "---\nname: incomplete\n---\nbody\n";
        assert_eq!(parse_skill_frontmatter(content), None);
    }

    #[test]
    fn test_parse_skill_frontmatter_missing_name() {
        let content = "---\ndescription: no name field\n---\nbody\n";
        assert_eq!(parse_skill_frontmatter(content), None);
    }

    #[test]
    fn test_read_plugin_skills_no_file() {
        let dir = std::path::PathBuf::from("/nonexistent/path/.claude");
        let result = read_plugin_skills(&dir);
        assert!(result.is_empty());
    }
}
