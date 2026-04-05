use crate::analysis::backend::AnalysisBackend;
use crate::errors::CoreError;
use crate::models::{
    KnowledgeNode, NodeType, Pattern, PatternStatus, PatternType, SkillDraft, SkillValidation,
    SuggestedTarget,
};
use crate::util;

const MAX_RETRIES: usize = 2;

/// Convert a v2 KnowledgeNode to a v1 Pattern for skill generation.
pub fn node_to_pattern(node: &KnowledgeNode) -> Pattern {
    let pattern_type = match node.node_type {
        NodeType::Skill => PatternType::WorkflowPattern,
        NodeType::Rule | NodeType::Directive => PatternType::RepetitiveInstruction,
        NodeType::Pattern => PatternType::RecurringMistake,
        NodeType::Preference | NodeType::Memory => PatternType::WorkflowPattern,
    };
    let suggested_target = match node.node_type {
        NodeType::Skill => SuggestedTarget::Skill,
        NodeType::Rule
        | NodeType::Directive
        | NodeType::Pattern
        | NodeType::Preference
        | NodeType::Memory => SuggestedTarget::ClaudeMd,
    };

    Pattern {
        id: node.id.clone(),
        pattern_type,
        description: node.content.clone(),
        confidence: node.confidence,
        times_seen: 1,
        first_seen: node.created_at,
        last_seen: node.updated_at,
        last_projected: None,
        status: PatternStatus::Active,
        source_sessions: vec![],
        related_files: vec![],
        suggested_content: node.content.clone(),
        suggested_target,
        project: node.project_id.clone(),
        generation_failed: false,
    }
}

/// JSON schema for constrained decoding of skill validation responses.
const SKILL_VALIDATION_SCHEMA: &str = r#"{"type":"object","properties":{"valid":{"type":"boolean"},"feedback":{"type":"string"}},"required":["valid","feedback"],"additionalProperties":false}"#;

/// Generate a skill with retry logic. Returns Err if all attempts fail.
pub fn generate_with_retry(
    backend: &dyn AnalysisBackend,
    pattern: &Pattern,
    max_retries: usize,
) -> Result<SkillDraft, CoreError> {
    let mut feedback = String::new();
    let retries = max_retries.min(MAX_RETRIES);

    for attempt in 0..=retries {
        let prompt = build_generation_prompt(pattern, if attempt > 0 { Some(&feedback) } else { None });
        let response = backend.execute(&prompt, None)?;
        let content = util::strip_code_fences(&response.text);

        let name = match parse_skill_name(&content) {
            Some(n) => n,
            None => {
                feedback = "The skill must have valid YAML frontmatter with a 'name' field.".to_string();
                continue;
            }
        };

        let draft = SkillDraft {
            name,
            content: content.clone(),
            pattern_id: pattern.id.clone(),
        };

        // Validate
        let validation_prompt = build_validation_prompt(&content, pattern);
        match backend.execute(&validation_prompt, Some(SKILL_VALIDATION_SCHEMA)) {
            Ok(val_response) => {
                match parse_validation(&val_response.text) {
                    Some(v) if v.valid => return Ok(draft),
                    Some(v) => {
                        feedback = v.feedback;
                    }
                    None => {
                        // Validation parse failed — accept the draft if it has valid structure
                        if has_valid_frontmatter(&content) {
                            return Ok(draft);
                        }
                        feedback = "Skill validation response was unparseable.".to_string();
                    }
                }
            }
            Err(_) => {
                // Validation call failed — accept draft if structurally valid
                if has_valid_frontmatter(&content) {
                    return Ok(draft);
                }
                feedback = "Skill validation call failed.".to_string();
            }
        }
    }

    Err(CoreError::Analysis(format!(
        "skill generation failed after {} retries for pattern {}",
        retries, pattern.id
    )))
}

fn build_generation_prompt(pattern: &Pattern, feedback: Option<&str>) -> String {
    let feedback_section = match feedback {
        Some(fb) => format!(
            "\n\n## Previous Attempt Feedback\n\nYour previous attempt was rejected: {fb}\nPlease address this feedback in your new attempt.\n"
        ),
        None => String::new(),
    };

    let related = if pattern.related_files.is_empty() {
        "None".to_string()
    } else {
        pattern.related_files.join(", ")
    };

    format!(
        r#"You are an expert at writing Claude Code skills. A skill is a reusable instruction file that Claude Code discovers and applies automatically.

Generate a skill for the following discovered pattern:

**Pattern Type:** {pattern_type}
**Description:** {description}
**Suggested Content:** {suggested_content}
**Related Files:** {related}
**Times Seen:** {times_seen}
{feedback_section}
## Skill Format

The skill MUST follow this exact format:

```
---
name: lowercase-letters-numbers-hyphens-only
description: Use when [specific triggering conditions]. Include keywords like error messages, tool names, symptoms.
---

[Skill body: Clear, actionable instructions with specific commands and file paths.]
```

## Examples

Example 1:
```
---
name: run-tests-after-rust-changes
description: Use when modifying .rs files in src/, when making code changes that could break functionality, or when the user mentions testing.
---

After modifying any Rust source file (.rs), always run the test suite:

1. Run `cargo test` in the workspace root
2. If tests fail, fix the failing tests before proceeding
3. Run `cargo clippy` to check for warnings
```

Example 2:
```
---
name: python-uv-package-management
description: Use when installing Python packages, setting up virtual environments, seeing pip-related errors, or when pyproject.toml is present.
---

Always use `uv` for Python package management instead of `pip`:

1. Install packages: `uv pip install <package>`
2. Create virtual environments: `uv venv`
3. Sync from requirements: `uv pip sync requirements.txt`
4. Never use bare `pip install`
```

## Requirements

- **name**: lowercase letters, numbers, and hyphens only. Descriptive of the skill's purpose.
- **description**: MUST start with "Use when...". Describe TRIGGERING CONDITIONS, not what the skill does. Include relevant keywords (error messages, tool names, file types). Total YAML frontmatter must be under 1024 characters.
- **body**: Actionable, specific instructions. Use numbered steps for procedures. Reference concrete commands and paths.

Return ONLY the skill content (YAML frontmatter + body), no explanation or wrapping."#,
        pattern_type = pattern.pattern_type,
        description = pattern.description,
        suggested_content = pattern.suggested_content,
        related = related,
        times_seen = pattern.times_seen,
    )
}

fn build_validation_prompt(skill_content: &str, pattern: &Pattern) -> String {
    format!(
        r#"You are a quality reviewer for Claude Code skills. Review the following skill and determine if it meets quality standards.

## Skill Content

```
{skill_content}
```

## Original Pattern

**Description:** {description}
**Suggested Content:** {suggested_content}

## Quality Criteria

1. **name** field: lowercase letters, numbers, and hyphens only
2. **description**: Starts with "Use when..."
3. **description**: Describes triggering conditions, NOT what the skill does
4. **Total YAML frontmatter**: Under 1024 characters
5. **Body**: Actionable and specific instructions
6. **Relevance**: Skill actually addresses the original pattern

Return ONLY a JSON object (no markdown wrapping):
{{"valid": true, "feedback": ""}}
or
{{"valid": false, "feedback": "explanation of what needs to be fixed"}}"#,
        skill_content = skill_content,
        description = pattern.description,
        suggested_content = pattern.suggested_content,
    )
}

/// Parse the skill name from YAML frontmatter.
pub fn parse_skill_name(content: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() || lines[0].trim() != "---" {
        return None;
    }

    for line in &lines[1..] {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("name:") {
            let name = rest.trim().trim_matches('"').trim_matches('\'').to_string();
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
                return Some(name);
            }
        }
    }
    None
}

/// Check if the content has valid frontmatter structure.
fn has_valid_frontmatter(content: &str) -> bool {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() || lines[0].trim() != "---" {
        return false;
    }
    // Find closing ---
    lines[1..].iter().any(|line| line.trim() == "---")
}

/// Parse the validation response JSON.
/// With `--json-schema` constrained decoding, the response is guaranteed valid JSON.
fn parse_validation(text: &str) -> Option<SkillValidation> {
    serde_json::from_str(text.trim()).ok()
}

/// Determine the skill file path: {project}/.claude/skills/{name}/SKILL.md
pub fn skill_path(project_root: &str, name: &str) -> String {
    format!("{project_root}/.claude/skills/{name}/SKILL.md")
}

/// Find the writing-skills SKILL.md content from the plugins cache directory.
/// Testable helper — takes the plugins directory as a parameter.
///
/// Glob pattern: `{plugins_dir}/cache/*/superpowers/*/skills/writing-skills/SKILL.md`
/// Picks the last match (highest version when sorted ascending by path).
/// Concatenates SKILL.md with companion .md files (everything except SKILL.md).
fn find_writing_skills_in_plugins_dir(plugins_dir: &std::path::Path) -> Option<String> {
    let pattern = plugins_dir
        .join("cache")
        .join("*")
        .join("superpowers")
        .join("*")
        .join("skills")
        .join("writing-skills")
        .join("SKILL.md");

    let pattern_str = pattern.to_string_lossy();
    let mut matches: Vec<std::path::PathBuf> = glob::glob(&pattern_str)
        .ok()?
        .filter_map(|r| r.ok())
        .collect();

    if matches.is_empty() {
        return None;
    }

    matches.sort();
    let skill_path = matches.last()?;
    let skill_dir = skill_path.parent()?;

    let skill_content = std::fs::read_to_string(skill_path).ok()?;

    // Read companion .md files from the same directory (everything except SKILL.md)
    let mut companions: Vec<std::path::PathBuf> = std::fs::read_dir(skill_dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|p| {
            p.extension().map(|ext| ext == "md").unwrap_or(false)
                && p.file_name().map(|n| n != "SKILL.md").unwrap_or(false)
        })
        .collect();
    companions.sort();

    let mut result = skill_content;
    for companion in &companions {
        if let Ok(companion_content) = std::fs::read_to_string(companion) {
            let filename = companion
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            result.push_str(&format!(
                "\n\n---\n## Companion: {filename}\n\n{companion_content}"
            ));
        }
    }

    Some(result)
}

/// Find writing-skills content from the global Claude plugins cache.
/// Reads `~/.claude/plugins` and delegates to `find_writing_skills_in_plugins_dir`.
pub fn find_writing_skills_content() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let plugins_dir = std::path::PathBuf::from(home).join(".claude").join("plugins");
    find_writing_skills_in_plugins_dir(&plugins_dir)
}

/// Check if superpowers plugin is installed by examining a specific plugins file.
/// Testable helper — takes the file path as a parameter.
fn check_superpowers_in_file(path: &std::path::Path) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else { return false };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else { return false };
    json.get("plugins")
        .and_then(|p| p.as_object())
        .map(|plugins| plugins.keys().any(|k| k.contains("superpowers")))
        .unwrap_or(false)
}

/// Check if the superpowers plugin is installed globally.
/// Reads `~/.claude/plugins/installed_plugins.json`.
pub fn is_superpowers_installed() -> bool {
    let home = std::env::var("HOME").unwrap_or_default();
    let path = std::path::PathBuf::from(home)
        .join(".claude")
        .join("plugins")
        .join("installed_plugins.json");
    check_superpowers_in_file(&path)
}

/// Generate a kebab-case slug from node content for use as a skill directory name.
/// Splits on all non-alphanumeric characters (including hyphens), filters words >= 2 chars,
/// takes first 4, joins with hyphens, and lowercases the result.
pub fn generate_skill_slug(content: &str) -> String {
    content
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 2)
        .take(4)
        .collect::<Vec<_>>()
        .join("-")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_writing_skills_in_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let skill_dir = dir.path()
            .join("cache")
            .join("marketplace")
            .join("superpowers")
            .join("1.0.0")
            .join("skills")
            .join("writing-skills");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Writing Skills\nMain content here.").unwrap();
        std::fs::write(skill_dir.join("best-practices.md"), "# Best Practices\nCompanion content.").unwrap();

        let result = find_writing_skills_in_plugins_dir(dir.path());
        assert!(result.is_some());
        let content = result.unwrap();
        assert!(content.contains("Main content here."));
        assert!(content.contains("Companion content."));
    }

    #[test]
    fn test_find_writing_skills_in_dir_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = find_writing_skills_in_plugins_dir(dir.path());
        assert!(result.is_none());
    }

    #[test]
    fn test_find_writing_skills_picks_latest_version() {
        let dir = tempfile::TempDir::new().unwrap();
        for version in &["1.0.0", "2.0.0"] {
            let skill_dir = dir.path()
                .join("cache")
                .join("mkt")
                .join("superpowers")
                .join(version)
                .join("skills")
                .join("writing-skills");
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(skill_dir.join("SKILL.md"), format!("version {version}")).unwrap();
        }
        let result = find_writing_skills_in_plugins_dir(dir.path());
        assert!(result.is_some());
        let content = result.unwrap();
        assert!(content.contains("version"));
    }

    #[test]
    fn test_parse_skill_name_valid() {
        let content = "---\nname: run-tests-after-changes\ndescription: Use when modifying files\n---\n\nBody here.";
        assert_eq!(parse_skill_name(content), Some("run-tests-after-changes".to_string()));
    }

    #[test]
    fn test_parse_skill_name_quoted() {
        let content = "---\nname: \"my-skill\"\ndescription: Use when stuff\n---\n\nBody.";
        assert_eq!(parse_skill_name(content), Some("my-skill".to_string()));
    }

    #[test]
    fn test_parse_skill_name_invalid_chars() {
        let content = "---\nname: My Skill Name\ndescription: test\n---\n";
        assert_eq!(parse_skill_name(content), None);
    }

    #[test]
    fn test_parse_skill_name_no_frontmatter() {
        let content = "Just some text";
        assert_eq!(parse_skill_name(content), None);
    }

    #[test]
    fn test_has_valid_frontmatter() {
        assert!(has_valid_frontmatter("---\nname: test\n---\nbody"));
        assert!(!has_valid_frontmatter("no frontmatter"));
        assert!(!has_valid_frontmatter("---\nno closing delimiter"));
    }

    #[test]
    fn test_parse_validation_valid() {
        let text = r#"{"valid": true, "feedback": ""}"#;
        let v = parse_validation(text).unwrap();
        assert!(v.valid);
        assert!(v.feedback.is_empty());
    }

    #[test]
    fn test_parse_validation_invalid() {
        let text = r#"{"valid": false, "feedback": "description doesn't start with Use when"}"#;
        let v = parse_validation(text).unwrap();
        assert!(!v.valid);
        assert!(v.feedback.contains("Use when"));
    }

    #[test]
    fn test_skill_validation_schema_is_valid_json() {
        let value: serde_json::Value = serde_json::from_str(SKILL_VALIDATION_SCHEMA)
            .expect("SKILL_VALIDATION_SCHEMA must be valid JSON");
        assert_eq!(value["type"], "object");
        assert!(value["properties"]["valid"].is_object());
        assert!(value["properties"]["feedback"].is_object());
    }

    #[test]
    fn test_skill_path() {
        assert_eq!(
            skill_path("/home/user/project", "run-tests"),
            "/home/user/project/.claude/skills/run-tests/SKILL.md"
        );
    }

    #[test]
    fn test_node_to_pattern() {
        use crate::models::*;
        use chrono::Utc;
        let node = KnowledgeNode {
            id: "node-1".to_string(),
            node_type: NodeType::Skill,
            scope: NodeScope::Global,
            project_id: None,
            content: "Pre-PR checklist: run tests, lint, format, commit".to_string(),
            confidence: 0.78,
            status: NodeStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            projected_at: None,
            pr_url: None,
        };

        let pattern = node_to_pattern(&node);
        assert_eq!(pattern.id, "node-1");
        assert_eq!(pattern.description, node.content);
        assert_eq!(pattern.suggested_content, node.content);
        assert_eq!(pattern.confidence, 0.78);
        assert_eq!(pattern.suggested_target, SuggestedTarget::Skill);
    }

    #[test]
    fn test_is_superpowers_installed_with_valid_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let plugins_path = dir.path().join("installed_plugins.json");
        std::fs::write(&plugins_path, r#"{"version":2,"plugins":{"superpowers@marketplace":[{"scope":"user"}]}}"#).unwrap();
        assert!(check_superpowers_in_file(&plugins_path));
    }

    #[test]
    fn test_is_superpowers_installed_no_superpowers() {
        let dir = tempfile::TempDir::new().unwrap();
        let plugins_path = dir.path().join("installed_plugins.json");
        std::fs::write(&plugins_path, r#"{"version":2,"plugins":{"other-plugin@marketplace":[]}}"#).unwrap();
        assert!(!check_superpowers_in_file(&plugins_path));
    }

    #[test]
    fn test_is_superpowers_installed_missing_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let plugins_path = dir.path().join("nonexistent.json");
        assert!(!check_superpowers_in_file(&plugins_path));
    }

    #[test]
    fn test_is_superpowers_installed_invalid_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let plugins_path = dir.path().join("installed_plugins.json");
        std::fs::write(&plugins_path, "not json").unwrap();
        assert!(!check_superpowers_in_file(&plugins_path));
    }

    #[test]
    fn test_generate_skill_slug_basic() {
        assert_eq!(
            generate_skill_slug("Pre-PR checklist: run tests, lint, format"),
            "pre-pr-checklist-run"
        );
    }

    #[test]
    fn test_generate_skill_slug_short_words() {
        assert_eq!(
            generate_skill_slug("CI check failures before merging"),
            "ci-check-failures-before"
        );
    }

    #[test]
    fn test_generate_skill_slug_single_char_filtered() {
        assert_eq!(
            generate_skill_slug("Run a test before commit"),
            "run-test-before-commit"
        );
    }

    #[test]
    fn test_generate_skill_slug_uppercase() {
        assert_eq!(
            generate_skill_slug("Rust Error Handling Pattern"),
            "rust-error-handling-pattern"
        );
    }

    #[test]
    fn test_generate_skill_slug_already_kebab() {
        assert_eq!(
            generate_skill_slug("pre-commit-hook"),
            "pre-commit-hook"
        );
    }
}
