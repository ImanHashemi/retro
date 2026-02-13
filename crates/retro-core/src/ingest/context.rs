use crate::config::Config;
use crate::errors::CoreError;
use crate::models::{AgentFile, ContextSnapshot, SkillFile};
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

    Ok(ContextSnapshot {
        claude_md,
        skills,
        memory_md,
        global_agents,
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
