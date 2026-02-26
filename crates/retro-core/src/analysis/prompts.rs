use crate::models::{
    CompactPattern, CompactSession, CompactUserMessage, ContextSnapshot, Pattern, Session,
};

const MAX_USER_MSG_LEN: usize = 500;
const MAX_USER_MSGS_PER_SESSION: usize = 300;
const MAX_PROMPT_CHARS: usize = 150_000;
const MAX_CONTEXT_SUMMARY_CHARS: usize = 5_000;

/// Build a compact summary of installed context for the analysis prompt.
/// Includes project skills, plugin skills, retro-managed CLAUDE.md rules, global agents,
/// and MEMORY.md notes (personal, informational only). Sections are omitted if empty.
/// Capped at 5K chars.
pub fn build_context_summary(snapshot: &ContextSnapshot) -> String {
    let mut sections: Vec<String> = Vec::new();

    // Project skills (name + description from frontmatter)
    let project_skills: Vec<(String, String)> = snapshot
        .skills
        .iter()
        .filter_map(|s| crate::ingest::context::parse_skill_frontmatter(&s.content))
        .collect();

    if !project_skills.is_empty() {
        let mut section = "### Project Skills\n".to_string();
        for (name, desc) in &project_skills {
            section.push_str(&format!("- {name}: {desc}\n"));
        }
        sections.push(section);
    }

    // Plugin skills
    if !snapshot.plugin_skills.is_empty() {
        let mut section = "### Plugin Skills\n".to_string();
        for ps in &snapshot.plugin_skills {
            section.push_str(&format!("- [{}] {}: {}\n", ps.plugin_name, ps.skill_name, ps.description));
        }
        sections.push(section);
    }

    // Existing retro-managed CLAUDE.md rules
    if let Some(ref claude_md) = snapshot.claude_md {
        if let Some(rules) = crate::projection::claude_md::read_managed_section(claude_md) {
            if !rules.is_empty() {
                let mut section = "### Existing CLAUDE.md Rules (retro-managed)\n".to_string();
                for rule in &rules {
                    section.push_str(&format!("- {rule}\n"));
                }
                sections.push(section);
            }
        }
    }

    // Global agents
    if !snapshot.global_agents.is_empty() {
        let mut section = "### Global Agents\n".to_string();
        for agent in &snapshot.global_agents {
            // Extract just the filename without extension
            let name = std::path::Path::new(&agent.path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| agent.path.clone());
            section.push_str(&format!("- {name}\n"));
        }
        sections.push(section);
    }

    // MEMORY.md (personal notes Claude Code wrote for itself)
    if let Some(ref memory) = snapshot.memory_md {
        if !memory.trim().is_empty() {
            let mut section = "### MEMORY.md (personal, not shared with team)\n".to_string();
            section.push_str(memory);
            section.push('\n');
            sections.push(section);
        }
    }

    let mut result = sections.join("\n");

    // Cap at budget — truncate plugin skills section first if over
    if result.len() > MAX_CONTEXT_SUMMARY_CHARS {
        // Try without plugin skills
        sections.retain(|s| !s.starts_with("### Plugin Skills"));
        result = sections.join("\n");
    }

    if result.len() > MAX_CONTEXT_SUMMARY_CHARS {
        // Hard truncate at char boundary
        let mut i = MAX_CONTEXT_SUMMARY_CHARS;
        while i > 0 && !result.is_char_boundary(i) {
            i -= 1;
        }
        result.truncate(i);
    }

    result
}

/// Build the pattern discovery prompt for a batch of sessions.
pub fn build_analysis_prompt(
    sessions: &[Session],
    existing_patterns: &[Pattern],
    context_summary: Option<&str>,
) -> String {
    let mut compact_sessions: Vec<CompactSession> = sessions.iter().map(to_compact_session).collect();
    let compact_patterns = existing_patterns.iter().map(to_compact_pattern).collect::<Vec<_>>();

    let patterns_json =
        serde_json::to_string_pretty(&compact_patterns).unwrap_or_else(|_| "[]".to_string());

    let context_section = match context_summary {
        Some(summary) if !summary.is_empty() => format!(
            r#"

## Installed Context

The following context is already installed for this project.

**Important:** MEMORY.md contains personal notes that Claude Code wrote for itself — these are NOT shared with the team. If a pattern overlaps with MEMORY.md content but would benefit the team as a shared rule or skill, **still create it** (do not mark as `db_only`). MEMORY.md overlap only justifies `db_only` for patterns targeting `global_agent`. For all other installed context (skills, CLAUDE.md rules, agents), overlap means the pattern is already covered — skip it or mark `db_only`.

{summary}
"#
        ),
        _ => String::new(),
    };

    // Estimate base prompt size (template + patterns + context), then fit as many sessions as possible
    let base_size = 3000 + patterns_json.len() + context_section.len();
    let budget = MAX_PROMPT_CHARS.saturating_sub(base_size);

    // Progressively drop sessions from the end until we fit
    let mut sessions_json = serde_json::to_string_pretty(&compact_sessions).unwrap_or_else(|_| "[]".to_string());
    while sessions_json.len() > budget && compact_sessions.len() > 1 {
        compact_sessions.pop();
        sessions_json = serde_json::to_string_pretty(&compact_sessions).unwrap_or_else(|_| "[]".to_string());
    }

    let prompt = format!(
        r#"You are an expert at analyzing AI coding agent session histories to discover **real, recurring patterns**.

A pattern is a behavior, preference, or workflow that appears in **2 or more sessions**. A single occurrence is just an observation — not a pattern. Your job is to find things worth automating because they keep happening.

Analyze the following session data from Claude Code conversations. Look for:

1. **Repetitive Instructions** — Things the user tells the agent across **multiple sessions** (e.g., "always use uv not pip", "run clippy before committing"). The same instruction given once is not a pattern — it becomes one when it recurs.

2. **Recurring Mistakes** — The same **class** of error the agent makes in **multiple sessions** (e.g., using the wrong API, forgetting edge cases, picking the wrong tool). A bug encountered and fixed once is not a recurring mistake.

3. **Workflow Patterns** — Specific multi-step procedures the user guides the agent through in **multiple sessions** (e.g., "first run lint, then test, then commit with this message format"). A workflow followed once for a particular task is not a pattern.

4. **Explicit Directives** — When the user uses strong directive language like **"always"**, **"never"**, **"must"**, or **"don't ever"**, they are explicitly stating a project rule or convention. These are high-confidence signals even from a single session. Examples:
   - "Always create API routes using the router factory pattern"
   - "Never import directly from internal modules, use the public API"
   - "You must run migrations before testing"
   These are typically project-specific conventions about how code should be written, not workflow preferences. They belong in `claude_md`.

## What is NOT a pattern

Do NOT report any of the following:
- **One-time bug fixes** — A bug that was encountered and resolved in a single session
- **Task-specific instructions** — Directions that only applied to one particular task and are not general preferences

## Confidence calibration

Confidence reflects how certain you are this is a real, recurring pattern:
- **Explicit directive (single session)**: When the user uses "always", "never", "must", or similar imperative language to state a rule, report with confidence **0.7-0.85** even from a single session. The directive language itself is strong evidence this is a standing rule, not a one-time instruction. Target: `claude_md`.
- **Seen in 1 session only (no directive language)**: Report with confidence 0.4-0.5 if the signal is clear and specific. These are stored as candidate observations and will be confirmed when the behavior recurs in a future session. Do NOT report vague or ambiguous single-session observations.
- **Seen in 2 sessions**: Confidence 0.6-0.75 depending on how clear and specific the pattern is.
- **Seen in 3+ sessions**: Confidence 0.7-1.0.

**suggested_target** — where should this pattern be projected?
- `claude_md` — Simple rules, project conventions, or explicit directives ("always do X", "never do Y"). Explicit directives qualify from a single session. Other rules require 2+ sessions.
- `skill` — Multi-step procedures or complex workflows. Requires evidence from 2+ sessions.
- `global_agent` — Cross-project personal preferences. Requires evidence from 2+ sessions.
- `db_only` — Already covered by installed context (skill, plugin, CLAUDE.md rule, or agent). Use this when a real pattern exists but is already handled.

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
{context_section}
## Session Data

```json
{sessions_json}
```

## Response Format

Return a JSON object with a "reasoning" string and a "patterns" array. Begin with a "reasoning" field: a 1-2 sentence summary of what you observed across the sessions and why you did or didn't find patterns. Each element of the "patterns" array is either a new pattern or an update to an existing one:

```json
{{
  "reasoning": "Sessions contained mostly one-off bug fixes with no recurring themes. One explicit directive about testing was found.",
  "patterns": [
    {{
      "action": "new",
      "pattern_type": "repetitive_instruction",
      "description": "Clear description of what was observed across sessions",
      "confidence": 0.85,
      "source_sessions": ["session-id-1", "session-id-2"],
      "related_files": ["path/to/relevant/file"],
      "suggested_content": "The rule or instruction to add (e.g., 'Always run cargo clippy -- -D warnings before committing')",
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
- **Quality over quantity** — fewer strong patterns are better than many weak ones. When in doubt, skip it.
- Strong patterns require evidence from **2+ sessions**. Single-session observations may be reported only if the signal is clear and specific (confidence 0.4-0.5).
- Only return patterns with confidence >= 0.4
- Be specific in descriptions — vague patterns like "user prefers clean code" are useless. State the concrete behavior.
- For `suggested_content`, write the actual rule or instruction as it should appear in the target
- CRITICAL: Do not create duplicate patterns. Two patterns about the same underlying behavior are duplicates even if described differently. Always check existing patterns for semantic overlap, not just textual similarity.
- Do not suggest skills or rules that duplicate installed plugin functionality
- CRITICAL: Return ONLY the raw JSON object. No prose, no explanation, no markdown formatting, no commentary before or after. Your entire response must be parseable as a single JSON object starting with {{ and ending with }}. If no patterns found, return {{"reasoning": "your observation summary", "patterns": []}}"#
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
        .take(MAX_USER_MSGS_PER_SESSION)
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
    use crate::models::{AgentFile, PluginSkillSummary, SkillFile};

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

    fn empty_snapshot() -> ContextSnapshot {
        ContextSnapshot {
            claude_md: None,
            skills: Vec::new(),
            memory_md: None,
            global_agents: Vec::new(),
            plugin_skills: Vec::new(),
        }
    }

    #[test]
    fn test_build_context_summary_empty() {
        let snapshot = empty_snapshot();
        let summary = build_context_summary(&snapshot);
        assert!(summary.is_empty());
    }

    #[test]
    fn test_build_context_summary_full() {
        let snapshot = ContextSnapshot {
            claude_md: Some("before\n<!-- retro:managed:start -->\n- Always use uv\n- Run cargo test\n<!-- retro:managed:end -->\nafter".to_string()),
            skills: vec![SkillFile {
                path: "skills/tdd/SKILL.md".to_string(),
                content: "---\nname: tdd\ndescription: Test-driven development workflow\n---\nbody".to_string(),
            }],
            memory_md: None,
            global_agents: vec![AgentFile {
                path: "/home/user/.claude/agents/code-reviewer.md".to_string(),
                content: "reviewer content".to_string(),
            }],
            plugin_skills: vec![PluginSkillSummary {
                plugin_name: "superpowers".to_string(),
                skill_name: "brainstorming".to_string(),
                description: "Explores user intent".to_string(),
            }],
        };
        let summary = build_context_summary(&snapshot);
        assert!(summary.contains("### Project Skills"));
        assert!(summary.contains("- tdd: Test-driven development workflow"));
        assert!(summary.contains("### Plugin Skills"));
        assert!(summary.contains("[superpowers] brainstorming: Explores user intent"));
        assert!(summary.contains("### Existing CLAUDE.md Rules (retro-managed)"));
        assert!(summary.contains("- Always use uv"));
        assert!(summary.contains("- Run cargo test"));
        assert!(summary.contains("### Global Agents"));
        assert!(summary.contains("- code-reviewer"));
    }

    #[test]
    fn test_build_context_summary_no_managed_section() {
        let snapshot = ContextSnapshot {
            claude_md: Some("# My CLAUDE.md\nNo managed section here.".to_string()),
            skills: Vec::new(),
            memory_md: None,
            global_agents: Vec::new(),
            plugin_skills: Vec::new(),
        };
        let summary = build_context_summary(&snapshot);
        // No sections should appear
        assert!(summary.is_empty());
    }

    #[test]
    fn test_build_context_summary_budget_cap() {
        // Create a snapshot with many plugin skills to exceed the 5K cap
        let mut plugin_skills = Vec::new();
        for i in 0..200 {
            plugin_skills.push(PluginSkillSummary {
                plugin_name: format!("plugin-{i}"),
                skill_name: format!("skill-with-a-long-name-{i}"),
                description: format!("A fairly long description for skill number {i} that takes up space"),
            });
        }
        let snapshot = ContextSnapshot {
            claude_md: None,
            skills: vec![SkillFile {
                path: "skills/my-skill/SKILL.md".to_string(),
                content: "---\nname: my-skill\ndescription: A project skill\n---\nbody".to_string(),
            }],
            memory_md: None,
            global_agents: Vec::new(),
            plugin_skills,
        };
        let summary = build_context_summary(&snapshot);
        assert!(summary.len() <= 5000);
        // Plugin skills should have been dropped, but project skills retained
        assert!(summary.contains("### Project Skills"));
        assert!(!summary.contains("### Plugin Skills"));
    }

    #[test]
    fn test_build_analysis_prompt_with_context() {
        let sessions = vec![Session {
            session_id: "sess-1".to_string(),
            project: "/test".to_string(),
            session_path: "/test/session.jsonl".to_string(),
            user_messages: vec![],
            assistant_messages: vec![],
            summaries: vec![],
            tools_used: vec![],
            errors: vec![],
            metadata: crate::models::SessionMetadata {
                cwd: None,
                version: None,
                git_branch: None,
                model: None,
            },
        }];
        let context = "### Plugin Skills\n- [superpowers] brainstorming: Explores intent\n";
        let prompt = build_analysis_prompt(&sessions, &[], Some(context));
        assert!(prompt.contains("## Installed Context"));
        assert!(prompt.contains("[superpowers] brainstorming"));
        assert!(prompt.contains("Already covered by installed context"));
        assert!(prompt.contains("Do not suggest skills or rules that duplicate installed plugin functionality"));
    }

    #[test]
    fn test_build_analysis_prompt_without_context() {
        let sessions = vec![Session {
            session_id: "sess-1".to_string(),
            project: "/test".to_string(),
            session_path: "/test/session.jsonl".to_string(),
            user_messages: vec![],
            assistant_messages: vec![],
            summaries: vec![],
            tools_used: vec![],
            errors: vec![],
            metadata: crate::models::SessionMetadata {
                cwd: None,
                version: None,
                git_branch: None,
                model: None,
            },
        }];
        let prompt = build_analysis_prompt(&sessions, &[], None);
        assert!(!prompt.contains("## Installed Context"));
        // Core prompt structure should still be there
        assert!(prompt.contains("## Existing Patterns"));
        assert!(prompt.contains("## Session Data"));
    }
}
