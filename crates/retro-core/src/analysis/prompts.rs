use crate::models::{
    CompactPattern, CompactSession, CompactUserMessage, Pattern, Session,
};

const MAX_USER_MSG_LEN: usize = 200;
const MAX_PROMPT_CHARS: usize = 150_000;

/// Build the pattern discovery prompt for a batch of sessions.
pub fn build_analysis_prompt(sessions: &[Session], existing_patterns: &[Pattern]) -> String {
    let mut compact_sessions: Vec<CompactSession> = sessions.iter().map(to_compact_session).collect();
    let compact_patterns = existing_patterns.iter().map(to_compact_pattern).collect::<Vec<_>>();

    let patterns_json =
        serde_json::to_string_pretty(&compact_patterns).unwrap_or_else(|_| "[]".to_string());

    // Estimate base prompt size (template + patterns), then fit as many sessions as possible
    let base_size = 3000 + patterns_json.len(); // ~3K for template text
    let budget = MAX_PROMPT_CHARS.saturating_sub(base_size);

    // Progressively drop sessions from the end until we fit
    let mut sessions_json = serde_json::to_string_pretty(&compact_sessions).unwrap_or_else(|_| "[]".to_string());
    while sessions_json.len() > budget && compact_sessions.len() > 1 {
        compact_sessions.pop();
        sessions_json = serde_json::to_string_pretty(&compact_sessions).unwrap_or_else(|_| "[]".to_string());
    }

    let prompt = format!(
        r#"You are an expert at analyzing AI coding agent session histories to discover patterns.

Analyze the following session data from Claude Code conversations. Look for:

1. **Repetitive Instructions** — Things the user tells the agent repeatedly across sessions (e.g., "always use uv not pip", "run tests after changes"). These indicate rules that should be automated.

2. **Recurring Mistakes** — Errors or wrong approaches the agent keeps making, especially ones the user has to correct (e.g., using wrong API, forgetting to handle edge cases). These indicate rules or skills needed.

3. **Workflow Patterns** — Multi-step procedures the user guides the agent through repeatedly (e.g., "first run lint, then test, then commit with this format"). These are ideal for skills.

For each pattern found, assess:
- **confidence** (0.0-1.0): How certain are you this is a real, recurring pattern? A single occurrence = low confidence (0.3-0.5). Multiple clear occurrences = high confidence (0.7-1.0).
- **suggested_target**: Where should this pattern be projected?
  - "claude_md" — Simple rules ("always do X", "never do Y")
  - "skill" — Multi-step procedures or complex workflows
  - "global_agent" — Cross-project personal preferences
  - "db_only" — Interesting but not actionable yet

## Existing Patterns

These patterns have already been discovered. **Before creating any "new" pattern, carefully check each existing pattern below.** If a new finding is about the same topic, behavior, or user preference as an existing pattern — even if the wording is completely different — you MUST use "update" with the existing pattern's ID rather than creating a new pattern.

Examples of patterns that should be merged (same topic, different wording):
- "User repeatedly asks to update docs after completing each phase" ↔ "After each phase completion, user expects documentation updates"
- "Always run tests before committing" ↔ "User insists on running the test suite prior to any git commit"
- "Use uv instead of pip" ↔ "User prefers uv as the Python package manager, not pip"

When in doubt, prefer "update" over "new" — duplicate patterns are worse than missed ones.

```json
{patterns_json}
```

## Session Data

```json
{sessions_json}
```

## Response Format

Return a JSON object with a "patterns" array. Each element is either a new pattern or an update to an existing one:

```json
{{
  "patterns": [
    {{
      "action": "new",
      "pattern_type": "repetitive_instruction",
      "description": "Clear description of what was observed",
      "confidence": 0.85,
      "source_sessions": ["session-id-1", "session-id-2"],
      "related_files": ["path/to/relevant/file"],
      "suggested_content": "The rule or instruction to add (e.g., 'Always use uv for Python package management')",
      "suggested_target": "claude_md"
    }},
    {{
      "action": "update",
      "existing_id": "existing-pattern-uuid",
      "new_sessions": ["session-id-3"],
      "new_confidence": 0.92
    }}
  ]
}}
```

Important:
- Only return patterns you're confident about (confidence >= 0.5)
- Be specific in descriptions — vague patterns are useless
- For suggested_content, write the actual rule/instruction as it should appear
- CRITICAL: Do not create duplicate patterns. Two patterns about the same underlying behavior are duplicates even if described differently. Always check existing patterns for semantic overlap, not just textual similarity
- Return ONLY the JSON object, no other text"#
    );

    prompt
}

/// Build the context audit prompt for redundancy/contradiction detection.
pub fn build_audit_prompt(
    claude_md: Option<&str>,
    skills: &[(String, String)],
    memory_md: Option<&str>,
    agents: &[(String, String)],
) -> String {
    let claude_md_section = match claude_md {
        Some(content) => format!("### CLAUDE.md\n```\n{content}\n```"),
        None => "### CLAUDE.md\n(not present)".to_string(),
    };

    let skills_section = if skills.is_empty() {
        "### Skills\n(none)".to_string()
    } else {
        let mut s = "### Skills\n".to_string();
        for (path, content) in skills {
            s.push_str(&format!("**{path}**:\n```\n{content}\n```\n\n"));
        }
        s
    };

    let memory_section = match memory_md {
        Some(content) => format!("### MEMORY.md\n```\n{content}\n```"),
        None => "### MEMORY.md\n(not present)".to_string(),
    };

    let agents_section = if agents.is_empty() {
        "### Global Agents\n(none)".to_string()
    } else {
        let mut s = "### Global Agents\n".to_string();
        for (path, content) in agents {
            s.push_str(&format!("**{path}**:\n```\n{content}\n```\n\n"));
        }
        s
    };

    format!(
        r#"You are an expert at reviewing AI coding agent context for quality and consistency.

Review the following context files used by Claude Code. Look for:

1. **Redundant** — Same information appears in multiple places (e.g., a rule in CLAUDE.md and a skill that says the same thing). Suggest consolidation.

2. **Contradictory** — Conflicting instructions across files (e.g., one says "use pip" and another says "use uv"). Flag for review.

3. **Oversized** — CLAUDE.md or skills that are excessively long and should be broken up or consolidated.

4. **Stale** — Rules or skills that reference outdated tools, deprecated patterns, or things that no longer apply.

## Context Files

{claude_md_section}

{skills_section}

{memory_section}

{agents_section}

## Response Format

Return a JSON object with a "findings" array:

```json
{{
  "findings": [
    {{
      "finding_type": "redundant",
      "description": "Clear description of what's redundant/contradictory/etc",
      "affected_items": ["CLAUDE.md", ".claude/skills/some-skill/SKILL.md"],
      "suggestion": "Specific suggestion for how to fix this"
    }}
  ]
}}
```

Important:
- Only report genuine issues, not minor style differences
- Be specific about which files and which content is affected
- Return ONLY the JSON object, no other text
- If no issues found, return {{"findings": []}}"#
    )
}

fn to_compact_session(session: &Session) -> CompactSession {
    let user_messages: Vec<CompactUserMessage> = session
        .user_messages
        .iter()
        .map(|m| CompactUserMessage {
            text: truncate_str(&m.text, MAX_USER_MSG_LEN),
            timestamp: m.timestamp.clone(),
        })
        .collect();

    let thinking_highlights: Vec<String> = session
        .assistant_messages
        .iter()
        .filter_map(|m| m.thinking_summary.clone())
        .collect();

    CompactSession {
        session_id: session.session_id.clone(),
        project: session.project.clone(),
        user_messages,
        tools_used: session.tools_used.clone(),
        errors: session.errors.clone(),
        thinking_highlights,
        summaries: session.summaries.clone(),
    }
}

fn to_compact_pattern(pattern: &Pattern) -> CompactPattern {
    CompactPattern {
        id: pattern.id.clone(),
        pattern_type: pattern.pattern_type.to_string(),
        description: pattern.description.clone(),
        confidence: pattern.confidence,
        times_seen: pattern.times_seen,
        suggested_target: pattern.suggested_target.to_string(),
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    // Find valid UTF-8 boundary
    let mut i = max;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    format!("{}...", &s[..i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_audit_prompt_all_present() {
        let skills = vec![
            ("skills/lint/SKILL.md".to_string(), "lint skill content".to_string()),
        ];
        let agents = vec![
            ("agents/helper.md".to_string(), "helper agent content".to_string()),
        ];
        let prompt = build_audit_prompt(
            Some("# CLAUDE.md content"),
            &skills,
            Some("# MEMORY.md content"),
            &agents,
        );
        assert!(prompt.contains("# CLAUDE.md content"));
        assert!(prompt.contains("lint skill content"));
        assert!(prompt.contains("# MEMORY.md content"));
        assert!(prompt.contains("helper agent content"));
        assert!(prompt.contains("\"findings\""));
    }

    #[test]
    fn test_build_audit_prompt_none_present() {
        let prompt = build_audit_prompt(None, &[], None, &[]);
        assert!(prompt.contains("(not present)"));
        assert!(prompt.contains("(none)"));
    }

    #[test]
    fn test_build_audit_prompt_partial() {
        let prompt = build_audit_prompt(Some("rules here"), &[], None, &[]);
        assert!(prompt.contains("rules here"));
        assert!(prompt.contains("### MEMORY.md\n(not present)"));
        assert!(prompt.contains("### Skills\n(none)"));
    }
}
