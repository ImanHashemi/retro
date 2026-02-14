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

These patterns have already been discovered. If you find NEW evidence supporting an existing pattern, return an "update" action with the existing pattern's ID. Only create "new" patterns for genuinely new findings.

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
- Don't create duplicate patterns — check existing ones first
- Return ONLY the JSON object, no other text"#
    );

    prompt
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
