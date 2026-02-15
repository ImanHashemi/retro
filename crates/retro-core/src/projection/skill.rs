use crate::analysis::backend::AnalysisBackend;
use crate::errors::CoreError;
use crate::models::{Pattern, SkillDraft, SkillValidation};
use crate::util;

const MAX_RETRIES: usize = 2;

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
        let response = backend.execute(&prompt)?;
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
        match backend.execute(&validation_prompt) {
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
fn parse_validation(text: &str) -> Option<SkillValidation> {
    let json_str = util::strip_code_fences(text);
    serde_json::from_str(&json_str).ok()
}

/// Determine the skill file path: {project}/.claude/skills/{name}/SKILL.md
pub fn skill_path(project_root: &str, name: &str) -> String {
    format!("{project_root}/.claude/skills/{name}/SKILL.md")
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_parse_validation_markdown_wrapped() {
        let text = "```json\n{\"valid\": true, \"feedback\": \"\"}\n```";
        let v = parse_validation(text).unwrap();
        assert!(v.valid);
    }

    #[test]
    fn test_skill_path() {
        assert_eq!(
            skill_path("/home/user/project", "run-tests"),
            "/home/user/project/.claude/skills/run-tests/SKILL.md"
        );
    }
}
