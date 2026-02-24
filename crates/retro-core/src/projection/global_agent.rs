use crate::analysis::backend::AnalysisBackend;
use crate::errors::CoreError;
use crate::models::{AgentDraft, Pattern};
use crate::util;

/// Generate a global agent from a pattern via AI.
pub fn generate_agent(
    backend: &dyn AnalysisBackend,
    pattern: &Pattern,
) -> Result<AgentDraft, CoreError> {
    let prompt = build_generation_prompt(pattern);
    let response = backend.execute(&prompt, None)?;
    let content = util::strip_code_fences(&response.text);

    let name = parse_agent_name(&content).ok_or_else(|| {
        CoreError::Analysis(format!(
            "generated agent has no valid 'name' in frontmatter for pattern {}",
            pattern.id
        ))
    })?;

    Ok(AgentDraft {
        name,
        content,
        pattern_id: pattern.id.clone(),
    })
}

fn build_generation_prompt(pattern: &Pattern) -> String {
    let related = if pattern.related_files.is_empty() {
        "None".to_string()
    } else {
        pattern.related_files.join(", ")
    };

    format!(
        r#"You are an expert at writing Claude Code global agents. A global agent is a personal agent configuration file that applies across all projects.

Generate a global agent for the following discovered pattern:

**Pattern Type:** {pattern_type}
**Description:** {description}
**Suggested Content:** {suggested_content}
**Related Files:** {related}
**Times Seen:** {times_seen}

## Agent Format

The agent MUST follow this exact format:

```
---
name: lowercase-letters-numbers-hyphens-only
description: When and how to use this agent
model: sonnet
color: blue
---

[Agent body: Clear instructions for the agent's behavior and capabilities.]
```

## Requirements

- **name**: lowercase letters, numbers, and hyphens only
- **description**: Clear description of when/how to use the agent
- **model**: Use "sonnet" as default
- **color**: Use "blue" as default
- **body**: Clear, actionable instructions for the agent

Return ONLY the agent content (YAML frontmatter + body), no explanation or wrapping."#,
        pattern_type = pattern.pattern_type,
        description = pattern.description,
        suggested_content = pattern.suggested_content,
        related = related,
        times_seen = pattern.times_seen,
    )
}

/// Parse the agent name from YAML frontmatter.
pub fn parse_agent_name(content: &str) -> Option<String> {
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
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            {
                return Some(name);
            }
        }
    }
    None
}

/// Determine the agent file path: ~/.claude/agents/{name}.md
pub fn agent_path(claude_dir: &str, name: &str) -> String {
    format!("{claude_dir}/agents/{name}.md")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_agent_name_valid() {
        let content =
            "---\nname: code-reviewer\ndescription: Reviews code\nmodel: sonnet\ncolor: blue\n---\n\nBody.";
        assert_eq!(
            parse_agent_name(content),
            Some("code-reviewer".to_string())
        );
    }

    #[test]
    fn test_parse_agent_name_quoted() {
        let content = "---\nname: \"my-agent\"\n---\n";
        assert_eq!(parse_agent_name(content), Some("my-agent".to_string()));
    }

    #[test]
    fn test_parse_agent_name_invalid() {
        let content = "---\nname: My Agent\n---\n";
        assert_eq!(parse_agent_name(content), None);
    }

    #[test]
    fn test_parse_agent_name_no_frontmatter() {
        assert_eq!(parse_agent_name("no frontmatter"), None);
    }

    #[test]
    fn test_agent_path() {
        assert_eq!(
            agent_path("/home/user/.claude", "code-reviewer"),
            "/home/user/.claude/agents/code-reviewer.md"
        );
    }

}
